use super::*;
use crate::import_batch_decode::{AudioBatchCancel, AudioBatchPhase};
use crate::test_wait::HANG_BOUND;
use std::{fs, sync::atomic::AtomicU64};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Files(PathBuf);
impl Files {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "hex-Batch-Service-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
    fn wav(&self, name: &str, rate: u32, samples: &[i16]) -> PathBuf {
        let n = (samples.len() * 2) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + n).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&1_u16.to_le_bytes());
        bytes.extend_from_slice(&rate.to_le_bytes());
        bytes.extend_from_slice(&(rate * 2).to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&16_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&n.to_le_bytes());
        for sample in samples {
            bytes.extend_from_slice(&sample.to_le_bytes());
        }
        let p = self.0.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }
    fn recorded_wav(&self, name: &str, frames: &[[f32; 2]]) -> PathBuf {
        let n = (frames.len() * 8) as u32;
        let mut bytes = Vec::new();
        bytes.extend_from_slice(b"RIFF");
        bytes.extend_from_slice(&(36 + n).to_le_bytes());
        bytes.extend_from_slice(b"WAVEfmt ");
        bytes.extend_from_slice(&16_u32.to_le_bytes());
        bytes.extend_from_slice(&3_u16.to_le_bytes());
        bytes.extend_from_slice(&2_u16.to_le_bytes());
        bytes.extend_from_slice(&48_000_u32.to_le_bytes());
        bytes.extend_from_slice(&(48_000_u32 * 8).to_le_bytes());
        bytes.extend_from_slice(&8_u16.to_le_bytes());
        bytes.extend_from_slice(&32_u16.to_le_bytes());
        bytes.extend_from_slice(b"data");
        bytes.extend_from_slice(&n.to_le_bytes());
        for frame in frames {
            for sample in frame {
                bytes.extend_from_slice(&sample.to_le_bytes());
            }
        }
        let path = self.0.join(name);
        fs::write(&path, bytes).unwrap();
        path
    }
}
impl Drop for Files {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn event(service: &Service) -> Event {
    // Blocks on the event channel; the old 5 s yield-spin both raced and burnt CPU.
    match service.events.recv_timeout(HANG_BOUND) {
        Ok(e) => e,
        Err(error) => panic!("worker result absent after {HANG_BOUND:?}: {error:?}"),
    }
}
fn service() -> (Service, Event) {
    let s = Service::new(48_000, 16, 8_000_000).unwrap();
    let e = event(&s);
    (s, e)
}
fn context(state: &ViewState) -> AudioBatchContext {
    AudioBatchContext {
        project_generation: 0,
        revision: state.revision,
        active_gesture: false,
    }
}
fn submit(
    s: &Service,
    paths: Vec<PathBuf>,
    destination: AudioBatchDestination,
    state: &ViewState,
) -> Arc<AudioBatchControl> {
    let (request, control) = Request::audio_batch(paths, destination, context(state)).unwrap();
    s.submit(request).unwrap();
    control
}
fn new_tracks() -> AudioBatchDestination {
    AudioBatchDestination::NewTracks {
        before_track: None,
        start: 0,
    }
}

#[test]
fn recorded_take_uses_captured_project_and_preserves_one_undo() {
    let files = Files::new();
    let first = files.recorded_wav("first.wav", &[[0.125, 0.25]; 8]);
    let take = files.recorded_wav("take.wav", &[[0.5, 0.25]; 8]);
    let (s, _) = service();
    s.submit(Request::Edit(Command::InsertTrack {
        name: "record".into(),
        before_track: None,
    }))
    .unwrap();
    let added = event(&s);
    let track = added.state.project.tracks()[0].id();
    let (request, _) =
        Request::recorded_audio_take(first, track, 16, context(&added.state)).unwrap();
    s.submit(request).unwrap();
    let before = event(&s);
    assert!(before.error.is_none(), "{:?}", before.error);
    let snapshot = before.state.project.snapshot();

    let mut stale = context(&before.state);
    stale.revision += 1;
    for (path, source) in [
        (take.clone(), stale),
        (files.0.join("missing.wav"), context(&before.state)),
    ] {
        let (request, control) = Request::recorded_audio_take(path, track, 16, source).unwrap();
        s.submit(request).unwrap();
        let refused = event(&s);
        assert!(refused.error.is_some());
        assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
        assert_eq!(refused.state.project.snapshot(), snapshot);
        assert_eq!(refused.state.revision, before.state.revision);
        assert!(take.exists());
    }
    let (request, control) =
        Request::recorded_audio_take(take.clone(), track, 16, context(&before.state)).unwrap();
    s.submit(request).unwrap();
    let mut committed = event(&s);
    assert!(committed.error.is_none(), "{:?}", committed.error);
    assert_eq!(control.progress().phase, AudioBatchPhase::Completed);
    let clips = committed.state.project.track(track).unwrap().clips();
    assert_eq!(clips.len(), 1);
    assert_eq!(clips[0].start(), 16);
    assert_ne!(
        clips[0].asset(),
        before.state.project.track(track).unwrap().clips()[0].asset()
    );
    let after = committed.state.project.snapshot();
    let mut playback = committed.playback.take().unwrap();
    playback.apply(hex_engine::playback::PlaybackCommand::Seek(16));
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut out = [[0.0; 2]; 8];
    playback.render(&mut out).unwrap();
    assert!(out.iter().any(|frame| frame[0] > 0.0 && frame[1] > 0.0));
    s.submit(Request::Undo).unwrap();
    let mut restored = event(&s).state.project.snapshot();
    // IDs are monotonic across undo to prevent reuse; musical state is restored.
    assert!(restored.next_id >= snapshot.next_id);
    restored.next_id = snapshot.next_id;
    assert_eq!(restored, snapshot);
    s.submit(Request::Redo).unwrap();
    assert_eq!(event(&s).state.project.snapshot(), after);
    assert!(take.exists());
}

#[test]
fn worker_batch_publishes_one_revision_playable_ordered_pcm_and_one_undo() {
    let files = Files::new();
    let first = files.wav("Z.WAV", 48_000, &[8192; 8]);
    let second = files.wav("a.wav", 24_000, &[-4096; 4]);
    let (s, initial) = service();
    let control = submit(
        &s,
        vec![first.clone(), second.clone()],
        new_tracks(),
        &initial.state,
    );
    let mut imported = event(&s);
    assert!(imported.request_complete);
    assert!(Arc::ptr_eq(
        imported.audio_batch_complete.as_ref().unwrap(),
        &control
    ));
    assert!(imported.error.is_none());
    assert_eq!(imported.state.revision, 1);
    assert!(imported.state.unsaved);
    assert_eq!(imported.state.waveforms.len(), 2);
    assert_eq!(imported.state.project.tracks().len(), 2);
    let paths: Vec<_> = imported
        .state
        .project
        .tracks()
        .iter()
        .map(|t| {
            imported
                .state
                .project
                .assets()
                .find(|a| a.id() == t.clips()[0].asset())
                .unwrap()
                .path()
                .to_path_buf()
        })
        .collect();
    assert_eq!(paths, vec![first.clone(), second.clone()]);
    let mut playback = imported.playback.take().unwrap();
    playback.apply(hex_engine::playback::PlaybackCommand::Play);
    let mut out = [[0.0; 2]; 8];
    playback.render(&mut out).unwrap();
    assert!(out.iter().all(|f| f[0].is_finite() && f[0] > 0.0));
    assert_eq!(control.progress().phase, AudioBatchPhase::Completed);
    assert_eq!(control.request_cancel(), AudioBatchCancel::TooLate);
    assert!(s.try_event().is_err());
    fs::remove_file(first).unwrap();
    fs::remove_file(second).unwrap();
    s.submit(Request::Undo).unwrap();
    let undone = event(&s);
    assert!(undone.state.project.tracks().is_empty());
    assert!(!undone.state.can_undo);
    assert!(undone.state.can_redo);
    s.submit(Request::Redo).unwrap();
    let redone = event(&s);
    assert_eq!(redone.state.project, imported.state.project);
    assert_eq!(redone.state.waveforms.len(), 2);
}

#[test]
fn worker_bad_middle_or_end_never_publishes_partial_media_dirty_or_playback() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, &[1; 8]);
    let bad = files.0.join("Kick.WAV");
    fs::write(&bad, b"not a wav").unwrap();
    for index in [1, 2] {
        let (s, initial) = service();
        let mut paths = vec![good.clone(); 3];
        paths[index] = bad.clone();
        let control = submit(&s, paths, new_tracks(), &initial.state);
        let refused = event(&s);
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains(&format!("file {}", index + 1))
        );
        let failure = refused.audio_batch_failure.as_ref().unwrap();
        assert_eq!(failure.member, index + 1);
        assert_eq!(failure.path.as_os_str(), bad.as_os_str());
        assert_eq!(fs::read(&failure.path).unwrap(), b"not a wav");
        assert!(
            refused
                .error
                .as_ref()
                .unwrap()
                .contains(bad.to_str().unwrap())
        );
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, 0);
        assert!(!refused.state.unsaved);
        assert!(!refused.state.can_undo);
        assert!(!refused.state.can_redo);
        assert!(refused.state.waveforms.is_empty());
        assert!(refused.playback.is_none());
        assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
        assert_eq!(control.progress().completed_members, index);
    }
}

#[test]
fn queued_cancel_and_stale_context_refuse_before_import_and_complete_correlated() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, &[1; 8]);
    let (s, initial) = service();
    let (request, control) =
        Request::audio_batch(vec![good.clone()], new_tracks(), context(&initial.state)).unwrap();
    assert_eq!(control.request_cancel(), AudioBatchCancel::Requested);
    s.submit(request).unwrap();
    let canceled = event(&s);
    assert!(canceled.error.as_ref().unwrap().contains("canceled"));
    assert_eq!(control.progress().phase, AudioBatchPhase::Canceled);
    assert!(canceled.audio_batch_complete.is_some());
    assert_eq!(canceled.state.revision, 0);
    assert!(canceled.state.waveforms.is_empty());
    let (request, control) =
        Request::audio_batch(vec![good], new_tracks(), context(&initial.state)).unwrap();
    s.submit(Request::Edit(Command::AddTrack {
        name: "changed".into(),
    }))
    .unwrap();
    s.submit(request).unwrap();
    let changed = event(&s);
    let refused = event(&s);
    assert_eq!(refused.state.project, changed.state.project);
    assert_eq!(refused.state.revision, changed.state.revision);
    assert!(refused.error.as_ref().unwrap().contains("changed"));
    assert!(refused.playback.is_none());
    assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
}

#[test]
fn worker_gesture_refusal_keeps_active_gesture_and_current_project() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, &[1; 8]);
    let (s, _) = service();
    s.submit(Request::Edit(Command::AddTrack {
        name: "target".into(),
    }))
    .unwrap();
    let added = event(&s);
    let track = added.state.project.tracks()[0].id();
    s.submit(Request::AddNativeDevice {
        track,
        class: hex_engine::NativeDeviceClass::Effect,
        type_id: Box::new(hex_lattice_contract::DeviceTypeId::from_static(
            "hex.utility",
        )),
        before: None,
    })
    .unwrap();
    let added = event(&s);
    let device = added.state.project.tracks()[0].devices()[0].id();
    s.submit(Request::DeviceGesture(GestureRequest::Begin {
        key: crate::device_gesture::GestureKey {
            generation: 0,
            token: 77,
        },
        target: crate::device_gesture::GestureTarget::DeviceParameter {
            device,
            parameter: hex_lattice_contract::ParameterId::new(1).unwrap(),
        },
    }))
    .unwrap();
    let active = event(&s);
    assert!(active.active_device_gesture.is_some());
    let control = submit(&s, vec![good], new_tracks(), &active.state);
    let refused = event(&s);
    assert!(refused.error.is_some());
    assert_eq!(refused.state.project, active.state.project);
    assert_eq!(refused.state.revision, active.state.revision);
    assert!(refused.active_device_gesture.is_some());
    assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
}

#[test]
fn additive_batch_preserves_target_identity_and_cancels_the_old_keyboard_lease_on_publication() {
    let files = Files::new();
    let audio = files.wav("audio.wav", 48_000, &[1; 8]);
    let (s, _) = service();
    s.submit(Request::Edit(Command::InsertTrackWithDevice {
        name: "typing".into(),
        before_track: None,
        slot: hex_project::TrackDeviceSlot::Instrument,
        state: hex_project::default_instrument_state().expect("fixture instrument state"),
    }))
    .unwrap();
    let added = event(&s);
    let track = added.state.project.tracks()[0].id();
    let device = added
        .state
        .project
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
    input.set_key(0, Some((60, 100, 1))).unwrap();
    s.submit(Request::PrepareComputerKeyboard {
        project_generation: 0,
        revision: added.state.revision,
        track,
        request_id: 7,
        input: Arc::clone(&input),
    })
    .unwrap();
    let armed = event(&s);
    assert!(armed.error.is_none());
    let control = submit(
        &s,
        vec![audio],
        AudioBatchDestination::NewTracks {
            before_track: Some(track),
            start: 0,
        },
        &armed.state,
    );
    let imported = event(&s);
    assert!(imported.error.is_none(), "{:?}", imported.error);
    assert!(!imported.project_replaced);
    assert_eq!(imported.state.revision, armed.state.revision + 1);
    let mut previous = armed.playback.unwrap();
    let mut pcm = [[0.0; 2]; 256];
    previous.render(&mut pcm).unwrap();
    assert!(pcm.iter().flatten().any(|sample| sample.abs() > 1e-6));
    let mut playback = imported.playback.unwrap();
    assert!(playback.prepared_revision().is_some());
    playback.try_inherit_runtime_from(&mut previous).unwrap();
    assert!(
        input.is_cancelled(),
        "ordinary graph replacement cancels the old audition lease"
    );
    playback.render(&mut pcm).unwrap();
    assert_eq!(
        pcm, [[0.0; 2]; 256],
        "stopped playback cannot resurrect the held input"
    );
    assert_eq!(control.progress().phase, AudioBatchPhase::Completed);
    assert_eq!(
        imported
            .state
            .project
            .track(track)
            .unwrap()
            .instrument()
            .unwrap()
            .id(),
        device
    );
}

#[test]
fn worker_conversion_failure_retains_mixed_case_source_without_history_or_media() {
    let files = Files::new();
    let path = files.wav("Kick.WAV", 24_000, &[1; 8]);
    let bytes = fs::read(&path).unwrap();
    let s = Service::new(48_000, 16, 8).unwrap();
    let initial = event(&s);
    let control = submit(&s, vec![path.clone()], new_tracks(), &initial.state);
    let refused = event(&s);
    let failure = refused.audio_batch_failure.as_ref().unwrap();
    assert_eq!(failure.member, 1);
    assert_eq!(failure.path.as_os_str(), path.as_os_str());
    assert_eq!(fs::read(&path).unwrap(), bytes);
    assert_eq!(refused.state.project, initial.state.project);
    assert_eq!(refused.state.revision, initial.state.revision);
    assert!(!refused.state.unsaved);
    assert!(!refused.state.can_undo);
    assert!(!refused.state.can_redo);
    assert!(refused.state.waveforms.is_empty());
    assert!(refused.playback.is_none());
    assert_eq!(control.progress().phase, AudioBatchPhase::Failed);
}

#[test]
fn captured_beats_import_is_one_revision_with_source_local_markers_history_and_roundtrip() {
    let files = Files::new();
    let first = files.wav("long.wav", 48_000, &[8192; 24_000]);
    let second = files.wav("short.wav", 48_000, &[-4096; 12_000]);
    for (warp, members) in [
        (hex_project::AudioImportWarp::Off, vec![first.clone()]),
        (
            hex_project::AudioImportWarp::Beats,
            vec![first.clone(), second.clone()],
        ),
    ] {
        let (s, initial) = service();
        let (mut request, control) =
            Request::audio_batch(members, new_tracks(), context(&initial.state)).unwrap();
        if let Request::ImportAudioBatch { options, .. } = &mut request {
            options.warp = warp;
        }
        s.submit(request).unwrap();
        let mut imported = event(&s);
        assert!(imported.error.is_none(), "{:?}", imported.error);
        assert_eq!(imported.state.revision, initial.state.revision + 1);
        let count = if warp == hex_project::AudioImportWarp::Beats {
            2
        } else {
            1
        };
        assert_eq!(imported.state.project.tracks().len(), count);
        for (index, track) in imported.state.project.tracks().iter().enumerate() {
            let clip = &track.clips()[0];
            let frames = if index == 0 { 24_000 } else { 12_000 };
            assert_eq!(clip.duration(), frames);
            if warp == hex_project::AudioImportWarp::Off {
                assert!(clip.audio_edit().is_none());
            } else {
                let domain = &clip.audio_edit().unwrap().domain;
                assert!(domain.warp_enabled);
                let settings = domain.warp.as_ref().unwrap();
                assert!(matches!(
                    settings.mode,
                    hex_project::WarpMode::Beats {
                        preserve: hex_project::BeatsPreserve::Transients,
                        tail: hex_project::BeatsTail::Forward,
                        decay: 100
                    }
                ));
                assert_eq!(settings.markers[0].source_frame, 0);
                assert_eq!(settings.markers[1].source_frame, frames);
                assert_eq!(settings.markers[0].beat, hex_project::BeatTime::ZERO);
                assert_eq!(
                    settings.markers[1].beat,
                    if index == 0 {
                        hex_project::BeatTime::ONE
                    } else {
                        hex_project::BeatTime::from_raw(hex_project::BeatTime::ONE.raw() / 2)
                    }
                );
            }
        }
        let mut playback = imported.playback.take().unwrap();
        playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut out = [[0.0; 2]; 32];
        playback.render(&mut out).unwrap();
        let expected = if count == 2 { 0.125 } else { 0.25 };
        assert!(out.iter().all(|frame| (frame[0] - expected).abs() < 1e-6));
        assert_eq!(control.progress().phase, AudioBatchPhase::Completed);
        s.submit(Request::Undo).unwrap();
        let undone = event(&s);
        assert!(undone.state.project.tracks().is_empty());
        assert_eq!(undone.state.project.assets().count(), 0);
        assert_eq!(
            undone.state.project.tempo_map(),
            initial.state.project.tempo_map()
        );
        assert!(!undone.state.can_undo);
        s.submit(Request::Redo).unwrap();
        let redone = event(&s);
        assert_eq!(redone.state.project, imported.state.project);
        let document = files.0.join(format!("import-{count}.hex"));
        s.submit(Request::SaveNew(document.clone())).unwrap();
        let saved = event(&s);
        assert!(saved.error.is_none(), "{:?}", saved.error);
        s.submit(Request::Edit(Command::SetProjectTempo {
            tempo: hex_project::Tempo::new(60_000_000).unwrap(),
        }))
        .unwrap();
        let mut changed = event(&s);
        assert!(changed.error.is_none(), "{:?}", changed.error);
        for (old, new) in imported
            .state
            .project
            .tracks()
            .iter()
            .zip(changed.state.project.tracks())
        {
            assert_eq!(
                new.clips()[0].duration(),
                old.clips()[0].duration() * if count == 2 { 2 } else { 1 }
            );
        }
        let mut slower = changed.playback.take().unwrap();
        slower.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut stretched = vec![[0.0; 2]; 24_032];
        slower.render(&mut stretched).unwrap();
        if count == 2 {
            assert!(stretched[24_000..].iter().any(|frame| frame[0] > 0.1));
        } else {
            assert!(stretched[24_000..].iter().all(|frame| *frame == [0.0; 2]));
        }
        s.submit(Request::Open(document)).unwrap();
        let reopened = event(&s);
        assert!(reopened.error.is_none(), "{:?}", reopened.error);
        assert_eq!(reopened.state.project, imported.state.project);
        drop(s);
    }
}

#[test]
fn beats_batch_failure_and_cancel_preserve_the_complete_previous_state() {
    let files = Files::new();
    let good = files.wav("good.wav", 48_000, &[4096; 24_000]);
    let bad = files.0.join("bad.wav");
    fs::write(&bad, b"malformed").unwrap();
    for cancel in [false, true] {
        let (s, initial) = service();
        let (mut request, control) = Request::audio_batch(
            vec![good.clone(), bad.clone()],
            new_tracks(),
            context(&initial.state),
        )
        .unwrap();
        if let Request::ImportAudioBatch { options, .. } = &mut request {
            options.warp = hex_project::AudioImportWarp::Beats;
        }
        if cancel {
            assert_eq!(control.request_cancel(), AudioBatchCancel::Requested);
        }
        s.submit(request).unwrap();
        let refused = event(&s);
        assert!(refused.error.is_some());
        assert_eq!(refused.state.project, initial.state.project);
        assert_eq!(refused.state.revision, initial.state.revision);
        assert!(!refused.state.can_undo);
        assert!(!refused.state.unsaved);
        assert!(refused.state.waveforms.is_empty());
        drop(s);
    }
}

#[test]
fn queued_legacy_off_import_keeps_overwrite_intent_after_an_unrelated_edit() {
    let files = Files::new();
    let original = files.wav("original.wav", 48_000, &[8192; 16]);
    let replacement = files.wav("replacement.wav", 48_000, &[-4096; 8]);
    for overwrite in [false, true] {
        let (s, _) = service();
        s.submit(Request::Import(original.clone())).unwrap();
        let initial = event(&s);
        assert!(initial.error.is_none(), "{:?}", initial.error);
        let track = initial.state.project.tracks()[0].id();
        // This owned legacy request was admitted with auto-warp Off. It carries
        // no late application-preference read or newly imposed revision guard.
        let queued = if overwrite {
            Request::ImportToTrackWithOverwrite {
                path: replacement.clone(),
                track,
                start: 4,
            }
        } else {
            Request::ImportToTrack {
                path: replacement.clone(),
                track,
                start: 4,
            }
        };
        s.submit(Request::Edit(Command::RenameTrack {
            track,
            name: "queued rename".into(),
        }))
        .unwrap();
        s.submit(queued).unwrap();
        let renamed = event(&s);
        assert!(renamed.error.is_none(), "{:?}", renamed.error);
        let mut result = event(&s);
        assert_eq!(renamed.state.revision, initial.state.revision + 1);
        assert_eq!(result.state.project.tracks().len(), 1);
        assert_eq!(result.state.project.tracks()[0].name(), "queued rename");
        // Both legacy spellings already use the canonical Arrangement overwrite
        // policy. Preserve that actual behavior instead of inventing a refusal.
        assert!(result.error.is_none(), "{:?}", result.error);
        assert_eq!(result.state.revision, renamed.state.revision + 1);
        let clips = result.state.project.tracks()[0].clips();
        assert_eq!(clips.len(), 3);
        assert!(clips.iter().all(|clip| {
            clip.audio_edit()
                .is_none_or(|edit| !edit.domain.warp_enabled)
        }));
        let mut playback = result.playback.take().unwrap();
        playback.apply(hex_engine::playback::PlaybackCommand::Play);
        let mut pcm = [[0.0; 2]; 16];
        playback.render(&mut pcm).unwrap();
        assert!(pcm[..4].iter().all(|frame| *frame == [0.25; 2]));
        assert!(pcm[4..12].iter().all(|frame| *frame == [-0.125; 2]));
        assert!(pcm[12..].iter().all(|frame| *frame == [0.25; 2]));
        s.submit(Request::Undo).unwrap();
        let undone = event(&s);
        assert!(undone.error.is_none(), "{:?}", undone.error);
        assert_eq!(
            undone.state.project.tracks(),
            renamed.state.project.tracks()
        );
        s.submit(Request::Redo).unwrap();
        let redone = event(&s);
        assert_eq!(redone.state.project, result.state.project);
        drop(s);
    }
}
