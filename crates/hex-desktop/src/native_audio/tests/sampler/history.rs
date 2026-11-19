//! Actual canonical history through the production fence and sealed native cell.
use super::*;
use hex_engine::SamplerParameter;
use hex_session::service::{SamplerHistoryEdit, ViewState};
use std::sync::atomic::Ordering;
pub(super) fn history(state: &ViewState, redo: bool) -> SamplerHistoryEdit {
    SamplerHistoryEdit {
        request_id: 42,
        project_generation: state
            .canonical_audio_source
            .as_ref()
            .unwrap()
            .context()
            .project_generation,
        revision: state.revision,
        redo,
        cancel: Arc::new(AtomicBool::new(false)),
    }
}
pub(super) fn complete(h: &mut Harness, request: Request) -> Arc<ViewState> {
    let old = h.audio.as_ref().unwrap().control.output_identity().unwrap();
    let initial = h.run(request);
    assert!(initial.sampler_edit_complete.is_none());
    assert!(initial.request_complete);
    h.render();
    let accepted = h.next();
    assert!(accepted.error.is_none(), "{:?}", accepted.error);
    assert!(accepted.sampler_edit_complete.is_some());
    assert!(!accepted.request_complete);
    assert_eq!(accepted.state.revision, initial.state.revision + 1);
    assert!(accepted.state.unsaved);
    assert!(
        accepted
            .state
            .source_overviews
            .shares_storage_with(&initial.state.source_overviews)
    );
    h.open_sampler(old);
    h.applied(h.context.unwrap());
    let observed = h.observation.lock().unwrap();
    let (sealed, drained) = observed.gain_cells.last().unwrap();
    assert_eq!(sealed, drained);
    accepted.state
}
fn pcm(h: &mut Harness, expected: &[[f32; 2]]) {
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    let rendered = h.render();
    assert_eq!(&rendered[..expected.len()], expected);
    h.audio.as_ref().unwrap().control.stop();
    assert_eq!(h.render(), [[0.0; 2]; 64]);
}
#[test]
fn sampler_history_orders_real_edges_clears_redo_and_refuses_load_boundary_without_consuming() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (files, mut h, track, device, original) = gain::current(12);
    let half = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(gain::edit(
            &original, track, device,
        ))),
    );
    let mut pitch = gain::edit(&half, track, device);
    pitch.parameter = SamplerParameter::Transpose;
    pitch.value = -12.0;
    let low = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(pitch)),
    );
    // A440, 8k original / 16k output: default step .5, transpose -12 => .25.
    pcm(
        &mut h,
        &[[0.0625, -0.125], [0.109375, -0.09375], [0.15625, -0.0625]],
    );
    let half_again = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(history(
            &low, false,
        ))),
    );
    assert_eq!(half_again.project, half.project);
    pcm(&mut h, &[[0.0625, -0.125], [0.15625, -0.0625], [0.25, 0.0]]);
    let restored = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(history(
            &half_again,
            false,
        ))),
    );
    assert_eq!(restored.project, original.project);
    gain::original_pcm(&mut h);
    let identity = h.audio.as_ref().unwrap().control.output_identity();
    let cells = h.observation.lock().unwrap().gain_cells.len();
    for _ in 0..2 {
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SamplerEdit(
                hex_session::service::SamplerEdit::History(history(&restored, false)),
            ))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.as_ref().unwrap().contains("history step"));
        assert_eq!(refused.sampler_edit_complete, Some(42));
        gain::unchanged(&restored, &refused);
    }
    assert_eq!(
        h.audio.as_ref().unwrap().control.output_identity(),
        identity
    );
    assert_eq!(h.observation.lock().unwrap().gain_cells.len(), cells);
    let redone = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(history(
            &restored, true,
        ))),
    );
    assert_eq!(redone.project, half.project);
    let mut edit = gain::edit(&redone, track, device);
    edit.value = 0.75;
    let changed = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(edit)),
    );
    assert!(!changed.can_redo);
    let no_op = h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::History(history(&changed, true)),
    ));
    gain::unchanged(&changed, &no_op);
    assert_eq!(no_op.sampler_edit_complete, Some(42));
    let path = files.0.join("history.hex");
    h.run(Request::SaveNew(path.clone()));
    let saved = hex_storage::load(&path).unwrap();
    assert_eq!(
        saved.device(device).unwrap().state(),
        changed.project.device(device).unwrap().state()
    );
}
#[test]
fn sampler_history_stale_cancel_playing_and_cancel_after_seal_preserve_history_and_pcm() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, original) = gain::current(12);
    let before = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(gain::edit(
            &original, track, device,
        ))),
    );
    for case in 0..4 {
        let mut edit = history(&before, false);
        match case {
            0 => edit.revision += 1,
            1 => edit.project_generation += 1,
            2 => edit.request_id = 0,
            _ => edit.cancel.store(true, Ordering::Release),
        }
        h.service
            .as_ref()
            .unwrap()
            .submit(Request::SamplerEdit(
                hex_session::service::SamplerEdit::History(edit),
            ))
            .unwrap();
        let refused = h.next();
        assert!(refused.error.is_some());
        gain::unchanged(&before, &refused);
    }
    h.audio
        .as_mut()
        .unwrap()
        .control
        .command(PlaybackCommand::Play)
        .unwrap();
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::History(history(&before, false)),
    ));
    assert_eq!(h.render()[0], [0.0625, -0.125]);
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
    let edit = history(&before, false);
    h.observation.lock().unwrap().cancel_edit_after_seal = Some(Arc::clone(&edit.cancel));
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::History(edit),
    ));
    h.render();
    let refused = h.next();
    assert!(refused.error.as_ref().unwrap().contains("cancelled"));
    gain::unchanged(&before, &refused);
    pcm(&mut h, &[[0.0625, -0.125], [0.15625, -0.0625], [0.25, 0.0]]);
    let restored = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::History(history(
            &before, false,
        ))),
    );
    assert_eq!(restored.project, original.project);
    gain::original_pcm(&mut h);
}

#[test]
fn sampler_history_occupied_refusal_and_new_cancel_share_the_single_pending_fence() {
    let _serial = NATIVE_FIXTURE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let (_files, mut h, track, device, original) = gain::current(12);
    let before = complete(
        &mut h,
        Request::SamplerEdit(hex_session::service::SamplerEdit::Parameter(gain::edit(
            &original, track, device,
        ))),
    );
    let edit = history(&before, false);
    let cancel = Arc::clone(&edit.cancel);
    h.run(Request::SamplerEdit(
        hex_session::service::SamplerEdit::History(edit),
    ));
    h.service
        .as_ref()
        .unwrap()
        .submit(Request::SamplerEdit(
            hex_session::service::SamplerEdit::History(history(&before, false)),
        ))
        .unwrap();
    let occupied = h.next();
    assert!(
        occupied
            .error
            .as_ref()
            .unwrap()
            .contains("already preparing")
    );
    gain::unchanged(&before, &occupied);
    let replaced = h.run(Request::NewWithAudioMode {
        mode: DocumentAudioMode::Resident,
        reservation: None,
    });
    assert!(cancel.load(Ordering::Acquire));
    assert_eq!(replaced.sampler_edit_complete, Some(42));
    assert!(replaced.project_replaced);
    assert!(replaced.state.project.tracks().is_empty());
    h.open();
    // Opened proves fresh native construction, not delivery of the worker's
    // next native_sampler_input observation. Wait for that actual refreshed
    // hold instead of asserting against the previous sampler's cached value.
    let deadline = Instant::now() + h.wait;
    loop {
        assert_eq!(h.render(), [[0.0; 2]; 64]);
        if h.observation.lock().unwrap().native_hold.frames == 0 {
            break;
        }
        assert!(Instant::now() < deadline, "old sampler hold did not retire");
        std::thread::sleep(Duration::from_millis(1));
    }
}
