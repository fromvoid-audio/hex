//! Control-side MIDI worker command/event contract, bounded outbox, and lifecycle.

use crate::framing::{ChannelFilter, Controllers};
use crate::held_notes::HeldNoteInput;
use crate::mpe::MpeConfig;
use crate::native;
use crate::timebase::TimestampedNoteEvent;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub name: String,
}

/// Constructed on control; the native callback only invokes this bounded sink.
pub struct Sink {
    pub publish: Box<dyn Fn(Controllers) + Send>,
    /// Ordered native input messages with monotonic hardware timestamps,
    /// independent of the step-entry snapshot.
    pub note: Box<dyn Fn(TimestampedNoteEvent) + Send>,
    pub notes: Arc<HeldNoteInput>,
    pub cancel: Box<dyn Fn() + Send>,
    pub is_cancelled: Box<dyn Fn() -> bool + Send>,
}

impl Sink {
    pub(crate) fn cancel(&self) {
        self.notes.cancel();
        (self.cancel)();
    }
}

pub enum Command {
    Discover {
        request_id: u64,
    },
    Connect {
        activation: u64,
        port: Port,
        channel: u8,
        sink: Sink,
    },
    /// Connect one or more ports (up to [`crate::MAX_AGGREGATED_PORTS`]) with a
    /// [`ChannelFilter`] (single channel or Omni), aggregating them through one
    /// serialized [`crate::MultiPortAggregator`].
    ConnectPorts {
        activation: u64,
        ports: Vec<Port>,
        channel_filter: ChannelFilter,
        sink: Sink,
    },
    /// Connect one or more ports (up to [`crate::MAX_AGGREGATED_PORTS`]) using one
    /// explicit bounded MPE zone configuration ([`MpeConfig`]). Does not
    /// auto-negotiate MIDI RPN; invalid zone configurations are rejected and
    /// cancel the sink.
    ConnectMpe {
        activation: u64,
        ports: Vec<Port>,
        config: MpeConfig,
        sink: Sink,
    },
    /// Update member (`1..=96` st) and master (`1..=48` st) MPE pitch-bend ranges
    /// on the matching active MPE connection without disconnecting or resetting
    /// held voices. Stale activations are ignored cleanly.
    SetMpePitchBendRanges {
        activation: u64,
        member_semitones: u8,
        master_semitones: u8,
    },
    Disconnect {
        activation: u64,
    },
}

#[derive(Debug)]
pub enum Event {
    Ports {
        request_id: u64,
        result: Result<Vec<Port>, String>,
    },
    Connection {
        activation: u64,
        result: Result<bool, String>,
    },
}

/// Command queue capacity shared by every handle; a full queue refuses the command.
pub(crate) const COMMAND_CAPACITY: usize = 4;
/// Event queue capacity; replies that do not fit wait in the worker's [`Outbox`].
pub(crate) const EVENT_CAPACITY: usize = 4;
/// Retry interval for a retained reply while the event queue stays full.
pub(crate) const RETRY_INTERVAL: Duration = Duration::from_millis(20);
/// Idle wait between connected-endpoint presence checks.
pub(crate) const IDLE_INTERVAL: Duration = Duration::from_millis(500);

/// Worker-private queue entry: public commands plus the owner's prompt shutdown wake.
pub(crate) enum Request {
    Command(Command),
    Shutdown,
}

/// Cloneable command/event access to one worker.
///
/// Commands use a bounded queue of `COMMAND_CAPACITY`; a full or closed queue refuses
/// immediately and cancels a refused connection sink. Port and connection replies are
/// never silently lost while the receiver exists: see `Outbox` for the overflow policy.
#[derive(Clone)]
pub struct Handle {
    commands: SyncSender<Request>,
    events: Arc<std::sync::Mutex<Receiver<Event>>>,
}

impl Handle {
    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        match self.commands.try_send(Request::Command(command)) {
            Ok(()) => Ok(()),
            Err(mpsc::TrySendError::Full(request) | mpsc::TrySendError::Disconnected(request)) => {
                if let Request::Command(
                    Command::Connect { sink, .. }
                    | Command::ConnectPorts { sink, .. }
                    | Command::ConnectMpe { sink, .. },
                ) = request
                {
                    sink.cancel();
                }
                Err("controller input worker is busy or unavailable")
            }
        }
    }

    pub fn try_event(&self) -> Result<Event, TryRecvError> {
        self.events
            .lock()
            .map_err(|_| TryRecvError::Disconnected)?
            .try_recv()
    }
}

/// Bounded worker-side custody for replies that do not fit the event queue.
///
/// At most one undelivered reply per kind (ports, connection) is retained and retried every
/// [`RETRY_INTERVAL`] until the receiver drains. A newer reply of the same kind replaces a
/// retained one unless it answers an older discovery id or activation: the UI issues both
/// monotonically and ignores stale answers, so only the newest reply per kind is observable.
/// A connection is never torn down because its reply is waiting. Replies are discarded only
/// once the receiver is gone. Note input bypasses this queue; held-note, note-off and
/// all-notes-off delivery are unaffected.
#[derive(Default)]
pub(crate) struct Outbox {
    ports: Option<(u64, Result<Vec<Port>, String>)>,
    connection: Option<(u64, Result<bool, String>)>,
    #[cfg(test)]
    pub(crate) on_full_retain: Option<SyncSender<()>>,
    #[cfg(test)]
    pub(crate) on_idle_wait: Option<SyncSender<()>>,
}

#[cfg(test)]
impl Outbox {
    pub(crate) fn with_full_retain(on_full_retain: SyncSender<()>) -> Self {
        Self {
            on_full_retain: Some(on_full_retain),
            ..Self::default()
        }
    }

    pub(crate) fn with_idle_wait(on_idle_wait: SyncSender<()>) -> Self {
        Self {
            on_idle_wait: Some(on_idle_wait),
            ..Self::default()
        }
    }
}

impl Outbox {
    pub(crate) fn retain(&mut self, event: Event) {
        match event {
            Event::Ports { request_id, result } => {
                if self
                    .ports
                    .as_ref()
                    .is_none_or(|(held, _)| *held <= request_id)
                {
                    self.ports = Some((request_id, result));
                }
            }
            Event::Connection { activation, result } => {
                if self
                    .connection
                    .as_ref()
                    .is_none_or(|(held, _)| *held <= activation)
                {
                    self.connection = Some((activation, result));
                }
            }
        }
    }

    /// Delivers retained replies that fit; full-queue replies stay retained.
    pub(crate) fn flush(&mut self, events: &SyncSender<Event>) {
        let ports = self
            .ports
            .take()
            .map(|(request_id, result)| Event::Ports { request_id, result });
        let connection = self
            .connection
            .take()
            .map(|(activation, result)| Event::Connection { activation, result });
        for event in [ports, connection].into_iter().flatten() {
            match events.try_send(event) {
                Ok(()) => {}
                Err(mpsc::TrySendError::Full(event)) => {
                    self.retain(event);
                    #[cfg(test)]
                    if let Some(signal) = &self.on_full_retain {
                        let _ = signal.try_send(());
                    }
                }
                Err(mpsc::TrySendError::Disconnected(_)) => {
                    *self = Self::default();
                    return;
                }
            }
        }
    }

    pub(crate) fn send(&mut self, events: &SyncSender<Event>, event: Event) {
        self.retain(event);
        self.flush(events);
    }

    pub(crate) fn wait(&self) -> Duration {
        if self.ports.is_some() || self.connection.is_some() {
            RETRY_INTERVAL
        } else {
            IDLE_INTERVAL
        }
    }

    #[cfg(test)]
    pub(crate) fn notify_idle_wait(&self) {
        if let Some(signal) = &self.on_idle_wait {
            let _ = signal.try_send(());
        }
    }
}

pub(crate) fn cancel_queued(requests: &Receiver<Request>) {
    for _ in 0..=COMMAND_CAPACITY {
        match requests.try_recv() {
            Ok(Request::Command(
                Command::Connect { sink, .. }
                | Command::ConnectPorts { sink, .. }
                | Command::ConnectMpe { sink, .. },
            )) => sink.cancel(),
            Ok(_) => {}
            Err(_) => break,
        }
    }
}

/// Owner of the control-side MIDI thread.
///
/// Dropping the worker sets the stop flag and queues a shutdown wake, so an idle worker
/// exits without waiting for its idle interval. When the command queue is full the worker
/// is already draining it and observes the flag after the current command.
pub struct Worker {
    handle: Handle,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Worker {
    /// Starts an idle worker; discovery/native access requires an explicit command.
    pub fn start() -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (events, results) = mpsc::sync_channel(EVENT_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let thread = std::thread::Builder::new()
            .name("hex-midi".into())
            .spawn(move || native::run(requests, events, stop))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            handle: Handle {
                commands,
                events: Arc::new(std::sync::Mutex::new(results)),
            },
            stopping,
            thread: Some(thread),
        })
    }

    #[cfg(test)]
    fn start_with_idle_signal(on_idle_wait: SyncSender<()>) -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (events, results) = mpsc::sync_channel(EVENT_CAPACITY);
        let stopping = Arc::new(AtomicBool::new(false));
        let stop = Arc::clone(&stopping);
        let thread = std::thread::Builder::new()
            .name("hex-midi".into())
            .spawn(move || native::run_with_idle_signal(requests, events, stop, on_idle_wait))
            .map_err(|error| error.to_string())?;
        Ok(Self {
            handle: Handle {
                commands,
                events: Arc::new(std::sync::Mutex::new(results)),
            },
            stopping,
            thread: Some(thread),
        })
    }

    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        // A full queue is already being drained; the flag remains the fallback.
        let _ = self.handle.commands.try_send(Request::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mpe::MpeZone;

    #[test]
    fn full_or_closed_worker_queue_cancels_connection_sink_immediately() {
        let (commands, requests) = mpsc::sync_channel(1);
        let (_, results) = mpsc::sync_channel(1);
        let handle = Handle {
            commands,
            events: Arc::new(std::sync::Mutex::new(results)),
        };
        handle.send(Command::Discover { request_id: 1 }).unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            handle
                .send(Command::Connect {
                    activation: 2,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
        drop(requests);
        cancelled.store(false, Ordering::Release);
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            handle
                .send(Command::Connect {
                    activation: 3,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
    }

    #[test]
    fn worker_shutdown_cancels_pending_connections_without_native_access() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        let notes = Arc::new(HeldNoteInput::default());
        assert!(
            sender
                .try_send(Request::Command(Command::Connect {
                    activation: 7,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        notes: Arc::clone(&notes),
                        publish: Box::new(|_| {}),
                        note: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false)
                    }
                }))
                .is_ok()
        );
        let (events, _results) = mpsc::sync_channel(4);
        native::run(receiver, events, Arc::new(AtomicBool::new(true)));
        assert!(cancelled.load(Ordering::Acquire));
        assert!(notes.is_cancelled());
    }

    /// Drains events until one matches or the bounded deadline passes.
    fn receive_until(
        results: &Receiver<Event>,
        limit: Duration,
        done: impl Fn(&Event) -> bool,
    ) -> Vec<Event> {
        let deadline = std::time::Instant::now() + limit;
        let mut received = Vec::new();
        while let Some(left) = deadline.checked_duration_since(std::time::Instant::now()) {
            match results.recv_timeout(left) {
                Ok(event) => {
                    let finished = done(&event);
                    received.push(event);
                    if finished {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        received
    }

    #[test]
    fn full_event_queue_retains_discovery_reply_until_drained() {
        let (commands, requests) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (filler, results) = mpsc::sync_channel(1);
        let stop = Arc::new(AtomicBool::new(false));
        let (retained_tx, retained_rx) = mpsc::sync_channel(1);
        let worker = {
            let (events, stop) = (filler.clone(), Arc::clone(&stop));
            std::thread::spawn(move || {
                native::run_with_retain_signal(requests, events, stop, retained_tx)
            })
        };
        filler
            .try_send(Event::Connection {
                activation: 0,
                result: Ok(false),
            })
            .unwrap();
        commands
            .try_send(Request::Command(Command::Discover { request_id: 9 }))
            .unwrap();
        retained_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let received = receive_until(&results, Duration::from_secs(2), |event| {
            matches!(event, Event::Ports { request_id: 9, .. })
        });
        stop.store(true, Ordering::Release);
        let _ = commands.try_send(Request::Shutdown);
        worker.join().unwrap();
        assert!(
            received
                .iter()
                .any(|event| matches!(event, Event::Ports { request_id: 9, .. })),
            "{received:?}"
        );
    }

    #[test]
    fn retained_replies_keep_the_newest_answer_per_kind() {
        let (sender, results) = mpsc::sync_channel(1);
        sender
            .try_send(Event::Connection {
                activation: 0,
                result: Ok(false),
            })
            .unwrap();
        let mut outbox = Outbox::default();
        for event in [
            Event::Ports {
                request_id: 2,
                result: Ok(Vec::new()),
            },
            Event::Ports {
                request_id: 1,
                result: Err("stale".into()),
            },
            Event::Connection {
                activation: 5,
                result: Ok(true),
            },
            Event::Connection {
                activation: 4,
                result: Ok(false),
            },
        ] {
            outbox.send(&sender, event);
        }
        assert_eq!(outbox.wait(), RETRY_INTERVAL);
        let mut delivered = Vec::new();
        for _ in 0..3 {
            delivered.extend(results.try_recv());
            outbox.flush(&sender);
        }
        delivered.extend(results.try_recv());
        assert_eq!(outbox.wait(), IDLE_INTERVAL);
        assert!(matches!(
            delivered.as_slice(),
            [
                Event::Connection { activation: 0, .. },
                Event::Ports {
                    request_id: 2,
                    result: Ok(_)
                },
                Event::Connection {
                    activation: 5,
                    result: Ok(true)
                },
            ]
        ));
    }

    #[test]
    fn dropping_an_idle_worker_returns_promptly() {
        let (idle_tx, idle_rx) = mpsc::sync_channel(1);
        let worker = Worker::start_with_idle_signal(idle_tx).unwrap();
        // Wait deterministically for the worker to reach its idle wait loop.
        idle_rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let started = std::time::Instant::now();
        drop(worker);
        let elapsed = started.elapsed();
        assert!(elapsed < Duration::from_millis(50), "{elapsed:?}");
    }

    #[test]
    fn worker_connect_mpe_rejects_invalid_zone_and_cancels_sink_safely() {
        let worker = Worker::start().unwrap();
        let handle = worker.handle();
        let held = Arc::new(HeldNoteInput::default());
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel_flag = Arc::clone(&cancelled);
        let check_flag = Arc::clone(&cancelled);
        let sink = Sink {
            publish: Box::new(|_| {}),
            note: Box::new(|_| {}),
            notes: Arc::clone(&held),
            cancel: Box::new(move || cancel_flag.store(true, Ordering::SeqCst)),
            is_cancelled: Box::new(move || check_flag.load(Ordering::SeqCst)),
        };
        handle
            .send(Command::ConnectMpe {
                activation: 9,
                ports: vec![Port {
                    id: u32::MAX,
                    name: "synthetic".into(),
                }],
                config: MpeConfig {
                    zone: MpeZone::Lower { member_channels: 0 },
                    pitch_bend_range_semitones: 48,
                    master_pitch_bend_range_semitones: 2,
                },
                sink,
            })
            .unwrap();
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        let event = loop {
            match handle.try_event() {
                Ok(event) => break event,
                Err(TryRecvError::Empty) if std::time::Instant::now() < deadline => {
                    std::thread::yield_now();
                }
                Err(err) => panic!("expected connection error event: {err:?}"),
            }
        };
        match event {
            Event::Connection {
                activation: 9,
                result: Err(reason),
            } => {
                assert!(
                    reason.contains("invalid") || reason.contains("macos"),
                    "unexpected error: {reason}"
                );
            }
            other => panic!("unexpected worker event: {other:?}"),
        }
        assert!(held.is_cancelled());
        assert!(cancelled.load(Ordering::SeqCst));
    }
}
