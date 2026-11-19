use super::ordinary::measured_render;
use super::*;
use hex_engine::AudioSourceContext;
use hex_project::{Command, Editor, Tempo};
use std::time::{Duration, Instant};

#[test]
fn verified_tempo_cut_warms_future_pages_and_installs_in_the_callback_tail() {
    verify_cut(Outcome::Install);
}

#[test]
fn verified_tempo_cut_misses_once_when_the_future_page_is_unavailable() {
    verify_cut(Outcome::Miss);
}

#[test]
fn verified_tempo_cut_can_cancel_after_warming_future_pages() {
    verify_cut(Outcome::Cancel);
}

#[test]
fn verified_tempo_cut_yields_to_priority_stop() {
    verify_cut(Outcome::Stop);
}

#[derive(Clone, Copy)]
enum Outcome {
    Install,
    Miss,
    Cancel,
    Stop,
}

fn verify_cut(outcome: Outcome) {
    let _serial = serial();
    let frames = 16_384;
    let mut editor = Editor::new(readiness::project_with_frames(frames), 8).unwrap();
    let track = editor.project().tracks()[0].id();
    let asset = editor.project().assets().next().unwrap().id();
    editor
        .execute(Command::SetLoopRegion {
            start: 6000,
            end: 6300,
            enabled: true,
        })
        .unwrap();
    editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: hex_engine::default_native_device_state_for_class(
                hex_engine::NativeDeviceClass::Effect,
                hex_engine::DeviceTypeId::from_static("hex.limiter"),
            )
            .unwrap(),
        })
        .unwrap();
    let wav = stream_fixture::Wav::with_samples(frames.try_into().unwrap(), |_| 8192);
    let definition = wav.definition(asset, 11);
    let metadata = definition.metadata.clone();
    let (gate, entered, release) = hex_stream::test_support::PageGate::new(hex_stream::PageKey {
        source: metadata.key(),
        page_index: 3,
    });
    let context = AudioSourceContext {
        project_generation: 7,
        editor_revision: 3,
    };
    let mut first =
        stream_fixture::owner_with_effects(editor.project(), metadata.clone(), Some(context));
    let (mut control, mut data, errors, mut retirement) = anchored();
    let quote = hex_engine::AudioCandidateEffects::with_host_definitions(&[], |effects| {
        Ok(effects.quote(editor.project(), 1).unwrap())
    })
    .unwrap();
    let storage = quote.storage_reservation(0).unwrap();
    let mut limits = reservation(&mut first);
    limits.member_rows += quote.member_rows;
    limits.transfer_rows += quote.transfer_rows;
    limits.handle_rows += quote.handle_rows;
    limits.data_rows = limits.data_rows.max(quote.data_rows);
    limits.max_retained_data_bytes += storage.max_prepared_data_bytes;
    limits.max_construction_peak_data_bytes += storage.max_construction_data_bytes;
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    control.configure_audio_candidate_streaming(3).unwrap();
    let domain = control
        .begin_audio_candidate_domain_preparation_with_gate(
            hex_stream::StreamConfig::with_pool_pages(8),
            vec![definition],
            hex_stream::StreamReadinessReservation {
                extents_per_role: 3,
            },
            gate,
        )
        .unwrap();
    let deadline = Instant::now() + Duration::from_millis(500);
    while control
        .poll_audio_candidate_domain_preparation(domain)
        .unwrap()
        != crate::AudioCandidateDomainPreparationProgress::Ready
    {
        assert!(Instant::now() < deadline, "waiting for catalog");
        std::thread::yield_now();
    }
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let first_ticket = control
        .stage_eligible_audio_candidate(
            first,
            vec![domain],
            PublicationPolicy::InheritRuntime,
            context.project_generation,
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    let mut out = [[0.0; 2]; 16];
    while control.status.applied_audio_source().is_none() {
        measured_render(&mut data, &mut out);
        assert_eq!(control.status.rejected_revision(), None);
        assert!(
            Instant::now() < deadline,
            "waiting for installed predecessor"
        );
        std::thread::yield_now();
    }
    control.command(PlaybackCommand::PlayFrom(6180)).unwrap();
    while out.iter().flatten().any(|sample| *sample <= 0.20) {
        measured_render(&mut data, &mut out);
        assert!(Instant::now() < deadline, "waiting for predecessor");
        std::thread::yield_now();
    }
    let predecessor = control.status.applied_audio_source().unwrap();
    control.reclaim();
    let performance = control.status.performance_frame.load(Acquire);
    editor
        .execute(Command::SetProjectTempo {
            tempo: Tempo::new(60_000_000).unwrap(),
        })
        .unwrap();
    let context = AudioSourceContext {
        editor_revision: 4,
        ..context
    };
    let token = stream_fixture::prepare_candidate(
        &mut control,
        editor.project(),
        &metadata,
        domain,
        context,
    );
    // Place the cut well after the old loop's normal device reset and
    // before its next wrap. This separates the tempo handoff from the wrap.
    let position = control.status.position.load(Acquire);
    let due = performance + (6300 - position) + 280;
    let (_, fence) = control.capture_running_transport_fence().unwrap();
    let (ticket, receipt) = control
        .stage_verified_tempo_cut(token, context, predecessor, fence, due)
        .unwrap();
    measured_render(&mut data, &mut out);
    entered
        .recv_timeout(Duration::from_millis(250))
        .expect("future page requested while predecessor is playing");
    assert_eq!(control.status.applied_audio_source(), Some(predecessor));
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    assert_eq!(
        control.status.performance_frame.load(Acquire),
        performance + 16
    );
    if matches!(outcome, Outcome::Cancel) {
        receipt.cancel();
    }

    // Keep the worker blocked through the cut in the miss case.
    let mut release = Some(release);
    if matches!(outcome, Outcome::Install) {
        release.take().unwrap().send(()).unwrap();
        loop {
            measured_render(&mut data, &mut out[..1]);
            let mut protected = false;
            data.owner_mut().unwrap().run(|callback, _| {
                protected = callback
                    .deferred_eligible
                    .as_ref()
                    .is_some_and(|next| next.test_tempo_window_protected(&callback.domain_pages));
            });
            if protected {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "waiting for warmed candidate pages"
            );
            assert!(control.status.performance_frame.load(Acquire) < due - 16);
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    if matches!(outcome, Outcome::Stop) {
        control.command(PlaybackCommand::Stop).unwrap();
        measured_render(&mut data, &mut out);
    } else {
        let mut crossed_from = control.status.performance_frame.load(Acquire);
        let last_prefix = due - 12;
        while crossed_from < last_prefix {
            let frames = usize::try_from((last_prefix - crossed_from).min(16)).unwrap();
            measured_render(&mut data, &mut out[..frames]);
            crossed_from += frames as u64;
            assert_eq!(control.status.performance_frame.load(Acquire), crossed_from);
            assert_eq!(control.status.applied_audio_source(), Some(predecessor));
        }
        measured_render(&mut data, &mut out);
        assert_eq!(control.status.performance_frame.load(Acquire), due + 4);
    }
    let (expected, source) = match outcome {
        Outcome::Install => (
            PublicationReceiptState::Installed,
            AppliedAudioSource {
                context,
                audio_revision: receipt.revision(),
                ..predecessor
            },
        ),
        Outcome::Miss => (
            PublicationReceiptState::Refused(PlaybackHandoffRejection::TempoCutMissed),
            predecessor,
        ),
        Outcome::Cancel => (PublicationReceiptState::Cancelled, predecessor),
        Outcome::Stop => (
            PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged),
            predecessor,
        ),
    };
    assert_eq!(receipt.state(), expected);
    assert_eq!(control.status.applied_audio_source(), Some(source));
    if !matches!(outcome, Outcome::Stop) {
        assert!(
            out.iter().flatten().all(|sample| *sample > 0.20),
            "PCM at cut: {out:?}"
        );
    }
    let position = control.status.position.load(Acquire);
    if matches!(outcome, Outcome::Install) {
        assert!(
            (12_000..12_600).contains(&position),
            "retimed loop: {position}"
        );
    } else if !matches!(outcome, Outcome::Stop) {
        assert!(
            (6000..6300).contains(&position),
            "original loop: {position}"
        );
    }
    if let Some(release) = release {
        release.send(()).unwrap();
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
    drop(recover(&mut control, &first_ticket));
    drop(recover(&mut control, &ticket));
    assert_eq!(receipt.state(), expected);
    drop(control);
    finish(&mut retirement);
}
