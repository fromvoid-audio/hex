//! Platform-specific hardware MIDI output backend (CoreMIDI destinations on
//! macOS; explicit unsupported refusal on non-macOS targets).

use crate::output::{
    EncodedMidiMessage, MAX_OUTPUT_PORT_NAME_BYTES, MAX_OUTPUT_PORTS, OutputBackend, OutputPort,
    OutputSendError,
};
use std::sync::Arc;

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use crate::timebase::MachTimebase;
    use coremidi::{
        Client, Destination, Destinations, Notification, OutputPort as CoreMidiOutputPort,
        PacketBuffer,
    };

    pub(crate) struct CoreMidiEndpoint {
        port_id: u32,
        // Drop output port before CoreMIDI client.
        output_port: CoreMidiOutputPort,
        _client: Client,
        destination: Destination,
        timebase: MachTimebase,
    }

    pub(crate) struct PlatformOutputBackend {
        notifier: Option<Arc<dyn Fn() + Send + Sync>>,
    }

    impl PlatformOutputBackend {
        pub(crate) fn new() -> Self {
            Self { notifier: None }
        }
    }

    fn destination_by_id(id: u32) -> Option<Destination> {
        Destinations
            .into_iter()
            .take(MAX_OUTPUT_PORTS + 1)
            .find(|destination| destination.unique_id() == Some(id))
    }

    impl OutputBackend for PlatformOutputBackend {
        type Endpoint = CoreMidiEndpoint;

        fn install_setup_notifier(&mut self, notify: Arc<dyn Fn() + Send + Sync>) {
            self.notifier = Some(notify);
        }

        fn discover(&mut self) -> Result<Vec<OutputPort>, String> {
            if Destinations::count() > MAX_OUTPUT_PORTS {
                return Err("midi output exceeds 127 ports".into());
            }
            let result: Vec<_> = Destinations
                .into_iter()
                .take(MAX_OUTPUT_PORTS + 1)
                .map(|destination| {
                    let id = destination
                        .unique_id()
                        .ok_or("midi output port identity unavailable")?;
                    let name = destination
                        .display_name()
                        .ok_or("midi output port name unavailable")?;
                    if name.is_empty() || name.len() > MAX_OUTPUT_PORT_NAME_BYTES {
                        return Err("midi output port name exceeds limit".into());
                    }
                    Ok(OutputPort { id, name })
                })
                .collect::<Result<_, String>>()?;
            if result.len() > MAX_OUTPUT_PORTS {
                return Err("midi output exceeds 127 ports".into());
            }
            Ok(result)
        }

        fn connect(&mut self, port: &OutputPort) -> Result<Self::Endpoint, String> {
            let destination =
                destination_by_id(port.id).ok_or("midi output port is unavailable")?;
            let notifier = self.notifier.clone();
            let client = Client::new_with_notifications(
                "hex midi output",
                move |notification: &Notification| {
                    if matches!(
                        notification,
                        Notification::SetupChanged
                            | Notification::ObjectAdded(_)
                            | Notification::ObjectRemoved(_)
                            | Notification::IoError(_)
                    ) && let Some(notify) = notifier.as_ref()
                    {
                        notify();
                    }
                },
            )
            .map_err(|status| format!("midi output client failed: {status}"))?;
            let output_port = client
                .output_port("hex midi output")
                .map_err(|status| format!("midi output port creation failed: {status}"))?;
            Ok(CoreMidiEndpoint {
                port_id: port.id,
                output_port,
                _client: client,
                destination,
                timebase: MachTimebase::host(),
            })
        }

        fn is_present(&mut self, endpoint: &Self::Endpoint) -> bool {
            endpoint
                .destination
                .unique_id()
                .filter(|&id| id == endpoint.port_id)
                .and_then(destination_by_id)
                .is_some_and(|current| current == endpoint.destination)
        }

        fn send(
            &mut self,
            endpoint: &mut Self::Endpoint,
            timestamp_ns: u64,
            message: EncodedMidiMessage,
        ) -> Result<(), OutputSendError> {
            let ticks = if timestamp_ns == 0 {
                0
            } else {
                endpoint.timebase.nanos_to_ticks_saturating(timestamp_ns)
            };
            let packets = PacketBuffer::new(ticks, message.as_slice());
            endpoint
                .output_port
                .send(&endpoint.destination, &packets)
                .map_err(|status| {
                    if !self.is_present(endpoint) {
                        OutputSendError::Disconnected(format!("midi output disconnected: {status}"))
                    } else {
                        OutputSendError::Transient(format!("midi output send failed: {status}"))
                    }
                })
        }

        fn close(&mut self, endpoint: Self::Endpoint) {
            drop(endpoint);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub(crate) struct UnsupportedEndpoint;

    pub(crate) struct PlatformOutputBackend;

    impl PlatformOutputBackend {
        pub(crate) fn new() -> Self {
            Self
        }
    }

    impl OutputBackend for PlatformOutputBackend {
        type Endpoint = UnsupportedEndpoint;

        fn discover(&mut self) -> Result<Vec<OutputPort>, String> {
            Err("hardware midi output currently supports macos only".into())
        }

        fn connect(&mut self, _port: &OutputPort) -> Result<Self::Endpoint, String> {
            Err("hardware midi output currently supports macos only".into())
        }

        fn is_present(&mut self, _endpoint: &Self::Endpoint) -> bool {
            false
        }

        fn send(
            &mut self,
            _endpoint: &mut Self::Endpoint,
            _timestamp_ns: u64,
            _message: EncodedMidiMessage,
        ) -> Result<(), OutputSendError> {
            Err(OutputSendError::Disconnected(
                "hardware midi output currently supports macos only".into(),
            ))
        }

        fn close(&mut self, _endpoint: Self::Endpoint) {}
    }
}

pub(crate) use platform::PlatformOutputBackend;
