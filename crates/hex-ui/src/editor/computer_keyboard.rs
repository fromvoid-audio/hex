//! Transient physical-key state; never edits the canonical project or history.
mod state;
use state::NOTE_KEYS;
pub(super) use state::{ComputerKeyboard, HeldKey};

impl super::EditorView {
    pub(super) fn retire_computer_keyboard_audio(&mut self) {
        if let Some(input) = self.computer_keyboard.input.take() {
            input.cancel();
        }
        self.computer_keyboard.held.fill(None);
        self.computer_keyboard.target = None;
        self.computer_keyboard.prepared = false;
    }

    pub(super) fn cancel_computer_keyboard(&mut self) {
        self.midi_audition.clear();
        self.retire_computer_keyboard_audio();
        self.computer_keyboard.release_all();
    }

    /// Priority Stop retains only a still-valid ordinary output-bound input root. The output
    /// endpoint revokes its active prefix; fresh physical input can reuse the already admitted
    /// graph without waiting for a publication slot. Other lifecycle cancellation stays terminal.
    pub(super) fn stop_computer_keyboard(&mut self) {
        self.stop_controller_notes();
        self.invalidate_hardware_step();
        self.midi_audition.clear();
        if self.reusable_worker_keyboard_input(self.worker_owns_keyboard_publication())
            && self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.is_output_bound())
        {
            if let Some(input) = &self.computer_keyboard.input {
                input.clear_keys_on_control();
            }
            self.computer_keyboard.release_all();
        } else {
            self.cancel_computer_keyboard();
        }
        self.computer_keyboard.suspended = true;
    }

    pub(super) fn toggle_computer_keyboard(
        &mut self,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        if self.document_audio_mode == hex_session::DocumentAudioMode::VerifiedStreaming {
            self.error =
                Some("computer keyboard is unavailable for verified streaming audio".into());
            cx.notify();
            return;
        }
        if self.controller_wants_live_notes() {
            if let Some(input) = &self.computer_keyboard.input {
                input.clear_keys_on_control();
            }
            self.computer_keyboard.release_all();
        } else {
            self.cancel_computer_keyboard();
        }
        self.computer_keyboard.toggle();
        self.computer_keyboard.suspended = false;
        if !self.midi_step.enabled || !self.midi_focus.is_focused(window) {
            self.focus.focus(window);
        }
        self.tick_computer_keyboard();
        cx.notify();
    }

    pub(super) fn worker_owns_keyboard_publication(&self) -> bool {
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && self
                .audio
                .as_ref()
                .is_some_and(|audio| audio.control.worker_owns_graph_publication())
    }

    // Worker transactions reprepare this physical input against each accepted instrument.
    // A model revision is not an input identity; selected track, generation and cancellation are.
    fn reusable_worker_keyboard_input(&self, worker_owned: bool) -> bool {
        worker_owned
            && self.computer_keyboard.input.as_ref().is_some_and(|input| {
                !input.is_cancelled()
                    && !input.is_note_audition()
                    && (!input.is_output_bound()
                        || self.audio.as_ref().is_some_and(|audio| {
                            audio.control.computer_keyboard_input_matches_output(input)
                        }))
            })
            && self.state.as_ref().is_some_and(|state| {
                self.computer_keyboard
                    .target
                    .is_some_and(|(generation, revision, track)| {
                        generation == self.project_generation
                            && revision <= state.revision
                            && self.selected_track == Some(track)
                            && state.project.track(track).is_some_and(|track| {
                                track.instrument().is_some_and(|device| !device.bypassed())
                            })
                    })
            })
    }

    pub(super) fn tick_computer_keyboard(&mut self) -> bool {
        if self.controller_wants_live_notes() {
            let changed =
                self.tick_computer_keyboard_with_policy(self.worker_owns_keyboard_publication());
            return self.tick_midi_audition().unwrap_or(false) || changed;
        }
        if let Some(changed) = self.tick_midi_audition() {
            return changed;
        }
        self.tick_computer_keyboard_with_policy(self.worker_owns_keyboard_publication())
    }

    fn tick_computer_keyboard_with_policy(&mut self, worker_owned: bool) -> bool {
        if (!self.computer_keyboard.enabled && !self.controller_wants_live_notes())
            || self.computer_keyboard.suspended
        {
            return false;
        }
        if self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.rejected() && input.is_cancelled())
        {
            // Voice-pressure refusal terminally cancels the runtime input. Preserve its actual
            // callback rejection before releasing that root; cancellation alone is silent.
            self.error = Some("computer keyboard note refused · unsupported pitch or voice capacity · release keys and toggle keyboard to retry".into());
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
            return true;
        }
        if self.controller_wants_live_notes()
            && self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.live_notes().is_none())
        {
            self.retire_computer_keyboard_audio();
            self.computer_keyboard.release_all();
        }
        let retain_input = self.reusable_worker_keyboard_input(worker_owned);
        if worker_owned
            && self.computer_keyboard.input.as_ref().is_some_and(|input| {
                input.is_output_bound()
                    && !self.audio.as_ref().is_some_and(|audio| {
                        audio.control.computer_keyboard_input_matches_output(input)
                    })
            })
        {
            // Recovery installs a new endpoint clock. End the old physical root before
            // preparing against that output, even when the canonical track/revision is equal.
            self.cancel_computer_keyboard();
        }
        // The resident worker orders a fresh physical activation after Stop restoration.
        // Waiting for its callback here would discard the newer press; legacy UI-owned
        // publication still requires restoration to apply before preparing another graph.
        if (self.device_gesture_pending()
            || self.prepared_seek.is_some()
            || (self.stopped_playback_restore.is_some() && !worker_owned))
            && !retain_input
        {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
            return true;
        }
        let target = self.state.as_ref().and_then(|state| {
            self.selected_track
                .map(|track| (self.project_generation, state.revision, track))
        });
        let cancelled = self
            .computer_keyboard
            .input
            .as_ref()
            .is_some_and(|input| input.is_cancelled());
        if cancelled {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
            return true;
        }
        let changed = retain_input && target != self.computer_keyboard.target;
        if changed {
            self.computer_keyboard.target = target;
        }
        if target == self.computer_keyboard.target && !cancelled {
            if self
                .computer_keyboard
                .input
                .as_ref()
                .is_some_and(|input| input.rejected())
            {
                let message = "computer keyboard note refused · unsupported pitch or voice capacity · release keys and toggle keyboard to retry";
                if self.error.as_deref() != Some(message) {
                    self.error = Some(message.into());
                    return true;
                }
                return changed;
            }
            return changed;
        }
        let same_physical_target = self.midi_step.enabled
            && target
                .zip(self.computer_keyboard.target)
                .is_some_and(|(new, old)| new.0 == old.0 && new.2 == old.2);
        if same_physical_target {
            self.retire_computer_keyboard_audio();
        } else if self.controller_wants_live_notes() {
            self.retire_computer_keyboard_audio();
            self.computer_keyboard.release_all();
        } else {
            self.cancel_computer_keyboard();
        }
        let Some((generation, revision, track)) = target else {
            return false;
        };
        self.computer_keyboard.target = target;
        if self
            .state
            .as_ref()
            .and_then(|state| state.project.track(track))
            .is_none_or(|track| track.instrument().is_none_or(|device| device.bypassed()))
        {
            self.error =
                Some("select a track with an enabled instrument for the computer keyboard".into());
            return true;
        }
        let Some(request_id) = self.computer_keyboard.request_id.checked_add(1) else {
            self.error = Some("computer keyboard request identity exhausted".into());
            return true;
        };
        self.computer_keyboard.request_id = request_id;
        let input = if worker_owned {
            match self
                .audio
                .as_ref()
                .ok_or(hex_engine::Invalid("audio output is unavailable"))
                .and_then(|audio| audio.control.new_computer_keyboard_input_on_control())
            {
                Ok(input) => input,
                Err(error) => {
                    self.error = Some(error.to_string().into());
                    self.cancel_computer_keyboard();
                    self.computer_keyboard.suspended = true;
                    return true;
                }
            }
        } else {
            hex_engine::computer_keyboard::ComputerKeyboardInput::default()
        };
        let input = if self.controller_wants_live_notes() {
            match input.with_live_notes() {
                Ok(input) => input,
                Err(error) => {
                    self.error = Some(error.to_string().into());
                    return true;
                }
            }
        } else {
            input
        };
        let input = if self.midi_recorder.preparing() {
            match input.with_recording_support() {
                Ok(input) => input,
                Err(reason) => {
                    self.error = Some(reason.to_string().into());
                    self.computer_keyboard.suspended = true;
                    return true;
                }
            }
        } else {
            input
        };
        let input = std::sync::Arc::new(input);
        self.computer_keyboard.prepared = false;
        self.computer_keyboard.input = Some(std::sync::Arc::clone(&input));
        if !self.enqueue(hex_session::service::Request::PrepareComputerKeyboard {
            project_generation: generation,
            revision,
            request_id,
            track,
            input,
        }) {
            self.cancel_computer_keyboard();
            self.computer_keyboard.suspended = true;
        }
        true
    }

    pub(super) fn computer_keyboard_accepts(
        &self,
        key: &str,
        modifiers: gpui::Modifiers,
        window: &gpui::Window,
        _cx: &gpui::App,
    ) -> bool {
        !modifiers.modified()
            && !self.theme_settings.open
            && !self.tuning.open
            && self.context_menu.is_none()
            && !self.opening_project
            && !self.close_prompt_open
            && (self.focus.is_focused(window) || self.midi_focus.is_focused(window))
            && (key == "m"
                || (self.computer_keyboard.enabled
                    && (NOTE_KEYS.contains(&key) || matches!(key, "z" | "x" | "c" | "v"))))
    }

    pub(super) fn computer_keyboard_down(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut gpui::Window,
        cx: &mut gpui::Context<Self>,
    ) {
        let key = event.keystroke.key.as_str();
        if !self.computer_keyboard_accepts(key, event.keystroke.modifiers, window, cx) {
            return;
        }
        if key == "m" {
            if !event.is_held {
                self.toggle_computer_keyboard(window, cx);
            }
        } else {
            if self.computer_keyboard.suspended && event.is_held {
                // Stop already revoked this prefix. Autorepeat cannot resume it, but must
                // not destroy a retained root needed by a newer physical press. Focus/disable
                // cancellation has already ended its input independently.
                cx.stop_propagation();
                return;
            }
            let deferred_audition = if !event.is_held && NOTE_KEYS.contains(&key) {
                self.pause_midi_audition_for_keyboard()
            } else {
                None
            };
            let worker_owned = self.worker_owns_keyboard_publication();
            let retain_input = self.reusable_worker_keyboard_input(worker_owned);
            if (self.device_gesture_pending()
                || self.prepared_seek.is_some()
                || (self.stopped_playback_restore.is_some() && !worker_owned))
                && !retain_input
            {
                self.cancel_computer_keyboard();
                self.computer_keyboard.suspended = true;
                cx.stop_propagation();
                return;
            }
            if self.computer_keyboard.suspended
                && !event.is_held
                && retain_input
                && let Some(input) = self
                    .computer_keyboard
                    .input
                    .as_ref()
                    .filter(|input| input.is_output_bound())
                && let Err(error) = input.ensure_output_activation_on_control()
            {
                self.error = Some(error.to_string().into());
                self.cancel_computer_keyboard();
                self.computer_keyboard.suspended = true;
                cx.stop_propagation();
                return;
            }
            self.computer_keyboard.suspended = false;
            self.tick_computer_keyboard();
            if self.computer_keyboard.key_down(key, event.is_held) {
                self.publish_computer_keyboard();
            }
            self.defer_paused_midi_audition(deferred_audition);
            cx.notify();
        }
        cx.stop_propagation();
    }

    pub(super) fn computer_keyboard_up(&mut self, key: &str) -> bool {
        let released = self.computer_keyboard.key_up(key);
        if released {
            self.publish_computer_keyboard();
        }
        released
    }

    pub(super) fn publish_computer_keyboard(&mut self) {
        let Some(input) = &self.computer_keyboard.input else {
            return;
        };
        if input.is_note_audition() {
            return;
        }
        for (key, held) in self.computer_keyboard.held.iter().enumerate() {
            if let Err(error) = input.set_key(
                key,
                held.map(|held| {
                    (
                        held.pitch,
                        held.velocity,
                        self.computer_keyboard.tokens[key],
                    )
                }),
            ) {
                self.error = Some(error.to_string().into());
                input.cancel();
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn worker_edits_keep_physical_keys_but_stop_and_invalid_targets_cancel(
        cx: &mut gpui::TestAppContext,
    ) {
        use hex_project::{Command, Created, Editor, Project};
        use std::sync::Arc;
        fn state(project: Project, revision: u64) -> Arc<hex_session::service::ViewState> {
            Arc::new(hex_session::service::ViewState {
                canonical_audio_source: None,
                comparison_audio: None,
                project: Arc::new(project),
                waveforms: Default::default(),
                source_overviews: Default::default(),
                audio_clip_waveforms: Default::default(),
                midi_preview: Arc::default(),
                gain_preview: Ok(Arc::default()),
                timeline_frames: 4096,
                can_undo: false,
                can_redo: false,
                revision,
                unsaved: true,
                document_path: None,
            })
        }
        let mut authored = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = authored
            .execute(Command::AddMidiTrack {
                name: "keys".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        authored
            .execute(Command::InsertInstrument {
                track,
                state: hex_engine::default_native_device_state_for_class(
                    hex_engine::NativeDeviceClass::Instrument,
                    hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sine"),
                )
                .unwrap(),
            })
            .unwrap();
        let original = authored.project().clone();
        let sampler = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Instrument,
            hex_lattice_contract::DeviceTypeId::from_static("hex.instrument.sampler"),
        )
        .unwrap();
        authored
            .execute(Command::RemoveInstrument { track })
            .unwrap();
        authored
            .execute(Command::InsertInstrument {
                track,
                state: sampler,
            })
            .unwrap();
        let replacement = authored.project().clone();
        let device = replacement.track(track).unwrap().instrument().unwrap().id();
        authored
            .execute(Command::SetDeviceBypass {
                device,
                bypassed: true,
            })
            .unwrap();
        let bypassed = authored.project().clone();
        authored
            .execute(Command::RemoveInstrument { track })
            .unwrap();
        let removed = authored.project().clone();
        let (_, editor) =
            crate::editor::clip_drag_ui_test::editor_window(cx, original.clone(), Arc::default());
        editor.update(cx, |view, _| {
            view.project_generation = 7;
            view.selected_track = Some(track);
            view.computer_keyboard.enabled = true;
            view.computer_keyboard.target = Some((7, 1, track));
            view.computer_keyboard.request_id = 51;
            let input = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
            view.computer_keyboard.input = Some(Arc::clone(&input));
            assert!(view.computer_keyboard.key_down("a", false));
            view.publish_computer_keyboard();
            let held = input.key_word(0).unwrap();
            assert_ne!(held, 0);
            view.state = Some(state(replacement.clone(), 2));
            assert!(view.tick_computer_keyboard_with_policy(true));
            assert!(!input.is_cancelled());
            assert!(Arc::ptr_eq(
                view.computer_keyboard.input.as_ref().unwrap(),
                &input
            ));
            assert_eq!(input.key_word(0), Some(held));
            assert_eq!(view.computer_keyboard.target, Some((7, 2, track)));
            assert_eq!(
                view.computer_keyboard.request_id, 51,
                "the worker already rebound this input"
            );
            view.stopped_playback_restore =
                Some(crate::editor::audio_admission::StoppedPlaybackRestore::Preparing);
            assert!(!view.tick_computer_keyboard_with_policy(true));
            assert!(
                !input.is_cancelled(),
                "preparation does not own the physical keys"
            );
            view.stopped_playback_restore = None;
            assert!(view.computer_keyboard_up("a"));
            assert_eq!(
                input.key_word(0),
                Some(0),
                "the original release reaches the same client"
            );
            view.stop();
            assert!(input.is_cancelled(), "explicit Stop remains terminal");
            assert!(view.computer_keyboard.held().iter().all(Option::is_none));

            let terminal =
                Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
            view.computer_keyboard.input = Some(Arc::clone(&terminal));
            view.computer_keyboard.target = Some((7, 2, track));
            view.computer_keyboard.suspended = false;
            assert!(view.computer_keyboard.key_down("a", false));
            view.publish_computer_keyboard();
            terminal.cancel();
            assert!(view.tick_computer_keyboard_with_policy(true));
            assert!(view.computer_keyboard.input.is_none());
            assert!(view.computer_keyboard.suspended);
            assert!(view.computer_keyboard.held().iter().all(Option::is_none));

            // Legacy publication, another project generation, and invalid instruments must not
            // inherit the physical activation, even when TrackId values happen to be equal.
            for (worker_owned, generation, project) in [
                (false, 7, replacement.clone()),
                (true, 8, replacement.clone()),
                (true, 7, bypassed),
                (true, 7, removed),
            ] {
                view.project_generation = generation;
                view.state = Some(state(project, 3));
                view.computer_keyboard.suspended = false;
                view.computer_keyboard.target = Some((7, 2, track));
                let old = Arc::new(hex_engine::computer_keyboard::ComputerKeyboardInput::default());
                view.computer_keyboard.input = Some(Arc::clone(&old));
                assert!(view.computer_keyboard.key_down("a", false));
                view.publish_computer_keyboard();
                assert!(view.tick_computer_keyboard_with_policy(worker_owned));
                assert!(old.is_cancelled());
                assert!(view.computer_keyboard.held().iter().all(Option::is_none));
                view.cancel_computer_keyboard();
            }
        });
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn replaced_output_clock_cancels_same_target_input_and_prepares_new_root(
        cx: &mut gpui::TestAppContext,
    ) {
        let _output_fixture_guard = crate::editor::device_panel::tests::eq_ui_tests::graph_layout_tests::status_fixture_guard();
        use hex_project::{Command, Created, Editor, Project};
        use std::{sync::Arc, time::Instant};
        fn open() -> (
            hex_audio::WorkerPreparedOutput,
            hex_audio::worker_fixture::HeadlessBackend,
        ) {
            let end = crate::test_wait::hang_deadline();
            loop {
                match hex_audio::worker_fixture::open(48_000) {
                    Ok((mut output, backend)) => {
                        output
                            .publication
                            .control()
                            .unwrap()
                            .configure_device_resource_publication_on_control(
                                hex_engine::AudioStorageLedgerReservation {
                                    data_rows: 512,
                                    handle_rows: 256,
                                    root_rows: 128,
                                    max_retained_bytes: 20_000_000,
                                    max_construction_peak_bytes: 20_000_000,
                                },
                            )
                            .unwrap();
                        return (output, backend);
                    }
                    Err(error)
                        if error == "audio output is open or still shutting down"
                            && Instant::now() < end =>
                    {
                        std::thread::yield_now()
                    }
                    Err(error) => panic!("{error}"),
                }
            }
        }
        let mut authored = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = authored
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            authored.project().clone(),
            Arc::default(),
        );
        let (mut old_output, old_backend) = open();
        let old_client = old_output.take_client().unwrap();
        assert!(old_client.worker_owns_graph_publication());
        let input = Arc::new(old_client.new_computer_keyboard_input_on_control().unwrap());
        editor.update(cx, |view, _| {
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: old_client,
                sample_rate: 48_000,
                label: "headless".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            view.document_audio_mode = hex_session::DocumentAudioMode::Resident;
            view.project_generation = 7;
            view.selected_track = Some(track);
            let revision = view.state.as_ref().unwrap().revision;
            view.computer_keyboard.enabled = true;
            view.computer_keyboard.target = Some((7, revision, track));
            view.computer_keyboard.request_id = 51;
            view.computer_keyboard.input = Some(Arc::clone(&input));
            assert!(view.computer_keyboard.key_down("a", false));
            view.publish_computer_keyboard();
            assert_ne!(input.key_word(0), Some(0));
            // Same document/target survive recovery while the actual native output closes.
            view.audio = None;
        });
        drop(old_output);
        drop(old_backend);
        let (mut new_output, new_backend) = open();
        let new_client = new_output.take_client().unwrap();
        assert!(new_client.worker_owns_graph_publication());
        assert!(!new_client.computer_keyboard_input_matches_output(&input));
        editor.update(cx, |view, _| {
            let target = view.computer_keyboard.target;
            view.audio = Some(crate::editor_backend::EditorAudio {
                control: new_client,
                sample_rate: 48_000,
                label: "headless".into(),
                spectrum_error: None,
                mode: hex_session::DocumentAudioMode::Resident,
            });
            assert!(view.tick_computer_keyboard());
            assert!(input.is_cancelled());
            let next = view.computer_keyboard.input.as_ref().unwrap();
            assert!(!Arc::ptr_eq(next, &input));
            assert!(
                view.audio
                    .as_ref()
                    .unwrap()
                    .control
                    .computer_keyboard_input_matches_output(next)
            );
            assert_eq!(next.key_word(0), Some(0));
            assert_eq!(
                view.computer_keyboard.target, target,
                "canonical target/revision did not change"
            );
            assert_eq!(
                view.computer_keyboard.request_id, 52,
                "new output needs fresh ordinary preparation"
            );
            assert!(view.computer_keyboard.held().iter().all(Option::is_none));
            view.cancel_computer_keyboard();
            view.audio = None;
        });
        drop(new_output);
        drop(new_backend);
    }
}
