//! Verified-file playback and original callback retirement for sparse EQ edits.
use super::*;
use hex_devices::eq::{EqState, Node, NodeId, NodeParameter, OUTPUT_GAIN};
use hex_engine::{
    AudioCandidateEffects, AudioSourceContext, MediaPages, patch_native_device_parameter,
};
use hex_lattice_contract::ParameterValue;
use hex_project::{Command, Created, Editor};
use std::time::{Duration, Instant};

#[test]
fn sparse_eq_streamed_edits_install_and_undo_without_callback_allocations() {
    let _serial = serial();
    if !hex_engine::audio_storage_constructor_layout_supported() {
        return;
    }
    let frames = 4096;
    let mut editor = Editor::new(readiness::project_with_frames(frames), 8).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    let mut eq = EqState::default();
    let node = NodeId::new(9).unwrap();
    eq.add(Node::new(node, 1100.0, 0.0, 0).unwrap()).unwrap();
    let original = eq.encode().unwrap();
    let Created::Device(device) = editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: original.clone(),
        })
        .unwrap()
    else {
        panic!("sparse EQ device")
    };
    let changed = patch_native_device_parameter(&original, OUTPUT_GAIN, ParameterValue::Float(0.5))
        .and_then(|state| {
            patch_native_device_parameter(
                &state,
                node.parameter(NodeParameter::Frequency).unwrap(),
                ParameterValue::Float(620.0),
            )
        })
        .unwrap();

    let wav = stream_fixture::Wav::with_frames(frames as u32);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let (stream_control, mut pages) = stream_fixture::pair(definition);
    pages.request_page(metadata.key(), 0);
    let deadline = Instant::now() + Duration::from_secs(2);
    while pages.page(metadata.key(), 0).is_none() {
        pages.pump_ready(2);
        assert!(Instant::now() < deadline, "first verified page");
        std::thread::yield_now();
    }
    let (mut control, mut data, errors, mut retirement) = anchored();
    let quote = AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    control
        .configure_audio_candidate_publication(crate::AudioCandidatePublicationReservation {
            runtime_slots: 3,
            archive_slots: 3,
            member_rows: quote.member_rows * 3,
            transfer_rows: quote.transfer_rows * 3,
            receipt_rows: 4,
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows * 3,
            max_retained_data_bytes: storage.max_prepared_data_bytes * 3,
            max_construction_peak_data_bytes: storage.max_construction_data_bytes * 3,
        })
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .register_audio_candidate_domain_with_readiness(
            (stream_control, pages),
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let mut prior = None;
    for (revision, gain) in [(1, 1.0), (2, 0.5), (3, 1.0)] {
        if revision == 2 {
            editor
                .execute(Command::ReplaceDeviceState {
                    device,
                    state: changed.clone(),
                })
                .unwrap();
        } else if revision == 3 {
            assert!(editor.undo());
            assert_eq!(editor.project().device(device).unwrap().state(), &original);
        }
        let context = AudioSourceContext {
            project_generation: 7,
            editor_revision: revision,
        };
        let preparation = stream_fixture::prepare_candidate(
            &mut control,
            editor.project(),
            &metadata,
            domain,
            context,
        );
        let ticket = control
            .stage_audio_candidate_preparation(preparation, context.project_generation)
            .unwrap();
        if revision == 1 {
            control.command(PlaybackCommand::Play).unwrap();
        }
        let mut output = [[0.0; 2]; 128];
        // Leave room for the existing EQ gain smoother to settle after each
        // handoff. The measurement still covers the first block that installs it.
        for _ in 0..7 {
            let allocation = allocation_counter::measure(|| {
                data.owner_mut().unwrap().run(|callback, _| {
                    callback.begin_block();
                    callback.render(&mut output).unwrap();
                });
            });
            assert_eq!(
                (
                    allocation.count_total,
                    allocation.bytes_total,
                    allocation.count_current,
                    allocation.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        assert_eq!(
            control.status.applied_audio_source().unwrap().context,
            context
        );
        assert!(
            output
                .iter()
                .flatten()
                .all(|&sample| (sample - 0.25 * gain).abs() < 1e-5),
            "revision {revision}: expected settled gain {gain}, first frame {:?}",
            output[0],
        );
        if let Some(previous) = prior.replace(ticket) {
            let returned = recover(&mut control, &previous);
            assert_eq!(
                returned.return_kind,
                crate::AudioCandidateReturnKind::Replaced
            );
        }
    }
    crate::lifecycle::test_dispose_backend(
        &mut Some(()),
        &mut false,
        &mut false,
        &mut retirement,
        |_| Ok(()),
    )
    .unwrap();
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let returned = recover(&mut control, &prior.unwrap());
    assert_eq!(
        returned.return_kind,
        crate::AudioCandidateReturnKind::FinalActive
    );
    drop(returned);
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        control.reclaim();
        if let Some(report) = control.recover_audio_candidate_domain(domain) {
            assert!(report.worker_joined && report.accounting_complete);
            break;
        }
        assert!(Instant::now() < deadline, "verified source retirement");
        std::thread::yield_now();
    }
    drop(control);
    finish(&mut retirement);
}
