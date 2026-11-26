//! Platform-specific MIDI worker loop and CoreMIDI port connection.

use crate::worker::{Command, Event, Outbox, Port, Request, Sink, cancel_queued};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender},
};

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use crate::aggregator::{MAX_AGGREGATED_PORTS, MultiPortAggregator};
    use crate::framing::ChannelFilter;
    use crate::held_notes::HeldNoteInput;
    use crate::mpe::MpeConfig;
    use crate::timebase::MachTimebase;
    use coremidi::{Client, InputPort, Notification, Source, Sources};
    use std::thread::{JoinHandle, Thread};

    /// Lock-free wake primitive for the dedicated MIDI packet-pump thread.
    struct PumpWake {
        parked: AtomicBool,
        thread: std::sync::OnceLock<Thread>,
    }

    impl PumpWake {
        fn new() -> Self {
            Self {
                parked: AtomicBool::new(false),
                thread: std::sync::OnceLock::new(),
            }
        }

        fn register_current(&self) {
            let _ = self.thread.set(std::thread::current());
        }

        fn unpark_if_parked(&self) {
            if self.parked.swap(false, Ordering::AcqRel)
                && let Some(thread) = self.thread.get()
            {
                thread.unpark();
            }
        }

        fn force_unpark(&self) {
            self.parked.store(false, Ordering::Release);
            if let Some(thread) = self.thread.get() {
                thread.unpark();
            }
        }
    }

    struct Active {
        activation: u64,
        // Drop input ports and CoreMIDI client before joining the pump thread
        // so all Mach callbacks stop cleanly before teardown completes.
        ports: Option<Vec<InputPort>>,
        client: Option<Client>,
        pump_stop: Arc<AtomicBool>,
        pump_wake: Arc<PumpWake>,
        pump_thread: Option<JoinHandle<()>>,
        setup_changed: Arc<AtomicBool>,
        aggregator: Arc<MultiPortAggregator>,
        sources: Vec<Source>,
        cancel: Box<dyn Fn() + Send>,
        is_cancelled: Box<dyn Fn() -> bool + Send>,
        notes: Arc<HeldNoteInput>,
    }

    impl Drop for Active {
        fn drop(&mut self) {
            self.notes.cancel();
            self.pump_stop.store(true, Ordering::Release);
            // Close CoreMIDI input ports and client first so no Mach callback
            // can still run while the pump thread drains and exits.
            self.ports.take();
            self.client.take();
            self.pump_wake.force_unpark();
            if let Some(thread) = self.pump_thread.take() {
                let _ = thread.join();
            }
            (self.cancel)();
        }
    }

    fn source_by_id(id: u32) -> Option<Source> {
        Sources
            .into_iter()
            .take(128)
            .find(|source| source.unique_id() == Some(id))
    }

    fn discover() -> Result<Vec<Port>, String> {
        if Sources::count() > 127 {
            return Err("controller input exceeds 127 ports".into());
        }
        let result: Vec<_> = Sources
            .into_iter()
            .take(128)
            .map(|source| {
                let id = source
                    .unique_id()
                    .ok_or("controller port identity unavailable")?;
                let name = source
                    .display_name()
                    .ok_or("controller port name unavailable")?;
                if name.len() > 1024 {
                    return Err("controller port name exceeds limit".into());
                }
                Ok(Port { id, name })
            })
            .collect::<Result<_, String>>()?;
        if result.len() > 127 {
            return Err("controller input exceeds 127 ports".into());
        }
        Ok(result)
    }

    enum ConnectMode {
        Channel(ChannelFilter),
        Mpe(MpeConfig),
    }

    fn connect_ports(
        activation: u64,
        ports: Vec<Port>,
        mode: ConnectMode,
        sink: Sink,
        wake_tx: &SyncSender<Request>,
    ) -> Result<Active, String> {
        let setup_changed = Arc::new(AtomicBool::new(false));
        let pump_stop = Arc::new(AtomicBool::new(false));
        let pump_wake = Arc::new(PumpWake::new());
        let result = (|| {
            if sink.notes.is_cancelled() || (sink.is_cancelled)() {
                return Err("controller activation was cancelled".into());
            }
            if ports.is_empty() || ports.len() > MAX_AGGREGATED_PORTS {
                return Err("controller port count must be between 1 and 16".into());
            }
            match mode {
                ConnectMode::Channel(filter) => {
                    if !filter.is_valid() {
                        return Err("controller channel is invalid".into());
                    }
                }
                ConnectMode::Mpe(config) => {
                    if !config.is_valid() {
                        return Err("controller mpe zone configuration is invalid".into());
                    }
                }
            }
            let mut seen_ids = std::collections::HashSet::with_capacity(ports.len());
            let mut sources = Vec::with_capacity(ports.len());
            for port in &ports {
                if !seen_ids.insert(port.id) {
                    return Err("controller port list contains duplicate port ids".into());
                }
                let source = source_by_id(port.id).ok_or("controller port is unavailable")?;
                sources.push(source);
            }
            let notify_flag = Arc::clone(&setup_changed);
            let notify_tx = wake_tx.clone();
            let client = Client::new_with_notifications(
                "hex controller input",
                move |notification: &Notification| {
                    if matches!(
                        notification,
                        Notification::SetupChanged
                            | Notification::ObjectAdded(_)
                            | Notification::ObjectRemoved(_)
                            | Notification::IoError(_)
                    ) {
                        notify_flag.store(true, Ordering::Release);
                        let _ = notify_tx.try_send(Request::SetupChanged);
                    }
                },
            )
            .map_err(|status| format!("controller client failed: {status}"))?;
            let aggregator = match mode {
                ConnectMode::Channel(filter) => MultiPortAggregator::new(
                    sources.len(),
                    filter,
                    MachTimebase::host(),
                    Arc::clone(&sink.notes),
                    sink.publish,
                    sink.note,
                ),
                ConnectMode::Mpe(config) => MultiPortAggregator::new_mpe(
                    sources.len(),
                    config,
                    MachTimebase::host(),
                    Arc::clone(&sink.notes),
                    sink.publish,
                    sink.note,
                ),
            }
            .map(Arc::new)
            .map_err(str::to_owned)?;
            let thread_agg = Arc::clone(&aggregator);
            let thread_stop = Arc::clone(&pump_stop);
            let thread_wake = Arc::clone(&pump_wake);
            let thread_notes = Arc::clone(&sink.notes);
            let pump_thread = std::thread::Builder::new()
                .name("hex-midi-pump".into())
                .spawn(move || {
                    thread_wake.register_current();
                    while !thread_stop.load(Ordering::Acquire) && !thread_notes.is_cancelled() {
                        if thread_agg.drain_queued_ports() > 0 {
                            continue;
                        }
                        thread_wake.parked.store(true, Ordering::Release);
                        if thread_agg.drain_queued_ports() > 0
                            || thread_stop.load(Ordering::Acquire)
                            || thread_notes.is_cancelled()
                        {
                            thread_wake.parked.store(false, Ordering::Release);
                            continue;
                        }
                        std::thread::park();
                        thread_wake.parked.store(false, Ordering::Release);
                    }
                })
                .map_err(|error| format!("controller pump thread failed: {error}"))?;
            let mut inputs = Vec::with_capacity(sources.len());
            for (port_index, source) in sources.iter().enumerate() {
                let Some(queue) = aggregator.port_queue(port_index) else {
                    return Err("controller port queue unavailable".into());
                };
                let wake = Arc::clone(&pump_wake);
                let stop_flag = Arc::clone(&pump_stop);
                let port_name = format!("hex controller input {port_index}");
                let input = client
                    .input_port(&port_name, move |packets| {
                        // Wait-free CoreMIDI Mach callback: zero mutexes, zero
                        // allocations, zero caller closures.
                        if stop_flag.load(Ordering::Acquire) {
                            return;
                        }
                        let mut pushed = false;
                        for packet in packets.iter() {
                            if queue.enqueue_packet(packet.timestamp(), packet.data()) {
                                pushed = true;
                            }
                        }
                        if pushed {
                            wake.unpark_if_parked();
                        }
                    })
                    .map_err(|status| format!("controller port creation failed: {status}"))?;
                input
                    .connect_source(source)
                    .map_err(|status| format!("controller connection failed: {status}"))?;
                inputs.push(input);
            }
            Ok((inputs, client, pump_thread, aggregator, sources))
        })();
        match result {
            Ok((inputs, client, pump_thread, aggregator, sources)) => Ok(Active {
                activation,
                ports: Some(inputs),
                client: Some(client),
                pump_stop,
                pump_wake,
                pump_thread: Some(pump_thread),
                setup_changed,
                aggregator,
                sources,
                cancel: sink.cancel,
                is_cancelled: sink.is_cancelled,
                notes: sink.notes,
            }),
            Err(error) => {
                // publish/note may have moved into the aggregator; stop any pump
                // thread cleanly before cancelling remaining owners.
                pump_stop.store(true, Ordering::Release);
                pump_wake.force_unpark();
                sink.notes.cancel();
                (sink.cancel)();
                Err(error)
            }
        }
    }

    pub(super) fn run_with_outbox(
        requests: Receiver<Request>,
        wake_tx: SyncSender<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        mut outbox: Outbox,
    ) {
        let mut active: Option<Active> = None;
        while !stop.load(Ordering::Acquire) {
            outbox.flush(&events);
            #[cfg(test)]
            outbox.notify_idle_wait();
            let mut check_presence = false;
            match requests.recv_timeout(outbox.wait()) {
                Ok(Request::Shutdown) => break,
                Ok(Request::SetupChanged) => {
                    check_presence = true;
                }
                Ok(Request::Command(Command::Discover { request_id })) => {
                    outbox.send(
                        &events,
                        Event::Ports {
                            request_id,
                            result: discover(),
                        },
                    );
                }
                Ok(Request::Command(Command::Connect {
                    activation,
                    port,
                    channel,
                    sink,
                })) => {
                    active.take();
                    let result = match connect_ports(
                        activation,
                        vec![port],
                        ConnectMode::Channel(ChannelFilter::Single(channel)),
                        sink,
                        &wake_tx,
                    ) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    outbox.send(&events, Event::Connection { activation, result });
                }
                Ok(Request::Command(Command::ConnectPorts {
                    activation,
                    ports,
                    channel_filter,
                    sink,
                })) => {
                    active.take();
                    let result = match connect_ports(
                        activation,
                        ports,
                        ConnectMode::Channel(channel_filter),
                        sink,
                        &wake_tx,
                    ) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    outbox.send(&events, Event::Connection { activation, result });
                }
                Ok(Request::Command(Command::ConnectMpe {
                    activation,
                    ports,
                    config,
                    sink,
                })) => {
                    active.take();
                    let result = match connect_ports(
                        activation,
                        ports,
                        ConnectMode::Mpe(config),
                        sink,
                        &wake_tx,
                    ) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    outbox.send(&events, Event::Connection { activation, result });
                }
                Ok(Request::Command(Command::SetMpePitchBendRanges {
                    activation,
                    member_semitones,
                    master_semitones,
                })) => {
                    if let Some(connection) = active
                        .as_ref()
                        .filter(|active| active.activation == activation)
                    {
                        let _ = connection
                            .aggregator
                            .set_mpe_pitch_bend_ranges(member_semitones, master_semitones);
                    }
                }
                Ok(Request::Command(Command::Disconnect { activation })) => {
                    if active
                        .as_ref()
                        .is_some_and(|active| active.activation == activation)
                    {
                        active.take();
                    }
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Ok(false),
                        },
                    );
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(connection) = &active {
                let setup_notified =
                    check_presence || connection.setup_changed.swap(false, Ordering::AcqRel);
                let present = if setup_notified {
                    // Check endpoint identity/presence only in response to CoreMIDI
                    // setup/object notifications rather than 500 ms polling.
                    connection.sources.iter().all(|connected| {
                        connected
                            .unique_id()
                            .and_then(source_by_id)
                            .is_some_and(|source| source == *connected)
                    })
                } else {
                    true
                };
                if !present || connection.notes.is_cancelled() || (connection.is_cancelled)() {
                    let activation = connection.activation;
                    active.take();
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Err("controller input disconnected".into()),
                        },
                    );
                }
            }
        }
        active.take();
        cancel_queued(&requests);
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(super) fn run_with_outbox(
        requests: Receiver<Request>,
        _wake_tx: SyncSender<Request>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
        mut outbox: Outbox,
    ) {
        while !stop.load(Ordering::Acquire) {
            outbox.flush(&events);
            #[cfg(test)]
            outbox.notify_idle_wait();
            match requests.recv_timeout(outbox.wait()) {
                Ok(Request::Shutdown) => break,
                Ok(Request::Command(
                    Command::Connect {
                        activation, sink, ..
                    }
                    | Command::ConnectPorts {
                        activation, sink, ..
                    }
                    | Command::ConnectMpe {
                        activation, sink, ..
                    },
                )) => {
                    sink.cancel();
                    outbox.send(
                        &events,
                        Event::Connection {
                            activation,
                            result: Err("controller input currently supports macos only".into()),
                        },
                    );
                }
                Ok(Request::Command(Command::Discover { request_id })) => {
                    outbox.send(
                        &events,
                        Event::Ports {
                            request_id,
                            result: Err("controller input currently supports macos only".into()),
                        },
                    );
                }
                Ok(Request::Command(
                    Command::Disconnect { .. } | Command::SetMpePitchBendRanges { .. },
                )) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        cancel_queued(&requests);
    }
}

pub(crate) fn run(
    requests: Receiver<Request>,
    wake_tx: SyncSender<Request>,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
) {
    platform::run_with_outbox(requests, wake_tx, events, stop, Outbox::default());
}

#[cfg(test)]
pub(crate) fn run_with_retain_signal(
    requests: Receiver<Request>,
    wake_tx: SyncSender<Request>,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    on_full_retain: SyncSender<()>,
) {
    platform::run_with_outbox(
        requests,
        wake_tx,
        events,
        stop,
        Outbox::with_full_retain(on_full_retain),
    );
}

#[cfg(test)]
pub(crate) fn run_with_idle_signal(
    requests: Receiver<Request>,
    wake_tx: SyncSender<Request>,
    events: SyncSender<Event>,
    stop: Arc<AtomicBool>,
    on_idle_wait: SyncSender<()>,
) {
    platform::run_with_outbox(
        requests,
        wake_tx,
        events,
        stop,
        Outbox::with_idle_wait(on_idle_wait),
    );
}
