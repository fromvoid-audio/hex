//! Controlled native VST3 effects through isolated bounce and atomic adoption.
use super::super::resident_sections::run_callback;
use super::*;
use hex_project::{BounceAdoption, BounceTarget, TrimEdge};

#[test]
#[ignore = "requires exact source-bound native helpers and original VST3 fixtures"]
fn native_vst3_effect_chain_bounces_streamed_audio_with_compensated_pcm_and_history() {
    let _native = native_fixture();
    let files = Files::new();
    let mut selected = selection();
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Fx", 0).unwrap();
    let recovery = files.0.join("recovery");
    fs::create_dir(&recovery).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        None,
        48_000,
        2_000_000,
        files.0.canonicalize().unwrap().join("plugin-catalog.json"),
        recovery.join("bounce.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    // A verified original larger than the resident budget, with a small
    // selected span and enough space for the normal bounce tail allowance.
    let path = files.wav(48_000, 2_048_000);
    let imported = h.run(Request::ImportToNewTrack {
        path: path.clone(),
        before_track: None,
        start: 0,
    });
    assert!(imported.error.is_none(), "{:?}", imported.error);
    assert!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_)))
    );
    let (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let clip = imported.state.project.tracks()[0].clips()[0].id();
    let trimmed = h.run(Request::Edit(Command::TrimClipEdge {
        clip,
        edge: TrimEdge::Right,
        frame: 1_024,
    }));
    assert!(trimmed.error.is_none(), "{:?}", trimmed.error);
    let (mut applied, _) = h.applied(applied);
    let mut source = trimmed;
    for _ in 0..2 {
        let mut insertion = request(&source.state, PluginEffectDestination::Track(track));
        insertion.selection = selected.clone();
        h.service
            .submit(Request::InsertPluginEffect(insertion))
            .unwrap();
        source = terminal(&mut h);
        assert!(source.error.is_none(), "{:?}", source.error);
        (applied, _) = h.applied(applied);
    }
    let before = Arc::clone(&source.state.project);
    let first_live = h.plugin_progress.lock().unwrap().binding;
    let bounce = |h: &mut ResidentHarness, revision, in_place| {
        run_callback(
            h,
            Request::BounceArrangement {
                project_generation: 0,
                revision,
                target: if in_place {
                    BounceTarget::Track(track)
                } else {
                    BounceTarget::Clips(vec![clip])
                },
                in_place,
                adoption: BounceAdoption::NewTracks,
                control: hex_export::ExportControl::default(),
            },
        )
    };
    let copied = bounce(&mut h, source.state.revision, false);
    assert!(copied.error.is_none(), "{:?}", copied.error);
    (applied, _) = h.applied(applied);
    assert_eq!(h.plugin_progress.lock().unwrap().binding, first_live);
    assert_eq!(copied.state.project.track(track), before.track(track));
    let copy = copied
        .state
        .project
        .tracks()
        .iter()
        .find(|row| row.id() != track)
        .unwrap();
    let expected = bounced_pcm(&copied.state.project, copy.clips()[0].asset());
    // The fixture's gain is 0.5 + 0.125 * block. The first bridge delays
    // input to the second processor by two quanta, including its priming.
    for (i, frame) in expected[..1024].iter().enumerate() {
        let block = (i / 256) as f32;
        let gain = (0.5 + 0.125 * block) * (0.75 + 0.125 * block);
        assert_eq!(*frame, [0.25 * gain; 2], "processed sample at {i}");
    }

    let baked = bounce(&mut h, copied.state.revision, true);
    assert!(baked.error.is_none(), "{:?}", baked.error);
    (applied, _) = h.applied(applied);
    assert_eq!(baked.state.revision, copied.state.revision + 1);
    let baked_track = baked.state.project.track(track).unwrap();
    assert!(baked_track.devices().is_empty());
    assert_eq!(baked_track.clips()[0].start(), 0);
    assert_eq!(
        bounced_pcm(&baked.state.project, baked_track.clips()[0].asset()),
        expected
    );

    let undo = run_callback(&mut h, Request::Undo);
    assert!(undo.error.is_none(), "{:?}", undo.error);
    (applied, _) = h.applied(applied);
    assert_undone(&undo.state.project, &copied.state.project);
    let mut changed = fs::read(&path).unwrap();
    changed[44] ^= 1;
    fs::write(path, changed).unwrap();
    let refused = bounce(&mut h, undo.state.revision, true);
    assert!(
        refused
            .error
            .as_deref()
            .is_some_and(|error| error.contains("changed")),
        "{:?}",
        refused.error
    );
    assert_eq!(refused.state.project, undo.state.project);
    assert_eq!(refused.state.revision, undo.state.revision);
    let redo = run_callback(&mut h, Request::Redo);
    assert!(redo.error.is_none(), "{:?}", redo.error);
    h.applied(applied);
    assert_eq!(redo.state.project, baked.state.project);
}

pub(super) fn assert_undone(actual: &hex_project::Project, before: &hex_project::Project) {
    let mut before = before.snapshot();
    let actual = actual.snapshot();
    assert!(
        actual.next_id >= before.next_id,
        "history never reuses identities"
    );
    before.next_id = actual.next_id;
    assert_eq!(actual, before);
}

fn bounced_pcm(
    project: &hex_project::Project,
    asset: hex_project::AssetId,
) -> Vec<hex_engine::Frame> {
    let path = project.asset(asset).unwrap().path();
    hex_media::decode_wav_file(path, hex_media::Limits::new(4 << 20, 1 << 16))
        .unwrap()
        .audio()
        .frames()
        .to_vec()
}

#[test]
#[ignore = "requires exact source-bound native helpers and original VST3 fixtures"]
fn native_vst3_streamed_main_export_uses_private_worker() {
    let _native = native_fixture();
    for limit in [8_192, 32_768] {
        native_effect_export_case(limit);
    }
}
fn native_effect_export_case(limit: u64) {
    use hex_session::service::{ExportControl, ExportSignal};
    let files = Files::new();
    let mut selected = selection();
    let module = files.0.canonicalize().unwrap().join("original.vst3");
    super::ui::copy_original_module(&selected.module, &module);
    selected.module = module;
    hex_ui::write_plugin_effect_test_catalog(&files.0, &selected, "Fx", 0).unwrap();
    let recovery = files.0.join("recovery");
    fs::create_dir(&recovery).unwrap();
    let mut h = ResidentHarness::with_plugins_at_rate_and_catalog_recovery(
        host(),
        None,
        48_000,
        limit,
        files.0.canonicalize().unwrap().join("plugin-catalog.json"),
        recovery.join("export.hex-journal"),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let original = files.wav(48_000, 16_384);
    let original_bytes = fs::read(&original).unwrap();
    let imported = h.run(Request::ImportToNewTrack {
        path: original.clone(),
        before_track: None,
        start: 0,
    });
    assert_eq!(
        imported
            .state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .sources()
            .values()
            .any(|source| matches!(source, hex_session::AudioSourceBinding::VerifiedFile(_))),
        limit == 8_192
    );
    let (applied, _) = h.applied(applied);
    let track = imported.state.project.tracks()[0].id();
    let mut insertion = request(&imported.state, PluginEffectDestination::Track(track));
    insertion.selection = selected;
    h.service
        .submit(Request::InsertPluginEffect(insertion))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    h.applied(applied);
    let before = Arc::clone(&inserted.state.project);
    let live = h.plugin_progress.lock().unwrap().binding;
    let path = files.0.join("private-export.wav");
    let queued = super::super::resident_sections::run_callback(
        &mut h,
        Request::ExportRange {
            path: path.clone(),
            start: 0,
            frames: Some(1024),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: false,
        },
    );
    assert!(queued.error.is_none(), "{:?}", queued.error);
    let deadline = Instant::now() + Duration::from_secs(10);
    let completed = loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_export_event() {
            break event;
        }
        assert!(Instant::now() < deadline, "private export timed out");
        std::thread::yield_now();
    };
    let report = completed.result.expect("private native export");
    assert_eq!(report.frames, 1024);
    let wav = fs::read(path).unwrap();
    let pcm: Vec<_> = wav[44..]
        .chunks_exact(4)
        .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
        .collect();
    assert!(
        pcm.iter().any(|sample| (1000..=7000).contains(sample)),
        "the private effect must audibly reduce the 8192-level input"
    );
    assert_eq!(h.plugin_progress.lock().unwrap().binding, live);
    assert_eq!(queued.state.project, before);
    assert_eq!(fs::read(&original).unwrap(), original_bytes);
    let refused = super::super::resident_sections::run_callback(
        &mut h,
        Request::ExportRange {
            path: original.clone(),
            start: 0,
            frames: Some(1024),
            signal: ExportSignal::Main,
            control: ExportControl::default(),
            format: hex_export::ExportFormat::Pcm16,
            overwrite: true,
        },
    );
    assert!(refused.error.is_none(), "{:?}", refused.error);
    let deadline = Instant::now() + Duration::from_secs(10);
    let error = loop {
        h.pump();
        h.render();
        if let Ok(event) = h.service.try_export_event() {
            break event.result.unwrap_err();
        }
        assert!(Instant::now() < deadline, "refusal timed out");
        std::thread::yield_now();
    };
    assert!(error.message().contains("original media"), "{error}");
    assert_eq!(fs::read(original).unwrap(), original_bytes);
}
