//! Native control returns exact verified-file compiler facts without claiming a page loan.
use super::*;
use hex_session::service::ResidentFileInput;

#[test]
fn long_file_pitch_playback_undo_and_redo_use_verified_pages() {
    use super::resident_preview::ResidentHarness;
    use std::io::Write;
    const FRAMES: u32 = 20_000_000;
    let _native = native_fixture();
    let files = Files::new();
    let path = files.0.join("long-pitch.wav");
    let mut file = fs::File::create(&path).unwrap();
    let bytes = FRAMES * 2;
    file.write_all(b"RIFF").unwrap();
    file.write_all(&(36 + bytes).to_le_bytes()).unwrap();
    file.write_all(b"WAVEfmt ").unwrap();
    file.write_all(&16u32.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&1u16.to_le_bytes()).unwrap();
    file.write_all(&48_000u32.to_le_bytes()).unwrap();
    file.write_all(&96_000u32.to_le_bytes()).unwrap();
    file.write_all(&2u16.to_le_bytes()).unwrap();
    file.write_all(&16u16.to_le_bytes()).unwrap();
    file.write_all(b"data").unwrap();
    file.write_all(&bytes.to_le_bytes()).unwrap();
    let mut prefix = Vec::with_capacity(16_384);
    for index in 0..8192 {
        let phase = index as f32 * (std::f32::consts::TAU * 440.0 / 48_000.0);
        prefix.extend_from_slice(&((phase.sin() * 8_000.0) as i16).to_le_bytes());
    }
    file.write_all(&prefix).unwrap();
    file.set_len(u64::from(bytes) + 44).unwrap();
    drop(file);
    let mut h = ResidentHarness::with_frame_limit(16_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path: path.clone(),
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    let snapshot = imported.state.canonical_audio_source.as_ref().unwrap();
    let AudioSourceBinding::VerifiedFile(captured) = snapshot.sources().values().next().unwrap()
    else {
        panic!("file must remain streamed")
    };
    let fingerprint = captured.fingerprint();
    let limits = captured.reader_limits();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    assert_eq!(
        imported.state.project.clip(clip).unwrap().source_length(),
        u64::from(FRAMES)
    );
    h.run(Request::Edit(hex_project::Command::SetClipFades {
        clip,
        fade_in: 128,
        fade_out: 2000,
    }));
    (applied, _) = h.applied(applied);
    let pitched_event = h.run(Request::Edit(hex_project::Command::SetAudioClipPitch {
        clip,
        cents: 700,
    }));
    (applied, _) = h.applied(applied);
    let edited = pitched_event.state.project.clip(clip).unwrap();
    assert_eq!(edited.retained_fade().fade_in(), 128);
    fn crossings(h: &mut ResidentHarness) -> usize {
        h.audio
            .as_mut()
            .unwrap()
            .control
            .command(PlaybackCommand::PlayFrom(128))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            h.pump();
            if h.render()
                .iter()
                .flatten()
                .any(|sample| sample.abs() > 0.01)
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "verified pages never reached the callback"
            );
            std::thread::yield_now();
        }
        let mut previous = 0.0;
        let mut count = 0;
        for _ in 0..32 {
            h.pump();
            for frame in h.render() {
                if frame[0] > 0.0 && previous <= 0.0 {
                    count += 1;
                }
                previous = frame[0];
            }
            std::thread::sleep(Duration::from_micros(250));
        }
        count
    }
    let pitched = crossings(&mut h);
    assert!((24..=32).contains(&pitched), "pitch: {pitched} crossings");
    h.run(Request::Undo);
    (applied, _) = h.applied(applied);
    let raw = crossings(&mut h);
    assert!((16..=22).contains(&raw), "undo: {raw} crossings");
    h.run(Request::Redo);
    h.applied(applied);
    let redone = crossings(&mut h);
    assert!((24..=32).contains(&redone), "redo: {redone} crossings");
    let saved = files.0.join("pitched-fade.hex");
    assert!(h.run(Request::SaveNew(saved.clone())).save_complete);
    let output = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let reopened = h.run(Request::Open(saved));
    assert!(reopened.open_complete);
    assert_eq!(
        reopened.state.project.snapshot(),
        pitched_event.state.project.snapshot()
    );
    super::resident_lifecycle::finish_replacement(&mut h, &reopened, output);
    let reopened_crossings = crossings(&mut h);
    assert!(
        (24..=32).contains(&reopened_crossings),
        "reopened: {reopened_crossings} crossings"
    );
    let after = hex_session::VerifiedAudioFile::inspect(path, 48_000, limits, || false).unwrap();
    assert_eq!(after.fingerprint(), fingerprint);
}

#[test]
fn ordinary_file_pitch_is_published_through_the_verified_native_domain() {
    use super::resident_preview::ResidentHarness;
    let _native = native_fixture();
    let files = Files::new();
    let path = files.wav(48_000, 24_000);
    let mut wav = fs::read(&path).unwrap();
    for (index, bytes) in wav[44..].chunks_exact_mut(2).enumerate() {
        let phase = index as f32 * (std::f32::consts::TAU * 440.0 / 48_000.0);
        bytes.copy_from_slice(&((phase.sin() * 8_000.0) as i16).to_le_bytes());
    }
    fs::write(&path, wav).unwrap();
    let mut h = ResidentHarness::with_frame_limit(16_000);
    h.next();
    let (mut applied, _) = h.applied(0);
    let imported = h.run(Request::ImportToNewTrack {
        path,
        before_track: None,
        start: 0,
    });
    (applied, _) = h.applied(applied);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .all(|source| matches!(source, AudioSourceBinding::VerifiedFile(_)))
    );
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    h.run(Request::Edit(hex_project::Command::SetClipFades {
        clip,
        fade_in: 0,
        fade_out: 0,
    }));
    (applied, _) = h.applied(applied);
    let changed = h.run(Request::Edit(hex_project::Command::SetAudioClipPitch {
        clip,
        cents: 700,
    }));
    let (_, metadata) = h.applied(applied);
    assert_eq!(metadata.context.editor_revision, changed.state.revision);
    assert_eq!(
        changed
            .state
            .project
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .pitch_cents,
        700
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        h.pump();
        if h.render()
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 0.01)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "pitched verified pages never reached the callback"
        );
        std::thread::yield_now();
    }
    // 700 cents takes 440 Hz to about 659 Hz: approximately 42 rising
    // crossings in 3072 frames. Raw pages would yield only about 28.
    let mut previous = 0.0;
    let mut crossings = 0;
    for _ in 0..48 {
        h.pump();
        for frame in h.render() {
            if frame[0] > 0.0 && previous <= 0.0 {
                crossings += 1;
            }
            previous = frame[0];
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(
        (36..=48).contains(&crossings),
        "unexpected pitched frequency: {crossings} crossings"
    );
}

#[test]
fn native_control_binds_actual_verified_file_to_ordinary_catalog_metadata() {
    let _native = native_fixture();
    let files = Files::new();
    let mut h = Harness::new();
    h.next();
    h.open();
    h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::VerifiedStreaming,
        reservation: Some(AudioPolicy::source_reservation()),
    });
    h.open();
    let event = h.run(Request::Import(files.wav(48_000, 128)));
    let source = event.state.canonical_audio_source.as_ref().unwrap();
    let (asset, binding) = source.required_sources().next().unwrap();
    let AudioSourceBinding::VerifiedFile(file) = binding else {
        panic!("verified source")
    };
    let (sender, _receiver) = mpsc::sync_channel(1);
    let mut sink = NativeAudioSink::new(sender);
    let rows = [(asset, file)];
    let metadata = sink
        .prepare_resident_file_metadata(ResidentFileInput {
            context: source.context(),
            semantic_source_epoch: 1,
            files: &rows,
            pitches: &[],
            repitches: &[],
            granulars: &[],
        })
        .unwrap()
        .unwrap();
    let catalog = metadata.catalog_generation();
    assert!(catalog > 0);
    assert_eq!(
        metadata.sources()[&asset].as_ref(),
        &file.source_metadata(asset, catalog).unwrap()
    );
    let pitches = [hex_session::service::ResidentFilePitch {
        asset,
        cents: 700,
        law: hex_project::WarpResampler::V2,
    }];
    let raw_input = ResidentFileInput {
        context: source.context(),
        semantic_source_epoch: 1,
        files: &rows,
        pitches: &[],
        repitches: &[],
        granulars: &[],
    };
    let pitch_input = ResidentFileInput {
        context: source.context(),
        semantic_source_epoch: 1,
        files: &rows,
        pitches: &pitches,
        repitches: &[],
        granulars: &[],
    };
    let raw_domain = crate::native_sources::DomainSources::capture_files(&raw_input, 19).unwrap();
    assert!(!raw_domain.matches_files(&pitch_input));
    let pitch_domain =
        crate::native_sources::DomainSources::capture_files(&pitch_input, 19).unwrap();
    assert!(pitch_domain.matches_files(&raw_input));
    assert!(pitch_domain.matches_files(&pitch_input));
    assert_eq!(pitch_domain.definitions().unwrap().len(), 2);
    let metadata = sink
        .prepare_resident_file_metadata(pitch_input)
        .unwrap()
        .unwrap();
    let expected = file
        .source_metadata(asset, metadata.catalog_generation())
        .unwrap()
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
    assert_eq!(
        metadata.derived_sources()[&expected.key()].as_ref(),
        &expected
    );
    assert!(metadata.matches_source(&expected));
    let clip = event.state.project.tracks()[0].clips()[0].id();
    let mut editor = hex_project::Editor::new(event.state.project.as_ref().clone(), 8).unwrap();
    editor
        .execute(hex_project::Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let warp = hex_project::default_audio_warp(
        editor.project().clip(clip).unwrap(),
        editor.project().tempo_map(),
        hex_project::WarpMode::RePitch,
    )
    .unwrap();
    editor
        .execute(hex_project::Command::SetAudioClipWarp {
            clip,
            settings: Some(warp),
        })
        .unwrap();
    let raw = file.source_metadata(asset, 19).unwrap();
    let requirement = hex_engine::audio_transform::StreamedRepitchRequirement::new(
        &raw,
        editor.project().clip(clip).unwrap().audio_edit().unwrap(),
        editor.project().tempo_map(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let repitches = [requirement];
    let repitch_input = ResidentFileInput {
        context: source.context(),
        semantic_source_epoch: 1,
        files: &rows,
        pitches: &[],
        repitches: &repitches,
        granulars: &[],
    };
    assert!(!pitch_domain.matches_files(&repitch_input));
    let repitch_domain =
        crate::native_sources::DomainSources::capture_files(&repitch_input, 19).unwrap();
    assert!(repitch_domain.matches_files(&repitch_input));
    assert!(
        repitch_domain
            .definitions_for_files(&repitch_input, 0)
            .is_err()
    );
    let definitions = repitch_domain
        .definitions_for_files(&repitch_input, 32 * 1024 * 1024)
        .unwrap();
    assert_eq!(definitions.len(), 2);
    assert!(
        definitions
            .iter()
            .any(|definition| definition.key.repitch_identity().is_some())
    );
    let foreign_path = files.constant_wav("foreign-repitch.wav", 48_000, 128, &[4096]);
    let foreign =
        hex_session::VerifiedAudioFile::inspect(foreign_path, 48_000, file.reader_limits(), || {
            false
        })
        .unwrap();
    let foreign_rows = [(asset, &foreign)];
    let foreign_input = ResidentFileInput {
        context: source.context(),
        semantic_source_epoch: 1,
        files: &foreign_rows,
        pitches: &[],
        repitches: &repitches,
        granulars: &[],
    };
    assert!(!repitch_domain.matches_files(&foreign_input));
    assert!(crate::native_sources::DomainSources::capture_files(&foreign_input, 19).is_err());
    assert!(
        repitch_domain
            .definitions_for_files(&raw_input, 32 * 1024 * 1024)
            .is_err()
    );
    let mut changed = hex_project::Editor::new(editor.project().clone(), 8).unwrap();
    changed
        .execute(hex_project::Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(180_000_000).unwrap(),
        })
        .unwrap();
    let changed_requirement = hex_engine::audio_transform::StreamedRepitchRequirement::new(
        &raw,
        changed.project().clip(clip).unwrap().audio_edit().unwrap(),
        changed.project().tempo_map(),
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let changed_repitches = [changed_requirement];
    let changed_input = ResidentFileInput {
        context: source.context(),
        semantic_source_epoch: 1,
        files: &rows,
        pitches: &[],
        repitches: &changed_repitches,
        granulars: &[],
    };
    assert!(!repitch_domain.matches_files(&changed_input));
    assert!(
        repitch_domain
            .definitions_for_files(&changed_input, 32 * 1024 * 1024)
            .is_err()
    );
    let metadata = sink
        .prepare_resident_file_metadata(repitch_input)
        .unwrap()
        .unwrap();
    let expected_repitch = repitches[0]
        .metadata_for(
            &file
                .source_metadata(asset, metadata.catalog_generation())
                .unwrap(),
        )
        .unwrap();
    assert!(metadata.matches_source(&expected_repitch));
    assert_eq!(
        source.context(),
        event
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
    );
}
