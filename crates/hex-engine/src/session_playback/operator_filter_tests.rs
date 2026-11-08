use super::*;
use hex_devices::operator_state::OPERATOR_METADATA;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
fn fixture() -> Project {
    fixture_duration(8 << 32)
}
fn fixture_duration(duration: u64) -> Project {
    let mut snapshot = project(1.0, 0, duration).snapshot();
    let mut state = crate::operator_legacy_test::legacy_operator_state();
    for (id, v) in [
        (1, 1.0),
        (2, 1.0),
        (29, 1.0),
        (33, 1.0),
        (34, 1.0),
        (35, 1.0),
        (36, 0.0),
        (37, 0.001),
        (38, 0.01),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                ParameterId::from_static(id),
                ParameterValue::Float(v),
            )
            .unwrap();
    }
    let id = snapshot.next_id;
    snapshot.next_id += 1;
    snapshot.tracks[0].instrument = Some(DeviceSnapshot {
        parent: None,
        automation: Vec::new(),
        comparison: None,
        id,
        bypassed: false,
        state,
    });
    snapshot.tracks[0].instrument_position = Some(0);
    Project::from_legacy_snapshot(snapshot).unwrap()
}
fn enable(source: &Project) -> Project {
    let mut editor = Editor::new(source.clone(), 16).unwrap();
    let instrument = editor.project().tracks()[0].instrument().unwrap();
    let device = instrument.id();
    let mut state = instrument.state().clone();
    for (id, value) in [
        (20_001, ParameterValue::Bool(true)),
        (20_005, ParameterValue::Float(20.0)),
    ] {
        state = OPERATOR_METADATA
            .patch_parameter(&state, ParameterId::from_static(id), value)
            .unwrap();
    }
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    editor.project().clone()
}

fn modulation(source: Project, kind: u8) -> Project {
    let mut editor = Editor::new(source, 16).unwrap();
    let instrument = editor.project().tracks()[0].instrument().unwrap();
    let device = instrument.id();
    let mut state = instrument.state().clone();
    if kind > 0 {
        state = OPERATOR_METADATA
            .patch_parameter(
                &state,
                ParameterId::from_static(78),
                ParameterValue::Float(0.4),
            )
            .unwrap();
    }
    if kind == 2 {
        for (id, v) in [
            (82, ParameterValue::Bool(true)),
            (87, ParameterValue::Float(35.0)),
            (85, ParameterValue::Float(7.0)),
        ] {
            state = OPERATOR_METADATA
                .patch_parameter(&state, ParameterId::from_static(id), v)
                .unwrap();
        }
    }
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    editor.project().clone()
}
#[test]
fn operator_filter_imports_sounding_analytic_notes_without_retrigger_or_duplicate_release() {
    for kind in 0..3 {
        for session in [false, true] {
            let source = if session {
                session_only_project(&fixture())
            } else {
                fixture()
            };
            let source = modulation(source, kind);
            let table = Arc::new(hex_devices::SineTable::new());
            let old = revision(&source, Arc::clone(&table));
            let mut previous = SessionPlayback::new(Arc::clone(&old)).unwrap();
            let mut dry = SessionPlayback::new(Arc::clone(&old)).unwrap();
            for p in [&mut previous, &mut dry] {
                if session {
                    p.apply(SessionPlaybackCommand::LaunchClip {
                        clip: old.session().midi_clips()[0].id,
                        quantization: LaunchQuantization::Immediate,
                    })
                    .unwrap();
                }
                p.apply(SessionPlaybackCommand::Play).unwrap();
                render_checkpoint_prefix(p, 128);
            }
            let new = Arc::new(
                PreparedRevision::prepare(
                    &enable(&source),
                    &BTreeMap::new(),
                    table,
                    Some(old.session()),
                )
                .unwrap(),
            );
            let mut target = SessionPlayback::new(new).unwrap();
            let scratch_pointers = (
                target.analytic_import.seeds.as_ptr(),
                target.analytic_import.proofs.as_ptr(),
            );
            if kind == 0 {
                // Reuse the real candidate scratch across a successful query and
                // a declined source, then perform the normal two-phase handoff.
                let clock = previous.timing_view();
                assert!(
                    SessionPlayback::analytic_import_plan(
                        &previous,
                        0,
                        &target.revision,
                        0,
                        &clock,
                        &mut target.spans,
                        &mut target.analytic_import,
                    )
                    .unwrap()
                );
                assert_eq!(target.analytic_import.len, 1);
                assert!(
                    !SessionPlayback::analytic_import_plan(
                        &previous,
                        0,
                        &old,
                        0,
                        &clock,
                        &mut target.spans,
                        &mut target.analytic_import,
                    )
                    .unwrap()
                );
                assert_eq!(target.analytic_import.len, 0);
            }
            target.try_inherit_state_from(&mut previous).unwrap();
            assert_eq!(
                scratch_pointers,
                (
                    target.analytic_import.seeds.as_ptr(),
                    target.analytic_import.proofs.as_ptr(),
                )
            );
            assert_eq!(target.performance_frame(), 128);
            assert_eq!(
                target.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .occurrence_proofs()
                    .len(),
                1
            );
            assert!(target.releases.is_empty());
            let mut a = [[0.0; 2]; 1];
            let mut b = a;
            target.render(&mut a).unwrap();
            dry.render(&mut b).unwrap();
            assert_eq!(
                a, b,
                "activation boundary must retain dry phase and amplitude"
            );
            assert!(a[0][0] != 0.0);
            render_checkpoint_prefix(&mut target, 4096);
            let muted = render_checkpoint_prefix(&mut target, 256);
            let dry_pcm = render_checkpoint_prefix(&mut dry, 256);
            let energy =
                |x: &Vec<crate::Frame>| x.iter().map(|f| f64::from(f[0]).powi(2)).sum::<f64>();
            if kind == 0 {
                assert!(energy(&muted) < energy(&dry_pcm) * 0.01);
            } else {
                // Feedback may generate DC and LFO sidebands below cutoff;
                // a low-pass must preserve them rather than promise total RMS rejection.
                assert_ne!(muted, dry_pcm);
                assert!(muted.iter().flatten().all(|x| x.is_finite()));
            }
            assert_eq!(
                target.mutable_instruments[0]
                    .as_ref()
                    .unwrap()
                    .occurrence_proofs()
                    .len(),
                1
            );
        }
    }
}

#[test]
fn operator_filter_pause_seek_stop_and_saved_snapshot_preserve_expected_history() {
    let source = enable(&fixture());
    let saved = Project::from_legacy_snapshot(source.snapshot()).unwrap();
    assert_eq!(source.snapshot(), saved.snapshot());
    let table = Arc::new(hex_devices::SineTable::new());
    let prepared = revision(&source, Arc::clone(&table));
    let mut active = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut continuous = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    for p in [&mut active, &mut continuous] {
        p.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(p, 333);
    }
    active.apply(SessionPlaybackCommand::Pause).unwrap();
    let at = active.performance_frame();
    continuous
        .apply(SessionPlaybackCommand::StopTrack {
            track: source.tracks()[0].id(),
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    let mut released = [[0.0; 2]; 128];
    let mut expected_release = released;
    active.render(&mut released).unwrap();
    continuous.render(&mut expected_release).unwrap();
    assert_eq!(
        released, expected_release,
        "Pause preserves the one native kernel while releasing authored gates"
    );
    assert_eq!(active.performance_frame(), at);
    active.apply(SessionPlaybackCommand::Play).unwrap();
    assert_eq!(
        render_checkpoint_prefix(&mut active, 257),
        render_checkpoint_prefix(&mut continuous, 257)
    );
    assert_eq!(
        active.apply(SessionPlaybackCommand::Seek(0)),
        Err(SessionPlaybackError::MutableInstrumentActive)
    );
    assert_eq!(
        render_checkpoint_prefix(&mut active, 128),
        render_checkpoint_prefix(&mut continuous, 128)
    );
    active.apply(SessionPlaybackCommand::Stop).unwrap();
    active.apply(SessionPlaybackCommand::Seek(0)).unwrap();
    active.apply(SessionPlaybackCommand::Play).unwrap();
    let mut fresh = SessionPlayback::new(revision(&saved, Arc::clone(&table))).unwrap();
    fresh.apply(SessionPlaybackCommand::Play).unwrap();
    assert_eq!(
        render_checkpoint_prefix(&mut active, 512),
        render_checkpoint_prefix(&mut fresh, 512)
    );
}

#[test]
fn operator_filter_refuses_ambiguous_reloaded_sounding_sources_without_changing_old_audio() {
    let source = fixture();
    let table = Arc::new(hex_devices::SineTable::new());
    let prepared = revision(&source, Arc::clone(&table));
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    for p in [&mut old, &mut reference] {
        p.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(p, 128);
    }
    // Identical serialized notes have a new runtime source identity after reload.
    let reloaded = Project::from_legacy_snapshot(enable(&source).snapshot()).unwrap();
    let candidate = Arc::new(
        PreparedRevision::prepare(&reloaded, &BTreeMap::new(), table, Some(prepared.session()))
            .unwrap(),
    );
    let mut next = SessionPlayback::new(candidate).unwrap();
    assert!(next.try_inherit_state_from(&mut old).is_err());
    assert_eq!(
        render_checkpoint_prefix(&mut old, 256),
        render_checkpoint_prefix(&mut reference, 256)
    );
}

#[test]
fn operator_filter_imports_released_analytic_voice_with_original_gate_and_finite_end() {
    let source = fixture_duration(1 << 24);
    let table = Arc::new(hex_devices::SineTable::new());
    let prepared = revision(&source, Arc::clone(&table));
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    let mut reference = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    for p in [&mut old, &mut reference] {
        p.apply(SessionPlaybackCommand::Play).unwrap();
        render_checkpoint_prefix(p, 128);
    }
    let next_revision = Arc::new(
        PreparedRevision::prepare(
            &enable(&source),
            &BTreeMap::new(),
            table,
            Some(prepared.session()),
        )
        .unwrap(),
    );
    let mut next = SessionPlayback::new(next_revision).unwrap();
    next.try_inherit_state_from(&mut old).unwrap();
    assert_eq!(
        render_checkpoint_prefix(&mut next, 1),
        render_checkpoint_prefix(&mut reference, 1)
    );
    assert!(next.releases.is_empty());
    let release = next.mutable_instruments[0]
        .as_ref()
        .unwrap()
        .max_release_tail_frames() as usize;
    render_checkpoint_prefix(&mut next, release + 128);
    assert!(
        next.mutable_instruments[0]
            .as_ref()
            .unwrap()
            .committed_is_quiescent()
    );
    assert!(
        render_checkpoint_prefix(&mut next, 128)
            .iter()
            .all(|f| *f == [0.0; 2])
    );
}

#[test]
fn operator_filter_does_not_import_an_active_analytic_enable_continuation() {
    let source = fixture();
    let table = Arc::new(hex_devices::SineTable::new());
    let prepared = revision(&source, Arc::clone(&table));
    let mut old = SessionPlayback::new(Arc::clone(&prepared)).unwrap();
    old.apply(SessionPlaybackCommand::Play).unwrap();
    render_checkpoint_prefix(&mut old, 128);
    let mut editor = Editor::new(source, 16).unwrap();
    let instrument = editor.project().tracks()[0].instrument().unwrap();
    let device = instrument.id();
    let state = OPERATOR_METADATA
        .patch_parameter(
            instrument.state(),
            ParameterId::from_static(513),
            ParameterValue::Bool(false),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let changed = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::clone(&table),
            Some(prepared.session()),
        )
        .unwrap(),
    );
    let mut active = SessionPlayback::new(Arc::clone(&changed)).unwrap();
    active.try_inherit_state_from(&mut old).unwrap();
    render_checkpoint_prefix(&mut active, 17);
    assert!(
        active
            .analytic_continuations
            .iter()
            .flatten()
            .any(|r| r.elapsed_at(active.performance_frame()).unwrap() < r.duration())
    );
    let filtered = Arc::new(
        PreparedRevision::prepare(
            &enable(editor.project()),
            &BTreeMap::new(),
            table,
            Some(changed.session()),
        )
        .unwrap(),
    );
    let mut next = SessionPlayback::new(filtered).unwrap();
    let before = active.performance_frame();
    assert!(next.try_inherit_state_from(&mut active).is_err());
    assert_eq!(active.performance_frame(), before);
    assert!(active.mutable_instruments[0].is_none());
}
