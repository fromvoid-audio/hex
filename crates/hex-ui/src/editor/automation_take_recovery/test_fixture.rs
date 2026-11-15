use super::{DraftProvenance, Scope};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{Command, Created};
use hex_session::{Session, service::automation_draft::Vst3CaptureTarget};

pub(super) fn plugin_scope() -> Scope {
    let mut session = Session::new(48_000, 4, 4096).unwrap();
    let Created::Device(device) = session
        .command(Command::InsertMainDevice {
            before: None,
            state: hex_engine::default_native_device_state(DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
        })
        .unwrap()
    else {
        panic!("fixture did not create a device");
    };
    Scope {
        target: hex_session::service::DeviceCaptureTarget::Vst3(Vst3CaptureTarget {
            device,
            class: [9; 16],
            parameter: 0,
            binding: hex_lattice_contract::ForeignProcessorBinding {
                root: 7,
                instance: 2,
                graph: 3,
                epoch: 4,
            },
        }),
        stamp: DraftProvenance {
            sample_rate: 48_000,
            output_identity: 7,
            project_generation: 0,
            editor_revision: 1,
            audio_revision: 2,
            transport_epoch: 3,
            gesture_serial: 4,
        },
    }
}
