//! Optional, control-owned MIDI controller input. Hardware notes are ignored.
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, SyncSender, TryRecvError},
};
use std::thread::JoinHandle;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Controllers {
    pub pressure: u8,
    pub bend: i16,
    pub wheel: u8,
}
/// Exact channel-voice messages only; malformed input never changes the snapshot.
pub fn parse(message: &[u8], channel: u8, current: Controllers) -> Option<Controllers> {
    if !(2..=3).contains(&message.len()) {
        return None;
    }
    let (&status, data) = message.split_first()?;
    if channel >= 16 || status & 15 != channel || data.iter().any(|value| *value > 127) {
        return None;
    }
    let mut next = current;
    match (status & 0xf0, data) {
        (0xd0, [pressure]) => next.pressure = *pressure,
        (0xe0, [low, high]) => next.bend = (i16::from(*high) * 128 + i16::from(*low)) - 8192,
        (0xb0, [1, wheel]) => next.wheel = *wheel,
        (0xb0, [121, _]) => next = Controllers::default(),
        _ => return None,
    }
    Some(next)
}
/// Fixed MIDI 1 byte-stream decoder, including running status across packets.
/// Realtime bytes may interleave anywhere. SysEx/system-common cancel channel
/// running status; unsupported channel messages are consumed at their exact width.
#[derive(Default)]
pub struct Decoder {
    status: u8,
    data: [u8; 2],
    used: usize,
    sysex: bool,
}
impl Decoder {
    pub fn byte(&mut self, byte: u8, channel: u8, current: Controllers) -> Option<Controllers> {
        if byte >= 0xf8 {
            return None;
        }
        if byte >= 0x80 {
            self.used = 0;
            if byte >= 0xf0 {
                self.status = 0;
                self.sysex = byte == 0xf0;
            } else {
                self.status = byte;
                self.sysex = false;
            }
            return None;
        }
        if self.sysex || self.status == 0 {
            return None;
        }
        let width = if matches!(self.status & 0xf0, 0xc0 | 0xd0) {
            1
        } else {
            2
        };
        self.data[self.used] = byte;
        self.used += 1;
        if self.used < width {
            return None;
        }
        self.used = 0;
        let message = [self.status, self.data[0], self.data[1]];
        parse(&message[..width + 1], channel, current)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Port {
    pub id: u32,
    pub name: String,
}
/// Constructed on control; the native callback only invokes this bounded sink.
pub struct Sink {
    pub publish: Box<dyn Fn(Controllers) + Send>,
    pub cancel: Box<dyn Fn() + Send>,
    pub is_cancelled: Box<dyn Fn() -> bool + Send>,
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
#[derive(Clone)]
pub struct Handle {
    commands: SyncSender<Command>,
    events: Arc<std::sync::Mutex<Receiver<Event>>>,
}
impl Handle {
    pub fn send(&self, command: Command) -> Result<(), &'static str> {
        match self.commands.try_send(command) {
            Ok(()) => Ok(()),
            Err(error) => {
                if let mpsc::TrySendError::Full(Command::Connect { sink, .. })
                | mpsc::TrySendError::Disconnected(Command::Connect { sink, .. }) = error
                {
                    (sink.cancel)();
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
fn cancel_queued(requests: &Receiver<Command>) {
    for _ in 0..4 {
        match requests.try_recv() {
            Ok(Command::Connect { sink, .. }) => (sink.cancel)(),
            Ok(_) => {}
            Err(_) => break,
        }
    }
}
pub struct Worker {
    handle: Handle,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}
impl Worker {
    /// Starts an idle worker; discovery/native access requires an explicit command.
    pub fn start() -> Result<Self, String> {
        let (commands, requests) = mpsc::sync_channel(4);
        let (events, results) = mpsc::sync_channel(4);
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
    pub fn handle(&self) -> Handle {
        self.handle.clone()
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(target_os = "macos")]
mod native {
    use super::*;
    use coremidi::{Client, InputPort, Source, Sources};
    use std::time::Duration;
    struct Active {
        // Rust drops fields in declaration order: input port closes before client.
        activation: u64,
        _port: InputPort,
        _client: Client,
        source: Source,
        cancel: Box<dyn Fn() + Send>,
    }
    impl Drop for Active {
        fn drop(&mut self) {
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
    fn connect(activation: u64, port: Port, channel: u8, sink: Sink) -> Result<Active, String> {
        let result = (|| {
            if (sink.is_cancelled)() {
                return Err("controller activation was cancelled".into());
            }
            if channel >= 16 {
                return Err("controller channel is invalid".into());
            }
            let source = source_by_id(port.id).ok_or("controller port is unavailable")?;
            let client = Client::new("hex controller input")
                .map_err(|status| format!("controller client failed: {status}"))?;
            let mut decoder = Decoder::default();
            let mut current = Controllers::default();
            let publish = sink.publish;
            let input = client
                .input_port("hex controller input", move |packets| {
                    let mut remaining = 65_536_usize;
                    for (packet_index, packet) in packets.iter().take(1024).enumerate() {
                        let bytes = packet.data();
                        for byte in bytes.iter().take(remaining) {
                            if let Some(next) = decoder.byte(*byte, channel, current) {
                                current = next;
                                publish(current);
                            }
                        }
                        if bytes.len() >= remaining {
                            decoder = Decoder::default();
                            break;
                        }
                        remaining -= bytes.len();
                        if packet_index == 1023 {
                            decoder = Decoder::default();
                        }
                    }
                })
                .map_err(|status| format!("controller port creation failed: {status}"))?;
            input
                .connect_source(&source)
                .map_err(|status| format!("controller connection failed: {status}"))?;
            Ok((input, client, source))
        })();
        match result {
            Ok((input, client, source)) => Ok(Active {
                activation,
                _port: input,
                _client: client,
                source,
                cancel: sink.cancel,
            }),
            Err(error) => {
                (sink.cancel)();
                Err(error)
            }
        }
    }
    pub(super) fn run(
        requests: Receiver<Command>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
    ) {
        let mut active: Option<Active> = None;
        while !stop.load(Ordering::Acquire) {
            match requests.recv_timeout(Duration::from_millis(500)) {
                Ok(Command::Discover { request_id }) => {
                    let _ = events.try_send(Event::Ports {
                        request_id,
                        result: discover(),
                    });
                }
                Ok(Command::Connect {
                    activation,
                    port,
                    channel,
                    sink,
                }) => {
                    active.take();
                    let result = match connect(activation, port, channel, sink) {
                        Ok(connection) => {
                            active = Some(connection);
                            Ok(true)
                        }
                        Err(error) => Err(error),
                    };
                    if events
                        .try_send(Event::Connection { activation, result })
                        .is_err()
                    {
                        active.take();
                    }
                }
                Ok(Command::Disconnect { activation }) => {
                    if active
                        .as_ref()
                        .is_some_and(|active| active.activation == activation)
                    {
                        active.take();
                    }
                    let _ = events.try_send(Event::Connection {
                        activation,
                        result: Ok(false),
                    });
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            if let Some(connection) = &active {
                // Check endpoint identity/presence, never infer loss from silence.
                let present = connection
                    .source
                    .unique_id()
                    .and_then(source_by_id)
                    .is_some_and(|source| source == connection.source);
                if !present {
                    let activation = connection.activation;
                    active.take();
                    let _ = events.try_send(Event::Connection {
                        activation,
                        result: Err("controller input disconnected".into()),
                    });
                }
            }
        }
        active.take();
        cancel_queued(&requests);
    }
}
#[cfg(not(target_os = "macos"))]
mod native {
    use super::*;
    pub(super) fn run(
        requests: Receiver<Command>,
        events: SyncSender<Event>,
        stop: Arc<AtomicBool>,
    ) {
        while !stop.load(Ordering::Acquire) {
            match requests.recv_timeout(std::time::Duration::from_millis(500)) {
                Ok(Command::Connect {
                    activation, sink, ..
                }) => {
                    (sink.cancel)();
                    let _ = events.try_send(Event::Connection {
                        activation,
                        result: Err("controller input currently supports macos only".into()),
                    });
                }
                Ok(Command::Discover { request_id }) => {
                    let _ = events.try_send(Event::Ports {
                        request_id,
                        result: Err("controller input currently supports macos only".into()),
                    });
                }
                Ok(Command::Disconnect { .. }) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
        cancel_queued(&requests);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_messages_channel_endpoints_and_reset() {
        let initial = Controllers::default();
        let pressure = parse(&[0xd2, 127], 2, initial).unwrap();
        assert_eq!(pressure.pressure, 127);
        assert_eq!(parse(&[0xe2, 0, 0], 2, pressure).unwrap().bend, -8192);
        assert_eq!(parse(&[0xe2, 127, 127], 2, pressure).unwrap().bend, 8191);
        assert_eq!(parse(&[0xe2, 0, 64], 2, pressure).unwrap().bend, 0);
        let wheel = parse(&[0xb2, 1, 127], 2, pressure).unwrap();
        assert_eq!(wheel.wheel, 127);
        assert_eq!(parse(&[0xb2, 121, 0], 2, wheel), Some(initial));
        for malformed in [
            &[0xd2][..],
            &[0xd2, 128],
            &[0xd2, 1, 2],
            &[0xe2, 0],
            &[0xb2, 1],
            &[0x92, 60, 127],
            &[0xb2, 64, 127],
            &[0xd1, 127],
            &[0xf8],
            &[0xf0, 1, 0xf7],
        ] {
            assert_eq!(parse(malformed, 2, wheel), None, "{malformed:?}");
        }
        assert_eq!(parse(&[0xd0, 1], 16, wheel), None);
    }
    #[test]
    fn flood_coalesces_to_latest_without_message_storage() {
        let mut state = Controllers::default();
        for index in 0..100_000 {
            state = parse(&[0xb0, 1, (index % 128) as u8], 0, state).unwrap();
        }
        assert_eq!(state.wheel, (99_999 % 128) as u8);
    }
    #[test]
    fn decoder_keeps_running_status_across_packets_and_skips_realtime_notes_and_sysex() {
        let mut decoder = Decoder::default();
        let mut state = Controllers::default();
        let mut outputs = Vec::new();
        for packet in [
            &[0xb0, 1][..],
            &[0xf8, 100, 1, 20, 0xd0, 70, 0xfa, 90],
            &[0x90, 60, 100, 61, 110, 0xb0, 1, 30],
            &[0xf0, 0xd, 1, 0xf8, 30, 0xf7, 1, 100, 0xe0, 0, 64],
        ] {
            for byte in packet {
                if let Some(next) = decoder.byte(*byte, 0, state) {
                    state = next;
                    outputs.push(state);
                }
            }
        }
        assert_eq!(outputs.len(), 6);
        assert_eq!(
            outputs.iter().map(|state| state.wheel).collect::<Vec<_>>(),
            [100, 20, 20, 20, 30, 30]
        );
        assert_eq!(
            state,
            Controllers {
                pressure: 90,
                bend: 0,
                wheel: 30
            }
        );
        decoder.byte(0xb0, 0, state);
        decoder.byte(1, 0, state);
        decoder.byte(0xf2, 0, state); // system-common cancels partial running status
        assert_eq!(decoder.byte(100, 0, state), None);
        assert_eq!(decoder.byte(1, 0, state), None);
        assert_eq!(decoder.byte(100, 0, state), None);
    }

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
                        publish: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
        drop(requests);
        cancelled.store(false, Ordering::Release);
        let flag = Arc::clone(&cancelled);
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
                        publish: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false),
                    }
                })
                .is_err()
        );
        assert!(cancelled.load(Ordering::Acquire));
    }
    #[test]
    fn worker_shutdown_cancels_pending_connections_without_native_access() {
        let (sender, receiver) = mpsc::sync_channel(4);
        let cancelled = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&cancelled);
        assert!(
            sender
                .try_send(Command::Connect {
                    activation: 7,
                    port: Port {
                        id: 1,
                        name: "fake".into()
                    },
                    channel: 0,
                    sink: Sink {
                        publish: Box::new(|_| {}),
                        cancel: Box::new(move || flag.store(true, Ordering::Release)),
                        is_cancelled: Box::new(|| false)
                    }
                })
                .is_ok()
        );
        let (events, _results) = mpsc::sync_channel(4);
        native::run(receiver, events, Arc::new(AtomicBool::new(true)));
        assert!(cancelled.load(Ordering::Acquire));
    }
}
