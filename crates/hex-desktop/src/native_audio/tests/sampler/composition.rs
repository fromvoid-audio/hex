//! Actual CURRENT edits, global history and failure behavior after source load.
use super::*;
use hex_session::service::{SamplerEdit, SamplerMidiEdit, ViewState};
use std::sync::atomic::Ordering;
fn intent(state: &ViewState, command: Command) -> SamplerMidiEdit {
    SamplerMidiEdit {
        request_id: 43,
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        command: Box::new(command),
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
fn edit(h: &mut Harness, state: &ViewState, command: Command) -> Arc<ViewState> {
    history::complete(
        h,
        Request::SamplerEdit(SamplerEdit::Midi(intent(state, command))),
    )
}
#[test]
fn sampler_composition_current_arrange_console_history_and_save_roundtrip() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, initial) = gain::current(12);
    let original = initial.project.track(track).unwrap().midi_clips()[0].id();
    let empty = edit(&mut h, &initial, Command::RemoveMidiClip(original));
    let placed = edit(
        &mut h,
        &empty,
        Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        },
    );
    let clip = placed.project.track(track).unwrap().midi_clips()[0].id();
    let notes = edit(
        &mut h,
        &placed,
        Command::AddMidiNotes {
            clip,
            notes: vec![hex_project::MidiNoteInsert {
                active: true,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 127,
            }],
        },
    );
    gain::original_pcm(&mut h);
    let gain = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::Parameter(gain::edit(&notes, track, device))),
    );
    let note = gain.project.midi_source(clip).unwrap().notes()[0].id();
    let low = edit(
        &mut h,
        &gain,
        Command::UpdateMidiNote {
            note,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 48,
            channel: 0,
            velocity: 127,
        },
    );
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let pcm = h.render();
    assert_eq!(
        &pcm[..3],
        &[[0.0625, -0.125], [0.109375, -0.09375], [0.15625, -0.0625]]
    );
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    let restored = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&low, false))),
    );
    assert_eq!(restored.project, gain.project);
    let original_gain = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&restored, false))),
    );
    assert_eq!(original_gain.project, notes.project);
    gain::original_pcm(&mut h);
    let gain_again = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&original_gain, true))),
    );
    let low_again = history::complete(
        &mut h,
        Request::SamplerEdit(SamplerEdit::History(history::history(&gain_again, true))),
    );
    assert_eq!(low_again.project, low.project);
    let scene = edit(&mut h, &low_again, Command::AddScene { name: "new".into() });
    let id = scene.project.scenes()[0].id();
    let copied = edit(
        &mut h,
        &scene,
        Command::CopyArrangementMidiToSession {
            source: clip,
            scene: id,
        },
    );
    let Some(hex_project::SessionCellContent::Midi(cell)) = copied.project.session_cell(track, id)
    else {
        panic!("cell")
    };
    let cell = cell.id();
    let removed = edit(
        &mut h,
        &copied,
        Command::RemoveMidiNotes {
            clip: cell,
            notes: vec![copied.project.midi_source(cell).unwrap().notes()[0].id()],
        },
    );
    assert!(
        removed
            .project
            .midi_source(cell)
            .unwrap()
            .notes()
            .is_empty()
    );
    let cleared = edit(
        &mut h,
        &removed,
        Command::ClearSessionCell { track, scene: id },
    );
    let created = edit(
        &mut h,
        &cleared,
        Command::CreateSessionMidiClip {
            track,
            scene: id,
            name: "fresh".into(),
            source_length: BeatTime::ONE,
        },
    );
    assert!(matches!(
        created.project.session_cell(track, id),
        Some(hex_project::SessionCellContent::Midi(_))
    ));
    let path = files.0.join("composition.hex");
    h.run(Request::SaveNew(path.clone()));
    let mut saved = hex_storage::load(&path).unwrap().snapshot();
    assert!(saved.assets.iter().all(|a| a.original_source.is_some()));
    // Save enriches verified source facts; every authored field still round-trips.
    for asset in &mut saved.assets {
        asset.original_source = None;
    }
    assert_eq!(saved, created.project.snapshot());
}
#[test]
fn sampler_composition_stale_playing_cancel_seal_budget_and_noop_preserve_current() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, _, before) = gain::current(12);
    let clip = before.project.track(track).unwrap().midi_clips()[0].id();
    let note = before.project.midi_source(clip).unwrap().notes()[0].id();
    let command = || Command::RemoveMidiNote(note);
    for case in 0..4 {
        let mut i = intent(&before, command());
        match case {
            0 => i.revision += 1,
            1 => i.project_generation += 1,
            2 => i.request_id = 0,
            _ => i.cancel.store(true, Ordering::Release),
        }
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SamplerEdit(SamplerEdit::Midi(i)))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        gain::unchanged(&before, &refused);
    }
    let old = h.audio.as_ref().unwrap().control.output_identity();
    let noop = Command::UpdateMidiNote {
        note,
        start: BeatTime::ZERO,
        duration: BeatTime::ONE,
        pitch: 60,
        channel: 0,
        velocity: 127,
    };
    let no = h.run(Request::SamplerEdit(SamplerEdit::Midi(intent(
        &before, noop,
    ))));
    assert_eq!(no.status.as_deref(), Some("no change"));
    gain::unchanged(&before, &no);
    assert_eq!(h.audio.as_ref().unwrap().control.output_identity(), old);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SamplerEdit(SamplerEdit::Midi(intent(
        &before,
        command(),
    ))));
    h.render();
    let refused = h.next();
    assert!(
        refused
            .error
            .as_ref()
            .unwrap()
            .contains("sampler edit audio changed")
    );
    gain::unchanged(&before, &refused);
    h.audio.as_ref().unwrap().control.stop();
    h.render();
    let i = intent(&before, command());
    h.observation.lock().unwrap().cancel_edit_after_seal = Some(Arc::clone(&i.cancel));
    h.run(Request::SamplerEdit(SamplerEdit::Midi(i)));
    h.render();
    let refused = h.next();
    assert!(refused.error.as_ref().unwrap().contains("cancelled"));
    gain::unchanged(&before, &refused);
    gain::original_pcm(&mut h);
    let pending = intent(&before, command());
    let cancel = Arc::clone(&pending.cancel);
    h.run(Request::SamplerEdit(SamplerEdit::Midi(pending)));
    let new = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(new.sampler_edit_complete, Some(43));
    assert!(new.state.project.tracks().is_empty());
    h.open();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    drop(h);
    let (_files, mut h, track, _, before) = gain::current(8);
    let clip = before.project.track(track).unwrap().midi_clips()[0].id();
    let note = before.project.midi_source(clip).unwrap().notes()[0].id();
    h.run(Request::SamplerEdit(SamplerEdit::Midi(intent(
        &before,
        Command::RemoveMidiNote(note),
    ))));
    h.render();
    let refused = h.next();
    assert!(
        refused.error.as_ref().unwrap().contains("frame limit"),
        "{:?}",
        refused.error
    );
    gain::unchanged(&before, &refused);
    gain::original_pcm(&mut h);
}
