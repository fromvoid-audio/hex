use super::*;
use crate::{
    BorrowedMediaPage, BorrowedScalarPage, Frame, PreparedRevision, ScalarFileIdentity,
    ScalarProjectionIdentity, ScalarSourceKey, SessionPlayback, SessionPlaybackCommand, SineTable,
};
use hex_devices::utility::{UTILITY_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created};
use std::sync::Arc;

struct Pages {
    key: ScalarSourceKey,
    values: [Vec<f32>; 2],
    coverage: [Vec<u64>; 2],
    missing: Option<u64>,
    requests: usize,
}
impl MediaPages for Pages {
    fn page(&mut self, _: crate::StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
        None
    }
    fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}
    fn scalar_page(
        &mut self,
        key: ScalarSourceKey,
        page_index: u64,
    ) -> Option<BorrowedScalarPage<'_>> {
        if key != self.key || self.missing == Some(page_index) {
            return None;
        }
        let index = usize::try_from(page_index).ok()?;
        Some(BorrowedScalarPage {
            key,
            page_index,
            values: self.values.get(index)?,
            coverage: self.coverage.get(index)?,
        })
    }
    fn request_scalar_page(&mut self, _: ScalarSourceKey, _: u64) {
        self.requests += 1;
    }
}
fn source(map: &hex_project::TempoMap) -> ScalarSourceMetadata {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hex_project::visit_automation_projection_identity(map, &mut |bytes| hash.update(bytes));
    ScalarSourceMetadata::new(
        ScalarSourceKey::new(7, 1, 1).unwrap(),
        48000,
        4094,
        4098,
        ScalarProjectionIdentity {
            version: hex_project::AUTOMATION_PROJECTION_VERSION,
            fingerprint: hash.finalize().into(),
        },
        ScalarFileIdentity {
            format_version: 2,
            file_bytes: 100,
            terminal_chain: [1; 32],
        },
    )
    .unwrap()
}
fn fixture(
    target: DeviceAutomationTarget,
) -> (
    crate::signal_graph::tests::Fixture,
    hex_project::DeviceId,
    Arc<PreparedRevision>,
    Pages,
) {
    let mut f = crate::signal_graph::tests::fixture(&[&[]], 8192);
    for audio in f.media.values_mut() {
        *audio = crate::Audio::new(48000, vec![[0.25, -0.5]; 8192]).unwrap();
    }
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertDevice {
            track: f.tracks[0],
            before: None,
            state: UTILITY_DEFINITION.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("device")
    };
    let source = source(f.editor.project().tempo_map());
    let key = source.key();
    let revision = Arc::new(
        PreparedRevision::prepare_with_scalar_automation(
            f.editor.project(),
            &f.media,
            Arc::new(SineTable::new()),
            None,
            ScalarAutomationBinding {
                device,
                target,
                source,
            },
        )
        .unwrap(),
    );
    let mut pages = Pages {
        key,
        values: [vec![0.0; 4096], vec![0.0; 2]],
        coverage: [vec![0; 64], vec![0; 1]],
        missing: None,
        requests: 0,
    };
    pages.coverage[0][63] = (1 << 62) | (1 << 63);
    pages.values[0][4095] = 1.0;
    pages.coverage[1][0] = 1 << 1;
    (f, device, revision, pages)
}
fn seek(runtime: &mut SessionPlayback) {
    runtime.apply(SessionPlaybackCommand::Seek(4094)).unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();
}
#[test]
fn scalar_pages_drive_real_native_provider_and_live_clear_reveals_recorded_values() {
    let (_f, _device, revision, mut pages) =
        fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    assert!(
        revision.contains_streamed_sources(),
        "scalar-only page dependency enters native preflight"
    );
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    let mut demand = Vec::new();
    let window = runtime
        .visit_next_stream_chunk(4, None, &mut |extent| {
            demand.push(extent);
            Ok(())
        })
        .unwrap();
    assert_eq!(window.frames, 4);
    assert_eq!(
        demand
            .iter()
            .map(|extent| (extent.source, extent.page_index, extent.start, extent.end))
            .collect::<Vec<_>>(),
        vec![
            (pages.key.into(), 0, 4094, 4096),
            (pages.key.into(), 1, 0, 2)
        ]
    );
    let mut output = [[9.0; 2]; 4];
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2], "covered zero is a real value");
    assert!(
        output[1][0] > 0.25,
        "normalized maximum reaches the actual provider"
    );
    assert_eq!(
        output[2],
        [0.25, -0.5],
        "uncovered frame restores immutable base"
    );
    assert_eq!(output[3], [0.0; 2]);
    seek(&mut runtime);
    runtime
        .set_native_parameter_input(hex_lattice::NativeTransientOperation::Set(
            hex_lattice::NativeTransientValue::Parameter {
                id: parameter::GAIN,
                value: ParameterValue::Float(2.0),
            },
        ))
        .unwrap();
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output, [[0.5, -1.0]; 4]);
    seek(&mut runtime);
    runtime
        .set_native_parameter_input(hex_lattice::NativeTransientOperation::Clear(
            DeviceAutomationTarget::Parameter(parameter::GAIN),
        ))
        .unwrap();
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(output[0], [0.0; 2]);
    assert_eq!(output[2], [0.25, -0.5]);
}
#[test]
fn missing_cross_page_scalar_refuses_without_advancing_and_retries_same_window() {
    let (_, _, revision, mut pages) = fixture(DeviceAutomationTarget::Parameter(parameter::GAIN));
    let mut runtime = SessionPlayback::new(revision).unwrap();
    seek(&mut runtime);
    pages.missing = Some(1);
    let mut output = [[9.0; 2]; 4];
    assert!(runtime.render_with_pages(&mut output, &mut pages).is_err());
    assert_eq!(runtime.arrangement_frame(), 4094);
    assert_eq!(output, [[0.0; 2]; 4]);
    assert_eq!(pages.requests, 1);
    pages.missing = None;
    runtime.render_with_pages(&mut output, &mut pages).unwrap();
    assert_eq!(runtime.arrangement_frame(), 4098);
    assert_eq!(output[2], [0.25, -0.5]);
}
#[test]
fn scalar_host_stages_use_shared_host_mapping_and_projection_or_latency_changes_refuse() {
    for target in [
        DeviceAutomationTarget::HostInputGain,
        DeviceAutomationTarget::HostOutputGain,
    ] {
        let (mut f, device, revision, mut pages) = fixture(target);
        let mut runtime = SessionPlayback::new(revision).unwrap();
        seek(&mut runtime);
        let mut output = [[9.0; 2]; 4];
        runtime.render_with_pages(&mut output, &mut pages).unwrap();
        assert_eq!(output[0], [0.00025, -0.0005]);
        assert_eq!(output[2], [0.25, -0.5]);
        let binding = ScalarAutomationBinding {
            device,
            target,
            source: source(f.editor.project().tempo_map()),
        };
        f.editor
            .execute(Command::InsertDevice {
                track: f.tracks[0],
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    crate::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
        assert!(
            PreparedRevision::prepare_with_scalar_automation(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                binding.clone()
            )
            .is_err(),
            "compensated processing cannot use an undelayed scalar origin"
        );
        // Current destination-map identity is checked independently of a catalog lease.
        f.editor
            .execute(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(150_000_000).unwrap(),
            })
            .unwrap();
        assert!(
            PreparedRevision::prepare_with_scalar_automation(
                f.editor.project(),
                &f.media,
                Arc::new(SineTable::new()),
                None,
                binding
            )
            .is_err()
        );
    }
}
