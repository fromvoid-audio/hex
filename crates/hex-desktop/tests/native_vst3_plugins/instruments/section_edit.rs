//! Section edits following a real refused live VST3 graph handoff.
use super::*;
use crate::native_audio::tests::resident_sections::run_callback;
use hex_engine::PlaybackHandoffRejection;
use hex_project::ElementColor;

#[test]
#[ignore = "requires explicit source-bound SDK receipts"]
fn duplicate_section_recovers_after_a_refused_live_midi_edit() {
    const CHILD: &str = "HEX_DESKTOP_TEST_SECTION_RECOVERY_CHILD";
    if std::env::var_os(CHILD).is_none() {
        let name = std::thread::current().name().unwrap().to_owned();
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", &name, "--ignored", "--nocapture"])
            .env(CHILD, "1")
            .env("HEX_ORIGINAL_INSTANCE_VARIANT", "127")
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}\n{}",
            String::from_utf8_lossy(&result.stdout),
            String::from_utf8_lossy(&result.stderr)
        );
        return;
    }
    let _native = native_fixture();
    let mut h = ResidentHarness::with_plugins(
        host(),
        Some(EffectPresentation {
            input_frames: 0,
            output_frames: 512,
        }),
    );
    assert!(h.next().error.is_none());
    let (applied, _) = h.applied(0);
    let track = h.run(Request::Edit(Command::AddMidiTrack {
        name: "synth".into(),
    }));
    let Some(Created::Track(track)) = track.created else {
        panic!("midi track");
    };
    let (applied, _) = h.applied(applied);
    let clip = h.run(Request::Edit(Command::PlaceMidiClip {
        track,
        start: BeatTime::ZERO,
        length: BeatTime::from_quarters(4).unwrap(),
    }));
    let Some(Created::Clip(clip)) = clip.created else {
        panic!("midi clip");
    };
    let (applied, _) = h.applied(applied);
    let duration = BeatTime::from_quarters(2).unwrap();
    let added = h.run(Request::Edit(Command::AddMidiNote {
        clip,
        start: BeatTime::ZERO,
        duration,
        pitch: 60,
        channel: 0,
        velocity: 100,
    }));
    let Some(Created::Note(note)) = added.created else {
        panic!("midi note");
    };
    let (applied, _) = h.applied(applied);
    let added = h.run(Request::EditArrangementSection {
        project_generation: 0,
        revision: added.state.revision,
        command: Command::AddArrangementSection {
            name: "intro".into(),
            color: ElementColor::Cyan,
            start: BeatTime::ZERO,
            end: BeatTime::from_quarters(4).unwrap(),
        },
    });
    let Some(Created::ArrangementSection(section)) = added.created else {
        panic!("section");
    };
    let (applied, _) = h.applied(applied);
    h.service
        .submit(Request::InsertPluginEffect(request(
            &added.state,
            PluginEffectDestination::Instrument(track),
        )))
        .unwrap();
    let inserted = terminal(&mut h);
    assert!(inserted.error.is_none(), "{:?}", inserted.error);
    let (applied, _) = h.applied(applied);
    let transport = |h: &mut ResidentHarness, command| {
        h.audio.as_mut().unwrap().control.command(command).unwrap();
        render_completed_block(h);
    };
    transport(&mut h, PlaybackCommand::Play);
    let edited = same_source_edit(
        &mut h,
        Request::Edit(Command::UpdateMidiNote {
            note,
            start: BeatTime::ZERO,
            duration,
            pitch: 60,
            channel: 0,
            velocity: 80,
        }),
    );
    render_completed_block(&h);
    let status = h.audio.as_ref().unwrap().status().unwrap();
    assert_eq!(status.applied_revision.load(Acquire), applied);
    let (rejected, reason) = status.rejected_revision().unwrap();
    assert!(rejected > applied);
    assert_eq!(reason, PlaybackHandoffRejection::SessionRuntime);

    let duplicate = || Request::EditArrangementSection {
        project_generation: 0,
        revision: edited.state.revision,
        command: Command::DuplicateArrangementSection(section),
    };
    let refused = run_callback(&mut h, duplicate());
    assert!(refused.error.is_some());
    assert_eq!(refused.state.revision, edited.state.revision);
    assert_eq!(refused.state.project, edited.state.project);
    let refused_duplicate = h
        .audio
        .as_ref()
        .unwrap()
        .status()
        .unwrap()
        .rejected_revision();

    // Leave the old gate behind while transport keeps running. No fresh
    // project/audio revision is submitted by this transport action.
    transport(&mut h, PlaybackCommand::PlayFrom(72_000));
    let duplicated = run_callback(&mut h, duplicate());
    assert!(duplicated.error.is_none(), "{:?}", duplicated.error);
    let Some(Created::ArrangementSection(copy)) = duplicated.created else {
        panic!("section copy");
    };
    assert_ne!(copy, section);
    assert_eq!(duplicated.state.project.arrangement_sections().len(), 2);
    assert!(refused_duplicate.is_some_and(|(revision, _)| revision > rejected));
    let track = duplicated.state.project.track(track).unwrap();
    assert_eq!(track.midi_clips().len(), 2);
    assert!(
        track
            .midi_clips()
            .iter()
            .all(|clip| clip.notes()[0].velocity() == 80)
    );
    let status = h.audio.as_ref().unwrap().status().unwrap();
    let installed = status.recording_source().unwrap();
    assert_eq!(installed.context.editor_revision, duplicated.state.revision);
    assert!(installed.audio_revision > refused_duplicate.unwrap().0);
    assert!(status.playing.load(Acquire));
    let undone = run_callback(&mut h, Request::Undo);
    assert!(undone.error.is_none(), "{:?}", undone.error);
    h.applied(installed.audio_revision);
    let mut restored = undone.state.project.snapshot();
    let before = edited.state.project.snapshot();
    restored.next_id = before.next_id;
    assert_eq!(restored, before);
}
