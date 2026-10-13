//! Opt-in computer-key input. Selection establishes the arm once; releases
//! always use the captured physical press owner and actual audio receipts.
use super::EditorView;
use gpui::{Context, KeyDownEvent, Window};
use hex_engine::{
    live_input::TypingTarget,
    typing_protocol::{AppliedResult, CutReason, Focus, Keyboard, ResultKind, Transport},
};
use hex_session::service::Request;
use std::sync::atomic::Ordering::Acquire;
pub(super) const HELP: &str = "arm selected native instrument · play explicitly · canvas keys a w s e d f t g y h u j · c4–b4 · velocity 100 · no midi input";

#[derive(Default)]
pub(super) struct MusicalTyping {
    keyboard: Option<Keyboard>,
    desired: bool,
    target: Option<TypingTarget>,
    last_cut: u64,
    // Real bounded channel for headless GPUI tests; no production fallback or
    // native-output claim. The test drives its paired callback explicitly.
    #[cfg(all(test, feature = "ui-test-support"))]
    test_control: Option<hex_audio::prepared::Control>,
}
impl EditorView {
    fn musical_control(&self) -> Option<&hex_audio::prepared::Control> {
        if let Some(audio) = &self.audio {
            return Some(&audio.control);
        }
        #[cfg(all(test, feature = "ui-test-support"))]
        {
            self.musical_typing.test_control.as_ref()
        }
        #[cfg(not(all(test, feature = "ui-test-support")))]
        None
    }
    pub(super) fn cut_musical_typing(&mut self, reason: CutReason) {
        if !self.musical_typing.desired {
            return;
        }
        self.musical_typing.desired = false;
        if let Some(keyboard) = &mut self.musical_typing.keyboard {
            let cut = keyboard.cut(reason);
            self.musical_typing.last_cut = self.musical_typing.last_cut.max(cut.through_epoch);
            if let Some(control) = self.musical_control() {
                control.cut_typing_immediately(cut.through_epoch);
            }
        }
    }
    pub(super) fn toggle_musical_typing(&mut self, cx: &mut Context<Self>) {
        if self.musical_typing.desired {
            let old = self.musical_typing.keyboard.as_ref().map(Keyboard::epoch);
            self.cut_musical_typing(CutReason::Disarm);
            if let (Some(target), Some(epoch)) = (self.musical_typing.target, old) {
                self.enqueue(Request::DisarmMusicalTyping {
                    project_generation: target.project_generation,
                    epoch,
                });
            }
            cx.notify();
            return;
        }
        if !super::device_panel::finalize_controls(self, cx) {
            return;
        }
        let target = self.selected_track.and_then(|track| {
            self.state().and_then(|state| {
                state
                    .project
                    .track(track)
                    .and_then(|row| row.instrument())
                    .map(|device| TypingTarget {
                        project_generation: self.project_generation,
                        project_revision: state.revision,
                        track,
                        device: device.id(),
                    })
            })
        });
        let Some(target) = target else {
            self.error =
                Some("select a track with a native instrument before arming musical typing".into());
            return;
        };
        let Some(control) = self.musical_control() else {
            self.error = Some("musical typing needs an active audio output".into());
            return;
        };
        let Ok(owner) = target.owner() else {
            self.error = Some("musical typing owner identity exhausted".into());
            return;
        };
        let keyboard = match self.musical_typing.keyboard {
            Some(mut keyboard) => {
                let Some(snapshot) = control.status.typing.snapshot() else {
                    self.error = Some("waiting for the audio input cut acknowledgement".into());
                    return;
                };
                if keyboard.rearm(owner, snapshot.cut_through_epoch).is_err() {
                    self.error = Some(
                        "waiting for the previous musical typing cut; retry after audio recovery"
                            .into(),
                    );
                    return;
                }
                keyboard
            }
            None => match Keyboard::new(owner, 1) {
                Ok(keyboard) => keyboard,
                Err(_) => return,
            },
        };
        let epoch = keyboard.epoch();
        let Some(bridge_bytes) = hex_audio::prepared::input_bridge_storage_bytes() else {
            self.error = Some("musical typing storage accounting overflow".into());
            return;
        };
        if self.enqueue(Request::ArmMusicalTyping {
            target,
            epoch,
            bridge_bytes,
        }) {
            self.musical_typing.keyboard = Some(keyboard);
            self.musical_typing.target = Some(target);
            self.musical_typing.desired = true;
            self.status = "musical typing requested · waiting for actual audio admission · play explicitly to hear notes".into();
        }
        cx.notify();
    }
    pub(super) fn musical_typing_label(&self) -> &'static str {
        if !self.musical_typing.desired {
            return "typing · off";
        }
        if self
            .musical_control()
            .and_then(|control| control.status.typing.snapshot())
            .is_some_and(|snapshot| {
                self.musical_typing
                    .keyboard
                    .as_ref()
                    .is_some_and(|keyboard| {
                        snapshot.armed
                            && snapshot.owner == keyboard.owner()
                            && snapshot.epoch == keyboard.epoch()
                    })
            })
        {
            "typing · ready"
        } else {
            "typing · waiting"
        }
    }
    pub(super) fn poll_musical_typing(&mut self) -> bool {
        let Some(control) = self.musical_control() else {
            self.cut_musical_typing(CutReason::AudioDisconnected);
            return false;
        };
        control.cut_typing_immediately(self.musical_typing.last_cut);
        if control.status.device_fault.load(Acquire)
            || control.status.render_fault.load(Acquire)
            || control.status.shutdown_complete.load(Acquire)
        {
            let changed = self.musical_typing.desired;
            self.cut_musical_typing(CutReason::AudioDisconnected);
            return changed;
        }
        let Some(snapshot) = control.status.typing.snapshot() else {
            return false;
        };
        let Some(keyboard) = &mut self.musical_typing.keyboard else {
            return false;
        };
        for application in snapshot.applications.into_iter().flatten() {
            keyboard.result(AppliedResult {
                action: application.action,
                kind: match application.result {
                    Ok(()) => ResultKind::OnApplied,
                    Err(reason) => ResultKind::Refused(reason),
                },
            });
        }
        if self.musical_typing.desired && snapshot.cut_through_epoch >= keyboard.epoch() {
            self.cut_musical_typing(CutReason::Stop);
            return true;
        }
        false
    }
    pub(super) fn musical_key_down(&mut self, event: &KeyDownEvent, window: &Window) -> bool {
        let pending_seek = self.seek_pending();
        let control = if let Some(audio) = &mut self.audio {
            Some(&mut audio.control)
        } else {
            #[cfg(all(test, feature = "ui-test-support"))]
            {
                self.musical_typing.test_control.as_mut()
            }
            #[cfg(not(all(test, feature = "ui-test-support")))]
            {
                None
            }
        };
        let Some(control) = control else {
            return false;
        };
        let Some(keyboard) = &mut self.musical_typing.keyboard else {
            return false;
        };
        let admitted = self.musical_typing.desired
            && !pending_seek
            && control.status.typing.snapshot().is_some_and(|snapshot| {
                snapshot.armed
                    && snapshot.owner == keyboard.owner()
                    && snapshot.epoch == keyboard.epoch()
            });
        let modifiers = event.keystroke.modifiers;
        let focus = Focus {
            active_window: window.is_window_active(),
            text_editing: !self.focus.is_focused(window) && !self.midi_focus.is_focused(window),
            modal: self.theme_settings.open,
            modified: modifiers.control || modifiers.alt || modifiers.platform || modifiers.shift,
        };
        let transport = if control.status.playing.load(Acquire) {
            Transport::Playing
        } else {
            Transport::Paused
        };
        let owned = keyboard.press_state(&event.keystroke.key).is_some();
        let action = match keyboard.key_down(
            &event.keystroke.key,
            event.is_held,
            focus,
            transport,
            admitted,
        ) {
            Ok(Some(action)) => action,
            _ => return owned && !focus.text_editing && !focus.modal && !focus.modified,
        };
        let ticket = control.typing_with_ticket(action).ok();
        if let Some(cut) = keyboard.queue_result(action, ticket) {
            control.cut_typing_immediately(cut.through_epoch);
        }
        true
    }
    pub(super) fn musical_key_up(&mut self, key: &str) -> bool {
        let Some(keyboard) = &mut self.musical_typing.keyboard else {
            return false;
        };
        let Some(action) = keyboard.key_up(key) else {
            return false;
        };
        let control = if let Some(audio) = &mut self.audio {
            Some(&mut audio.control)
        } else {
            #[cfg(all(test, feature = "ui-test-support"))]
            {
                self.musical_typing.test_control.as_mut()
            }
            #[cfg(not(all(test, feature = "ui-test-support")))]
            {
                None
            }
        };
        let ticket = control.and_then(|control| control.typing_with_ticket(action).ok());
        if let Some(cut) = keyboard.queue_result(action, ticket) {
            self.musical_typing.desired = false;
            self.musical_typing.last_cut = self.musical_typing.last_cut.max(cut.through_epoch);
            if let Some(control) = self.musical_control() {
                control.cut_typing_immediately(cut.through_epoch);
            }
        }
        true
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Empty, Entity, KeyUpEvent, Keystroke, TestAppContext, VisualTestContext,
    };
    use hex_audio::prepared;
    use hex_engine::PlaybackCommand;
    use hex_project::Command;

    fn wait(cx: &mut TestAppContext, handle: gpui::AnyWindowHandle, editor: &Entity<EditorView>) {
        for _ in 0..500 {
            cx.update_window(handle, |_, _, cx| {
                editor.update(cx, |view, cx| {
                    view.tick(cx);
                    cx.notify();
                })
            })
            .unwrap();
            if editor.read_with(cx, |view, _| {
                view.worker_requests_pending == 0 && view.pending_requests.is_empty()
            }) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        panic!("typing worker did not complete");
    }
    fn key(
        cx: &mut TestAppContext,
        handle: gpui::AnyWindowHandle,
        name: &str,
        up: bool,
        repeat: bool,
    ) {
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let keystroke = Keystroke::parse(name).unwrap();
        if up {
            visual.simulate_event(KeyUpEvent { keystroke });
        } else {
            visual.simulate_event(KeyDownEvent {
                keystroke,
                is_held: repeat,
            });
        }
    }
    fn step(callback: &mut prepared::Callback, frames: &mut [[f32; 2]]) {
        callback.begin_block();
        callback.render(frames).unwrap();
        callback.finish_typing_output(true);
    }
    #[gpui::test]
    fn rendered_capture_uses_actual_channel_ack_original_owner_and_text_focus_release(
        cx: &mut TestAppContext,
    ) {
        cx.update(crate::editor::bind_editor_keys);
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let (control, mut callback) = prepared::channel(48_000).unwrap();
        let editor = cx
            .update_window(handle, |_, window, cx| {
                window.activate_window();
                let service = hex_session::service::Service::new(48_000, 32, 0).unwrap();
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new(
                        window,
                        cx,
                        service,
                        Err("headless typing channel; no native output".into()),
                    )
                });
                editor.update(cx, |view, cx| {
                    view.musical_typing.test_control = Some(control);
                    view.enqueue(Request::Edit(Command::AddMidiTrack {
                        name: "physical owner".into(),
                    }));
                    view.focus.focus(window);
                    cx.notify();
                });
                editor
            })
            .unwrap();
        wait(cx, handle, &editor);
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = Some(view.state().unwrap().project.tracks()[0].id());
                view.focus.focus(window);
                cx.notify();
            })
        })
        .unwrap();
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        let bounds = visual
            .debug_bounds(HELP)
            .expect("discoverable rendered arm button");
        assert_eq!(bounds.size.width, gpui::px(116.0));
        visual.simulate_click(bounds.center(), gpui::Modifiers::default());
        editor.read_with(cx, |view, _| {
            assert_eq!(view.musical_typing_label(), "typing · waiting")
        });
        wait(cx, handle, &editor);
        editor.update(cx, |view, _| {
            let pending = view
                .pending_playback
                .take()
                .expect("real worker prepared arm");
            assert!(pending.playback.typing_admission().unwrap().armed);
            let control = view.musical_typing.test_control.as_mut().unwrap();
            control.publish(pending.playback).unwrap();
        });
        let mut pcm = [[0.0; 2]; 128];
        step(&mut callback, &mut pcm);
        assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
        editor.read_with(cx, |view, _| {
            assert_eq!(view.musical_typing_label(), "typing · ready");
        });
        // Paused input does not secretly start playback or manufacture an On.
        key(cx, handle, "a", false, false);
        key(cx, handle, "a", true, false);
        step(&mut callback, &mut pcm);
        assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
        editor.update(cx, |view, _| {
            view.musical_typing
                .test_control
                .as_mut()
                .unwrap()
                .command(PlaybackCommand::Play)
                .unwrap();
        });
        step(&mut callback, &mut pcm);
        cx.refresh().unwrap();
        cx.run_until_parked();
        key(cx, handle, "a", false, false);
        editor.read_with(cx, |view, _| {
            assert!(matches!(
                view.musical_typing.keyboard.unwrap().press_state("a"),
                Some(hex_engine::typing_protocol::PressState::Queued(_))
            ))
        });
        key(cx, handle, "a", false, true);
        step(&mut callback, &mut pcm);
        assert!(pcm.iter().any(|frame| frame[0] != 0.0));
        editor.update(cx, |view, _| {
            view.poll_musical_typing();
            assert_eq!(
                view.musical_typing.keyboard.unwrap().press_state("a"),
                Some(hex_engine::typing_protocol::PressState::Applied)
            );
        });
        // Move focus to an unrelated input handle and selection away from the
        // captured device. Root capture must still send the ORIGINAL key-up.
        cx.update_window(handle, |_, window, cx| {
            editor.update(cx, |view, cx| {
                view.selected_track = None;
                let text = cx.focus_handle();
                text.focus(window);
                // Retain focus handle for the following dispatched event.
                view.browser.open = true;
                view.browser.search.read(cx).focus_handle().focus(window);
                cx.notify();
            })
        })
        .unwrap();
        key(cx, handle, "s", false, false);
        key(cx, handle, "s", true, false);
        key(cx, handle, "A", true, false);
        step(&mut callback, &mut pcm);
        editor.read_with(cx, |view, _| {
            let snapshot = view
                .musical_control()
                .unwrap()
                .status
                .typing
                .snapshot()
                .unwrap();
            assert!(snapshot.last_application.unwrap().result.is_ok());
            assert!(matches!(
                snapshot.last_application.unwrap().action,
                hex_engine::typing_protocol::Action::Off { .. }
            ));
        });
        for _ in 0..24 {
            step(&mut callback, &mut pcm);
        }
        assert!(pcm.iter().all(|frame| *frame == [0.0; 2]));
        visual.deactivate_window();
        step(&mut callback, &mut pcm);
        editor.read_with(cx, |view, _| {
            assert_eq!(view.musical_typing_label(), "typing · off");
            assert!(
                view.musical_control()
                    .unwrap()
                    .status
                    .typing
                    .snapshot()
                    .unwrap()
                    .cut_through_epoch
                    >= 1
            );
        });
    }
}
