use super::*;
use crate::import_batch_decode::{AudioBatchCancel, AudioBatchPhase};
use std::{
    fs,
    sync::atomic::AtomicU64,
    time::{Duration, Instant},
};
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
}
impl Drop for Files {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn event(service: &Service) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        match service.try_event() {
            Ok(e) => return e,
            Err(TryRecvError::Empty) if Instant::now() < deadline => thread::yield_now(),
            Err(error) => panic!("worker result absent: {error:?}"),
        }
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
    s.submit(Request::Edit(Command::AddMidiTrack {
        name: "typing".into(),
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
