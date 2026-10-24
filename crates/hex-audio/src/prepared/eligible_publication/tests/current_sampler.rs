use super::*;
use hex_engine::{AudioCandidatePreparation, AudioCandidateReservationQuote, AudioSourceContext};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::sync::atomic::AtomicBool;
fn fixture() -> (Project, hex_project::AssetId) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 24000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let native =
        hex_engine::native_device_resources::current_sampler_fixture::one_shot_state(4, asset)
            .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track,
            state: native,
        })
        .unwrap();
    // Retain canonical identities in the closed CURRENT Console inventory too.
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "console".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session_clip) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "source".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("session clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session_clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    (editor.project().clone(), asset)
}

#[test]
fn current_sampler_intact_adoption_preserves_context_and_actual_pcm_without_callback_allocation() {
    let _serial = serial();
    let (project, asset) = fixture();
    let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
    let storage = quote.storage_reservation(65536).unwrap();
    let context = AudioSourceContext {
        project_generation: 4,
        editor_revision: 7,
    };
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, storage).unwrap();
    let pcm = hex_engine::Audio::new(
        24000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .unwrap();
    cell.copy_resident_source_on_control(asset, &pcm).unwrap();
    let original = cell.test_source_collection_identity();
    cell.prepare_current_sampler_on_control(&project, 11, context, 4, &AtomicBool::new(false))
        .unwrap();
    let identity = cell.owner_on_control().unwrap().identity();
    let runtime_address = cell
        .owner_on_control()
        .unwrap()
        .test_runtime_address()
        .unwrap();
    let wrong_rate_ledger = hex_engine::AudioStorageLedgerReservation {
        data_rows: quote.data_rows,
        handle_rows: quote.handle_rows,
        root_rows: 2,
        max_retained_bytes: storage.max_prepared_data_bytes,
        max_construction_peak_bytes: storage.max_construction_data_bytes,
    };
    assert!(
        cell.preflight_current_sampler_adoption_on_control(context, 48001, 1, wrong_rate_ledger, 0)
            .is_err()
    );
    let mut cell = Some(cell);
    let limits = crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        receipt_rows: 4,
        member_rows: quote.member_rows * 3,
        transfer_rows: quote.transfer_rows * 3,
        data_rows: quote.data_rows,
        handle_rows: quote.handle_rows * 4,
        max_retained_data_bytes: storage.max_prepared_data_bytes * 3,
        max_construction_peak_data_bytes: storage.max_construction_data_bytes * 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    assert!(
        control.audio_candidate_control_backing_bytes().unwrap()
            <= limits.worker_control_storage_quote().unwrap()
    );
    let token = control
        .reserve_audio_candidate_adoption(crate::AudioCandidatePreparationReservation {
            source_capacity: 1,
            storage,
            member_rows: quote.member_rows,
            transfer_rows: quote.transfer_rows,
            handle_rows: quote.handle_rows,
        })
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let stale = AudioSourceContext {
        editor_revision: 8,
        ..context
    };
    assert!(
        control
            .adopt_audio_candidate_preparation(token, stale, &mut cell)
            .is_err()
    );
    assert_eq!(
        cell.as_mut()
            .unwrap()
            .owner_on_control()
            .unwrap()
            .identity(),
        identity
    );
    assert!(
        control
            .adopt_audio_candidate_preparation(
                token,
                AudioSourceContext {
                    project_generation: context.project_generation + 1,
                    ..context
                },
                &mut cell
            )
            .is_err()
    );
    assert_eq!(
        cell.as_mut()
            .unwrap()
            .owner_on_control()
            .unwrap()
            .identity(),
        identity
    );
    control
        .adopt_audio_candidate_preparation(token, context, &mut cell)
        .unwrap();
    assert!(cell.is_none());
    control
        .prepare_audio_candidate_on_control(token, |cell| {
            assert_eq!(
                cell.owner_on_control().unwrap().test_runtime_address(),
                Some(runtime_address)
            );
            Ok(())
        })
        .unwrap();
    assert!(
        control
            .stage_current_sampler_preparation(token, stale)
            .is_err()
    );
    assert_eq!(control.status.submitted_revision.load(Acquire), 0);
    let ticket = control
        .stage_current_sampler_preparation(token, context)
        .unwrap();
    control
        .command(crate::prepared::PlaybackCommand::Play)
        .unwrap();
    let expected = [
        [0.125, -0.25],
        [0.3125, -0.125],
        [0.5, 0.0],
        [0.1875, 0.125],
        [-0.125, 0.25],
        [-0.0625, 0.125],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
        [0.0, 0.0],
    ];
    let mut output = [[0.0; 2]; 12];
    let counts = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, expected);
    control
        .command(crate::prepared::PlaybackCommand::Stop)
        .unwrap();
    let counts = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, [[0.0; 2]; 12]);
    control
        .command(crate::prepared::PlaybackCommand::Play)
        .unwrap();
    let counts = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, expected);
    let clip = project
        .session_cells()
        .iter()
        .find_map(|cell| match cell.content() {
            hex_project::SessionCellContent::Midi(clip) => Some(clip.id()),
            _ => None,
        })
        .unwrap();
    control
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    let counts = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        })
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        output, expected,
        "canonical Console source plays through the admitted graph"
    );
    control.request_shutdown();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    assert!(
        !crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    let recovered = recover(&mut control, &ticket);
    assert_eq!(recovered.disposed.identity(), identity);
    let bank = recovered.inputs.test_bank_identity();
    assert_eq!((bank.0, bank.1), original);
    assert_eq!(
        recovered
            .inputs
            .source_view(&recovered.inputs.source_token(asset).unwrap())
            .unwrap()
            .sample_rate(),
        24000
    );
    drop(recovered);
    finish(&mut retirement);
}

#[test]
fn current_sampler_adoption_source_capacity_and_actual_data_caps_refuse_without_taking_cell() {
    let _serial = serial();
    for source_limited in [true, false] {
        let (project, asset) = fixture();
        let quote = AudioCandidateReservationQuote::for_project(&project, 1).unwrap();
        let storage = quote.storage_reservation(65536).unwrap();
        let context = AudioSourceContext {
            project_generation: 4,
            editor_revision: 7,
        };
        let mut prepared = AudioCandidatePreparation::try_new_on_control(1, storage).unwrap();
        prepared
            .copy_resident_source_on_control(
                asset,
                &hex_engine::Audio::new(24000, vec![[0.25; 2]; 4]).unwrap(),
            )
            .unwrap();
        prepared
            .prepare_current_sampler_on_control(&project, 11, context, 4, &AtomicBool::new(false))
            .unwrap();
        let identity = prepared.owner_on_control().unwrap().identity();
        let mut cell = Some(prepared);
        let limits = crate::AudioCandidatePublicationReservation {
            runtime_slots: 1,
            archive_slots: 1,
            receipt_rows: 1,
            member_rows: quote.member_rows,
            transfer_rows: quote.transfer_rows,
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows * 2,
            max_retained_data_bytes: storage.max_prepared_data_bytes * 2,
            max_construction_peak_data_bytes: storage.max_construction_data_bytes * 2,
        };
        let (mut control, mut data, errors, mut retirement) = anchored();
        control
            .configure_audio_candidate_publication(limits)
            .unwrap();
        let mut target_storage = storage;
        if !source_limited {
            target_storage.max_prepared_data_bytes = 1;
            target_storage.max_construction_data_bytes = 1;
        }
        let token = control
            .reserve_audio_candidate_adoption(crate::AudioCandidatePreparationReservation {
                source_capacity: usize::from(!source_limited),
                storage: target_storage,
                member_rows: quote.member_rows,
                transfer_rows: quote.transfer_rows,
                handle_rows: quote.handle_rows,
            })
            .unwrap_or_else(|e| panic!("{}", e.reason));
        assert!(
            control
                .adopt_audio_candidate_preparation(token, context, &mut cell)
                .is_err()
        );
        assert_eq!(
            cell.as_mut()
                .unwrap()
                .owner_on_control()
                .unwrap()
                .identity(),
            identity
        );
        assert_eq!(control.status.submitted_revision.load(Acquire), 0);
        let mut output = [[1.0; 2]; 8];
        data.owner_mut().unwrap().run(|callback, _| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(output, [[0.0; 2]; 8]);
        assert!(control.cancel_audio_candidate_preparation(token));
        cell.as_mut().unwrap().cleanup_on_control().unwrap();
        cell.as_mut().unwrap().cleanup_on_control().unwrap();
        drop(cell);
        drop(data);
        drop(errors);
        finish(&mut retirement);
    }
}

#[test]
fn reversed_finite_sampler_repeats_keep_exact_callback_gates_without_allocation() {
    let _serial = serial();
    let (project, asset) = fixture();
    let mut e = Editor::new(project, 32).unwrap();
    let track = e.project().tracks()[0].id();
    let clip = e.project().track(track).unwrap().midi_clips()[0].id();
    let note = e.project().midi_clip(clip).unwrap().notes()[0].id();
    e.execute(Command::RemoveMidiNote(note)).unwrap();
    let beat = |frame| e.project().tempo_map().frame_to_beat(frame).unwrap();
    let period = beat(24);
    let duration = beat(6);
    let length = beat(60);
    e.execute(Command::SetMidiClipSource {
        clip,
        source_length: period,
        source_start: BeatTime::ZERO,
        loop_start: BeatTime::ZERO,
        loop_end: period,
        looping: true,
        length,
    })
    .unwrap();
    e.execute(Command::AddMidiNote {
        clip,
        start: duration,
        duration,
        pitch: 60,
        channel: 3,
        velocity: 127,
    })
    .unwrap();
    e.execute(Command::ReverseClips(vec![clip])).unwrap();
    let project = e.project();
    let onsets = hex_engine::midi_compile::midi_events(project)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == hex_engine::midi_compile::MidiEventKind::NoteOn)
        .map(|e| e.frame)
        .collect::<Vec<_>>();
    assert_eq!(onsets, vec![0, 24, 48]);
    let quote = AudioCandidateReservationQuote::for_project(project, 1).unwrap();
    let storage = quote.storage_reservation(65_536).unwrap();
    let context = AudioSourceContext {
        project_generation: 4,
        editor_revision: 7,
    };
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, storage).unwrap();
    let pcm = hex_engine::Audio::new(
        24_000,
        vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]],
    )
    .unwrap();
    cell.copy_resident_source_on_control(asset, &pcm).unwrap();
    cell.prepare_current_sampler_on_control(project, 11, context, 4, &AtomicBool::new(false))
        .unwrap();
    let limits = crate::AudioCandidatePublicationReservation {
        runtime_slots: 3,
        archive_slots: 3,
        receipt_rows: 4,
        member_rows: quote.member_rows * 3,
        transfer_rows: quote.transfer_rows * 3,
        data_rows: quote.data_rows,
        handle_rows: quote.handle_rows * 4,
        max_retained_data_bytes: storage.max_prepared_data_bytes * 3,
        max_construction_peak_data_bytes: storage.max_construction_data_bytes * 3,
    };
    let (mut control, mut data, errors, mut retirement) = anchored();
    control
        .configure_audio_candidate_publication(limits)
        .unwrap();
    let token = control
        .reserve_audio_candidate_adoption(crate::AudioCandidatePreparationReservation {
            source_capacity: 1,
            storage,
            member_rows: quote.member_rows,
            transfer_rows: quote.transfer_rows,
            handle_rows: quote.handle_rows,
        })
        .unwrap_or_else(|e| panic!("{}", e.reason));
    let mut cell = Some(cell);
    control
        .adopt_audio_candidate_preparation(token, context, &mut cell)
        .unwrap();
    let ticket = control
        .stage_current_sampler_preparation(token, context)
        .unwrap();
    control
        .command(crate::prepared::PlaybackCommand::Play)
        .unwrap();
    let mut output = [[0.0; 2]; 66];
    let counts = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|callback, _| {
            for block in output.chunks_mut(7) {
                callback.begin_block();
                callback.render(block).unwrap();
            }
        })
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    for onset in [0, 24, 48] {
        assert_eq!(output[onset], [0.125, -0.25]);
        assert!(output[onset..onset + 6].iter().any(|f| *f != [0.0; 2]));
    }
    assert!(output[60..].iter().all(|f| *f == [0.0; 2]));
    control
        .command(crate::prepared::PlaybackCommand::Stop)
        .unwrap();
    data.owner_mut().unwrap().run(|callback, _| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    control.request_shutdown();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    let mut backend = Some(());
    let mut pending = false;
    let mut disposed = false;
    assert!(
        !crate::lifecycle::test_dispose_backend(
            &mut backend,
            &mut pending,
            &mut disposed,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    drop(data);
    drop(errors);
    let _ = retirement.advance().unwrap();
    drop(recover(&mut control, &ticket));
    finish(&mut retirement);
    drop(control);
}
