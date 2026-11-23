mod catalog_barrier;
use super::EditorView;
pub(super) use catalog_barrier::CatalogBarrierState;
use hex_session::device_gesture::{
    DeviceParameterSnapshot, EqNodeCreationSnapshot, GestureKey, GesturePhase, GestureTarget,
    ParameterPairValue, ParameterValue,
};
use hex_session::service::{Request, SubmitError};
use std::collections::VecDeque;

fn defer_request(
    pending: &mut VecDeque<Request>,
    deferred: &mut VecDeque<Request>,
    request: Request,
) -> bool {
    if pending.len() + deferred.len() >= super::UI_REQUEST_CAPACITY {
        return false;
    }
    deferred.push_back(request);
    true
}

pub(super) fn release_deferred(pending: &mut VecDeque<Request>, deferred: &mut VecDeque<Request>) {
    pending.append(deferred);
}

pub(super) fn retire_deferred(
    pending: &mut VecDeque<Request>,
    deferred: &mut VecDeque<Request>,
    project_replaced: bool,
) {
    if project_replaced {
        deferred.clear();
    } else {
        release_deferred(pending, deferred);
    }
}

#[derive(Default, Debug, PartialEq, Eq)]
pub(super) struct FailedBarrierEffects {
    pub(super) save_refused: bool,
    pub(super) export_refused: bool,
    pub(super) tuning_library_refused: bool,
}

pub(super) fn retire_failed_tuning_library_barriers(deferred: &mut VecDeque<Request>) -> bool {
    let before = deferred.len();
    deferred.retain(|request| {
        !matches!(
            request,
            Request::TuningLibrary {
                operation: hex_session::tuning_gesture::TuningLibraryOperation::Save { .. }
                    | hex_session::tuning_gesture::TuningLibraryOperation::Export { .. },
                ..
            }
        )
    });
    deferred.len() != before
}

pub(super) fn retire_failed_finalize_barriers(
    deferred: &mut VecDeque<Request>,
) -> FailedBarrierEffects {
    let mut kept = VecDeque::with_capacity(deferred.len());
    let mut effects = FailedBarrierEffects {
        tuning_library_refused: retire_failed_tuning_library_barriers(deferred),
        ..FailedBarrierEffects::default()
    };
    while let Some(request) = deferred.pop_front() {
        match request {
            Request::SaveNew(_)
            | Request::SaveProjectAs { .. }
            | Request::SaveCurrent
            | Request::SaveCurrentWithFormatUpgrade { .. } => effects.save_refused = true,
            Request::Export(_) => effects.export_refused = true,
            Request::ExportRange { control, .. } => {
                control.cancel();
                effects.export_refused = true;
            }
            Request::SamplerEdit(hex_session::service::SamplerEdit::History(edit)) => edit
                .cancel
                .store(true, std::sync::atomic::Ordering::Release),
            Request::Undo
            | Request::Redo
            | Request::SaveDevicePreset { .. }
            | Request::SaveDeviceDefault { .. }
            | Request::SaveDeviceUnitPreset { .. }
            | Request::SaveDeviceUnitDefault { .. }
            | Request::LoadDevicePreset { .. } => {}
            request => kept.push_back(request),
        }
    }
    *deferred = kept;
    effects
}

pub(super) fn actor_retired_gesture(
    project_replaced: bool,
    actor_active: bool,
    ack: Option<&hex_session::device_gesture::GestureAck>,
    local_key: Option<hex_session::device_gesture::GestureKey>,
) -> bool {
    project_replaced
        || (!actor_active
            && ack.is_some_and(|ack| ack.reason.is_some() && Some(ack.key) == local_key))
}

fn device_gesture_admission_error(
    audio_recording: bool,
    close_frozen: bool,
    automation_busy: bool,
) -> Result<(), &'static str> {
    if audio_recording {
        return Err("finish the audio recording before changing a device");
    }
    if close_frozen {
        return Err("close is pending · device gestures are unavailable");
    }
    if automation_busy {
        return Err("finish the current automation touch first");
    }
    Ok(())
}

pub(super) fn insertion_control(
    request: &Request,
) -> Option<std::sync::Arc<hex_session::service::PluginEffectControl>> {
    match request {
        Request::InsertPluginEffect(insertion) => Some(std::sync::Arc::clone(&insertion.control)),
        other => other.builtin_device_insert_control().cloned(),
    }
}

impl EditorView {
    pub(super) fn check_device_gesture_admission(&self) -> Result<(), &'static str> {
        device_gesture_admission_error(
            self.audio_recorder.blocks_edits(),
            self.automation_close_preflight.frozen(),
            self.native_automation_recorder.busy(),
        )
    }

    /// Start the worker-owned native parameter edit used by device controls.
    pub fn begin_device_gesture(
        &mut self,
        target: GestureTarget,
        initial: ParameterValue,
    ) -> Result<(), &'static str> {
        self.check_device_gesture_admission()?;
        self.device_gesture
            .begin(self.project_generation, target, initial)?;
        Ok(())
    }

    pub(super) fn begin_device_pair_gesture(
        &mut self,
        device: hex_project::DeviceId,
        first: hex_lattice_contract::ParameterId,
        second: hex_lattice_contract::ParameterId,
        initial: ParameterPairValue,
    ) -> Result<(), &'static str> {
        self.check_device_gesture_admission()?;
        self.device_gesture
            .begin_pair(self.project_generation, device, first, second, initial)?;
        Ok(())
    }

    pub(super) fn begin_device_snapshot_gesture(
        &mut self,
        device: hex_project::DeviceId,
        initial: DeviceParameterSnapshot,
    ) -> Result<GestureKey, &'static str> {
        self.check_device_gesture_admission()?;
        self.device_gesture
            .begin_snapshot(self.project_generation, device, initial)
    }

    pub(super) fn begin_eq_creation_gesture(
        &mut self,
        device: hex_project::DeviceId,
        snapshot: EqNodeCreationSnapshot,
    ) -> Result<GestureKey, &'static str> {
        self.check_device_gesture_admission()?;
        self.device_gesture
            .begin_eq_creation(self.project_generation, device, snapshot)
    }

    pub(super) fn begin_track_gains_gesture(
        &mut self,
        group: hex_project::RelativeTrackGains,
    ) -> Result<(), &'static str> {
        self.check_device_gesture_admission()?;
        self.device_gesture
            .begin_track_gains(self.project_generation, group)?;
        Ok(())
    }

    pub fn change_device_gesture(&mut self, value: ParameterValue) -> Result<(), &'static str> {
        self.device_gesture.change(value).map(|_| ())
    }

    pub(super) fn change_device_pair_gesture(
        &mut self,
        value: ParameterPairValue,
    ) -> Result<(), &'static str> {
        self.device_gesture.change_pair(value).map(|_| ())
    }

    pub(super) fn finish_device_gesture(
        &mut self,
        value: ParameterValue,
    ) -> Result<(), &'static str> {
        self.device_gesture.finish(value)
    }

    pub(super) fn finish_device_pair_gesture(
        &mut self,
        value: ParameterPairValue,
    ) -> Result<(), &'static str> {
        self.device_gesture.finish_pair(value)
    }

    pub fn cancel_device_gesture(&mut self) -> Result<(), &'static str> {
        self.device_gesture.cancel()
    }

    pub(super) fn device_gesture_pending(&self) -> bool {
        self.device_gesture.snapshot().is_some() || !self.device_gesture_barriers.is_empty()
    }

    pub(super) fn refuse_device_gesture_barriers(&mut self, reason: &'static str) {
        let insertions: Vec<_> = self
            .device_gesture_barriers
            .iter()
            .filter_map(insertion_control)
            .collect();
        let had_clipboard = self
            .device_gesture_barriers
            .iter()
            .any(|request| matches!(request, Request::DeviceClipboard { .. }));
        self.device_gesture_barriers.clear();
        self.catalog_barrier.custody = None;
        if had_clipboard {
            self.device_panel.pending_duplicate = None;
        }
        for control in insertions {
            self.retire_plugin_effect(&control, false, Some(reason));
        }
    }

    pub(super) fn pump_device_gesture(&mut self) -> bool {
        let Some(gesture) = self.device_gesture.next_request() else {
            return false;
        };
        match self.service.submit(Request::device_gesture(gesture)) {
            Ok(()) => {
                self.device_gesture.mark_submitted();
                self.worker_submitted();
                true
            }
            Err(SubmitError::Full(_)) => false,
            Err(SubmitError::Busy(_, reason)) => {
                self.device_gesture.reset();
                self.refuse_device_gesture_barriers(super::user_errors::busy(reason));
                self.retire_tuning_library_device_terminal();
                self.refuse_deferred_tuning_writes();
                self.error = Some(super::user_errors::busy(reason).into());
                true
            }
            Err(SubmitError::Closed(_)) => {
                self.device_gesture.reset();
                self.refuse_device_gesture_barriers("project worker stopped");
                self.retire_tuning_library_device_terminal();
                self.refuse_deferred_tuning_writes();
                self.error = Some("project worker stopped".into());
                true
            }
        }
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn acknowledge_device_gesture(
        &mut self,
        ack: &hex_session::device_gesture::GestureAck,
    ) -> bool {
        self.acknowledge_device_gesture_with_snapshot(ack, None)
    }

    pub(super) fn acknowledge_device_gesture_with_snapshot(
        &mut self,
        ack: &hex_session::device_gesture::GestureAck,
        snapshot: Option<hex_session::device_gesture::GestureSnapshot>,
    ) -> bool {
        let committed_display = (ack.phase == GesturePhase::Commit
            && ack.accepted
            && !ack.preview_retained
            && !ack.source_lost)
            .then(|| {
                let active = self.device_gesture.snapshot()?;
                (active.key == ack.key
                    && active.sequence == ack.sequence
                    && active.plugin_value.is_some()
                    && self.device_gesture.terminal() == Some((GesturePhase::Commit, ack.sequence)))
                .then_some(active)
            })
            .flatten();
        let committed_plugin = committed_display.filter(|_| !self.owns_vst3_recording(ack.key));
        let terminal =
            matches!(ack.phase, GesturePhase::Commit | GesturePhase::Cancel) || ack.source_lost;
        let plugin = self.device_gesture.snapshot().is_some_and(|active| {
            active.key == ack.key && matches!(active.target, GestureTarget::PluginParameter { .. })
        });
        let matched = self.device_gesture.acknowledge_with_snapshot(ack, snapshot);
        if !matched {
            self.refuse_catalog_barriers(
                "device load canceled · device acknowledgment did not match",
            );
            return false;
        }
        if let Some(snapshot) = committed_plugin {
            self.accept_touched_vst3_commit(snapshot);
        }
        if plugin
            && !ack.preview_retained
            && (terminal || ack.phase == GesturePhase::Begin && !ack.accepted)
        {
            super::device_panel::vst3_parameters::refresh_after_gesture(self, committed_display);
        }
        if terminal {
            self.settle_catalog_barriers(ack);
            self.bind_tuning_library_device_terminal(ack.key, ack.accepted);
            if !ack.accepted {
                self.refuse_deferred_tuning_writes();
                let effects = retire_failed_finalize_barriers(&mut self.device_gesture_barriers);
                if effects.tuning_library_refused {
                    self.tuning.library_pending = None;
                }
                if effects.save_refused {
                    self.saving_project = false;
                    self.cancel_open_after_save();
                }
                if effects.export_refused {
                    // Deferred exports have not been submitted to Service, so
                    // there is no worker reservation to release here.
                    self.export_pending = false;
                    if let Some(control) = self.export_control.take() {
                        control.cancel();
                    }
                    self.export_window = None;
                }
                self.error = Some(
                    ack.reason
                        .clone()
                        .unwrap_or_else(|| "device parameter edit could not be committed".into())
                        .into(),
                );
            }
            // A refused publication still owns the audible preview. Keep unrelated intents
            // ordered behind its explicit retry/cancel instead of claiming canonical restoration.
            if !ack.preview_retained {
                release_deferred(
                    &mut self.pending_requests,
                    &mut self.device_gesture_barriers,
                );
            }
        } else if ack.phase == GesturePhase::Begin && !ack.accepted {
            self.refuse_catalog_barriers("device load canceled · device edit was refused");
            // The worker refused to admit the draft, so release dependent
            // barriers in their accepted order without an edit to finalize.
            release_deferred(
                &mut self.pending_requests,
                &mut self.device_gesture_barriers,
            );
            self.error = Some(
                ack.reason
                    .clone()
                    .unwrap_or_else(|| "device parameter edit could not start".into())
                    .into(),
            );
        }
        true
    }

    pub(super) fn enqueue_device_barrier(&mut self, request: Request) -> bool {
        if let Some(admitted) = self.submit_update_save(&request) {
            return admitted;
        }
        if self.device_gesture.snapshot().is_none() {
            return self.enqueue(request);
        }
        if !self.admit_independent_edit_with_local_draft(&request) {
            return false;
        }
        if self.pending_requests.len() + self.device_gesture_barriers.len()
            >= super::UI_REQUEST_CAPACITY
        {
            self.error = Some("device edit queue full · request was not applied".into());
            return false;
        }
        let custody = match self.catalog_custody(&request) {
            Ok(proof) => proof,
            Err(reason) => {
                self.error = Some(reason.into());
                return false;
            }
        };
        if !self.device_gesture.has_pending_terminal() {
            // An unreleased creation has no canonical node to finalize. An
            // independent project action cancels it; a released terminal keeps
            // its admitted custody and remains ordered ahead of this barrier.
            let result = if self.device_gesture.snapshot().is_some_and(|snapshot| {
                matches!(snapshot.target, GestureTarget::EqNodeCreation { .. })
            }) {
                self.device_gesture.cancel()
            } else {
                self.device_gesture.finish_current()
            };
            if let Err(error) = result {
                self.error = Some(error.into());
                return false;
            }
        }
        if defer_request(
            &mut self.pending_requests,
            &mut self.device_gesture_barriers,
            request,
        ) {
            if let Some(proof) = custody {
                self.catalog_barrier.custody = Some(proof);
            }
            true
        } else {
            self.error = Some("device edit queue full · request was not applied".into());
            false
        }
    }

    pub(super) fn enqueue_after_device_terminal(
        &mut self,
        request: &mut Option<Request>,
    ) -> Option<bool> {
        if self.device_gesture_barriers.is_empty() && !self.device_gesture.has_pending_terminal() {
            return None;
        }
        let Some(request) = request.take() else {
            return Some(false);
        };
        let custody = match self.catalog_custody(&request) {
            Ok(proof) => proof,
            Err(reason) => {
                self.error = Some(reason.into());
                return Some(false);
            }
        };
        if defer_request(
            &mut self.pending_requests,
            &mut self.device_gesture_barriers,
            request,
        ) {
            if let Some(proof) = custody {
                self.catalog_barrier.custody = Some(proof);
            }
            Some(true)
        } else {
            self.error = Some("device edit queue full · request was not applied".into());
            Some(false)
        }
    }
}

#[cfg(test)]
mod barrier_unit_tests {
    use super::*;
    use hex_engine::NativeDeviceClass;
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{Command, Created, Editor, Project};
    use hex_session::service::preset::Owner;
    use hex_session::service::{
        PluginEffectControl, PluginEffectDestination, PluginEffectInsertion, PluginEffectSelection,
    };
    use std::sync::Arc;

    #[test]
    fn retire_failed_finalize_barriers_drops_unit_preset_and_default_saves() {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::AddTrack { name: "a".into() })
            .unwrap()
        else {
            panic!("track");
        };
        let utility = DeviceTypeId::from_static("hex.utility");
        let state = hex_engine::default_native_device_state(utility).unwrap();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device");
        };
        let owner = Owner {
            project_generation: 0,
            revision: 1,
            device,
            class: NativeDeviceClass::Effect,
            type_id: utility,
        };
        let mut deferred = VecDeque::from([
            Request::SaveDevicePreset {
                owner,
                path: std::path::PathBuf::from("/tmp/p.hexpreset"),
            },
            Request::SaveDeviceDefault {
                owner,
                root: std::path::PathBuf::from("/tmp/defaults"),
            },
            Request::SaveDeviceUnitPreset {
                owner,
                path: std::path::PathBuf::from("/tmp/u.hexpreset"),
            },
            Request::SaveDeviceUnitDefault {
                owner,
                root: std::path::PathBuf::from("/tmp/defaults"),
            },
            Request::Open(std::path::PathBuf::from("/tmp/next.hex")),
        ]);
        let _ = retire_failed_finalize_barriers(&mut deferred);
        assert_eq!(deferred.len(), 1);
        assert!(matches!(deferred.front(), Some(Request::Open(_))));
    }

    #[test]
    fn insertion_control_extracts_vst3_plugin_effect_control() {
        let control = Arc::new(PluginEffectControl::default());
        let request = Request::InsertPluginEffect(PluginEffectInsertion {
            context: hex_engine::AudioSourceContext {
                project_generation: 0,
                editor_revision: 1,
            },
            selection: PluginEffectSelection {
                module: std::path::PathBuf::from("/tmp/test.vst3"),
                executable: "Contents/MacOS/test".into(),
                cid: [0x11; 16],
                executable_sha256: "a".repeat(64),
                info_sha256: "b".repeat(64),
            },
            destination: PluginEffectDestination::Main,
            before: None,
            track_device_name: None,
            control: Arc::clone(&control),
        });
        let extracted = insertion_control(&request).expect("plugin control");
        assert!(Arc::ptr_eq(&extracted, &control));
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use hex_session::service::Request;

    #[gpui::test]
    fn plugin_source_loss_on_change_releases_edits_and_refuses_deferred_save(
        cx: &mut gpui::TestAppContext,
    ) {
        use hex_lattice_contract::{DeviceTypeId, NativeDeviceState};
        use hex_project::{Command, Created, Editor, Project};
        use hex_session::device_gesture::{
            GestureAck, GesturePhase, PluginParameterOrigin, PluginParameterValue,
        };
        use std::sync::Arc;
        let mut project = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = project
            .execute(Command::AddTrack {
                name: "plugin control".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let Created::Device(device) = project
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![1])
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("device")
        };
        let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            project.project().clone(),
            Arc::default(),
        );
        editor.update(cx, |view, _| {
            let key = view
                .device_gesture
                .begin_plugin(PluginParameterOrigin {
                    context: hex_engine::AudioSourceContext {
                        project_generation: view.project_generation,
                        editor_revision: 0,
                    },
                    device,
                    parameter: 0,
                    page_first: 0,
                    metadata_revision: 1,
                    initial: PluginParameterValue::new(0.25).unwrap(),
                })
                .unwrap();
            view.device_gesture.next_request().unwrap();
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key,
                phase: GesturePhase::Begin,
                sequence: 0,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
            let sequence = view
                .device_gesture
                .change_plugin(PluginParameterValue::new(0.75).unwrap())
                .unwrap();
            view.device_gesture.next_request().unwrap();
            view.device_gesture.mark_submitted();
            view.device_gesture_barriers.push_back(Request::SaveCurrent);
            view.device_gesture_barriers
                .push_back(Request::Edit(Command::AddTrack {
                    name: "after lost source".into(),
                }));
            view.saving_project = true;
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key,
                phase: GesturePhase::Change,
                sequence,
                accepted: false,
                preview_retained: false,
                source_lost: true,
                reason: Some("plugin source disappeared".into()),
            }));
            assert!(view.device_gesture.snapshot().is_none());
            assert!(!view.saving_project);
            assert!(view.device_gesture_barriers.is_empty());
            assert_eq!(view.pending_requests.len(), 1);
            assert!(matches!(
                view.pending_requests.front(),
                Some(Request::Edit(_))
            ));
        });
    }

    #[gpui::test]
    fn retained_preview_refusal_keeps_edit_barrier_until_cancel(cx: &mut gpui::TestAppContext) {
        use hex_lattice_contract::ParameterValue;
        use hex_project::{Command, Created, Editor, Project};
        use hex_session::device_gesture::{GestureAck, GesturePhase, GestureTarget};
        use std::sync::Arc;
        let mut project = Editor::new(Project::new(48_000).unwrap(), 4).unwrap();
        let Created::Track(track) = project
            .execute(Command::AddTrack {
                name: "preview".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let (_, editor) = crate::editor::clip_drag_ui_test::editor_window(
            cx,
            project.project().clone(),
            Arc::default(),
        );
        editor.update(cx, |view, _| {
            let key = view
                .device_gesture
                .begin(
                    view.project_generation,
                    GestureTarget::TrackGain { track },
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            view.device_gesture.next_request().unwrap();
            let begin_ack = GestureAck {
                key,
                phase: GesturePhase::Begin,
                sequence: 0,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            };
            assert!(view.acknowledge_device_gesture(&begin_ack));
            let sequence = view
                .device_gesture
                .change(ParameterValue::Float(0.5))
                .unwrap();
            view.device_gesture.next_request().unwrap();
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key,
                phase: GesturePhase::Change,
                sequence,
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
            let accepted = view.device_gesture.snapshot().unwrap();
            view.device_gesture
                .finish(ParameterValue::Float(0.25))
                .unwrap();
            let terminal = view.device_gesture.next_request().unwrap();
            view.device_gesture.mark_submitted();
            view.device_gesture_barriers.push_back(Request::SaveCurrent);
            view.device_gesture_barriers
                .push_back(Request::Edit(Command::AddTrack {
                    name: "after cancellation".into(),
                }));
            view.saving_project = true;
            let refusal = GestureAck {
                key,
                phase: GesturePhase::Commit,
                sequence: terminal.sequence(),
                accepted: false,
                preview_retained: true,
                source_lost: false,
                reason: Some("audio publication queue full".into()),
            };
            assert!(view.acknowledge_device_gesture_with_snapshot(&refusal, Some(accepted)));
            assert_eq!(view.device_gesture.snapshot(), Some(accepted));
            assert!(
                view.device_gesture.next_request().is_none(),
                "refusal does not spin"
            );
            assert!(!view.saving_project);
            assert!(view.pending_requests.is_empty());
            assert_eq!(view.device_gesture_barriers.len(), 1);
            assert!(matches!(
                view.device_gesture_barriers.front(),
                Some(Request::Edit(_))
            ));
            view.device_gesture.cancel().unwrap();
            let cancel = view.device_gesture.next_request().unwrap();
            assert!(view.acknowledge_device_gesture(&GestureAck {
                key,
                phase: GesturePhase::Cancel,
                sequence: cancel.sequence(),
                accepted: true,
                preview_retained: false,
                source_lost: false,
                reason: None,
            }));
            assert!(view.device_gesture.snapshot().is_none());
            assert!(view.device_gesture_barriers.is_empty());
            assert!(matches!(
                view.pending_requests.front(),
                Some(Request::Edit(_))
            ));
        });
    }
}

#[cfg(all(test, not(feature = "ui-test-support")))]
mod unit_tests {
    use super::{
        FailedBarrierEffects, actor_retired_gesture, defer_request, release_deferred,
        retire_deferred, retire_failed_finalize_barriers,
    };
    use hex_session::service::Request;
    use std::collections::VecDeque;

    #[test]
    fn unrelated_event_does_not_retire_a_pending_begin_or_terminal() {
        assert!(!actor_retired_gesture(
            false,
            false,
            None,
            Some(current_key())
        ));
        assert!(!actor_retired_gesture(
            false,
            false,
            Some(&ack_without_reason()),
            Some(current_key())
        ));
        assert!(!actor_retired_gesture(
            false,
            false,
            Some(&ack_with_reason()),
            Some(other_key())
        ));
    }

    #[test]
    fn rejected_rebase_or_project_replacement_retires_the_local_gesture() {
        assert!(actor_retired_gesture(
            false,
            false,
            Some(&ack_with_reason()),
            Some(current_key())
        ));
        assert!(actor_retired_gesture(
            true,
            false,
            None,
            Some(current_key())
        ));
    }

    #[test]
    fn release_save_then_later_edit_stays_fifo_with_bounded_capacity() {
        let mut pending = VecDeque::new();
        let mut deferred = VecDeque::new();
        for _ in 0..30 {
            pending.push_back(Request::Undo);
        }
        assert!(defer_request(
            &mut pending,
            &mut deferred,
            Request::SaveCurrent
        ));
        assert!(defer_request(&mut pending, &mut deferred, Request::Redo));
        assert!(!defer_request(
            &mut pending,
            &mut deferred,
            Request::SaveCurrent
        ));
        release_deferred(&mut pending, &mut deferred);
        assert_eq!(pending.len(), super::super::UI_REQUEST_CAPACITY);
        assert!(matches!(pending.get(30), Some(Request::SaveCurrent)));
        assert!(matches!(pending.get(31), Some(Request::Redo)));
    }

    #[test]
    fn invalid_rebase_releases_retained_barriers_but_project_open_retires_them() {
        let mut pending = VecDeque::new();
        let mut deferred = VecDeque::from([Request::SaveCurrent, Request::Undo]);
        retire_deferred(&mut pending, &mut deferred, false);
        assert_eq!(pending.len(), 2);
        assert!(deferred.is_empty());
        assert!(matches!(pending.pop_front(), Some(Request::SaveCurrent)));
        assert!(matches!(pending.pop_front(), Some(Request::Undo)));

        deferred.push_back(Request::SaveCurrent);
        retire_deferred(&mut pending, &mut deferred, true);
        assert!(deferred.is_empty());
        assert!(pending.is_empty());
    }

    #[test]
    fn failed_terminal_retires_finalize_barriers_and_keeps_open_and_edits_ordered() {
        let control = hex_session::service::ExportControl::default();
        let mut project =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 4).unwrap();
        let hex_project::Created::Track(track) = project
            .execute(hex_project::Command::AddTrack {
                name: "default owner".into(),
            })
            .unwrap()
        else {
            panic!("track");
        };
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
        )
        .unwrap();
        let hex_project::Created::Device(device) = project
            .execute(hex_project::Command::InsertDevice {
                track,
                before: None,
                state,
            })
            .unwrap()
        else {
            panic!("device");
        };
        let mut deferred = VecDeque::from([
            Request::SaveCurrent,
            Request::SaveDeviceDefault {
                owner: hex_session::service::preset::Owner {
                    project_generation: 0,
                    revision: 0,
                    device,
                    class: hex_engine::NativeDeviceClass::Effect,
                    type_id: hex_lattice_contract::DeviceTypeId::from_static("hex.utility"),
                },
                root: "default-data".into(),
            },
            Request::TuningLibrary {
                project_generation: 1,
                revision: 2,
                operation_id: 3,
                operation: hex_session::tuning_gesture::TuningLibraryOperation::Export {
                    path: "deferred.freqs".into(),
                    name: "deferred".into(),
                },
            },
            Request::Edit(hex_project::Command::AddTrack {
                name: "later edit".into(),
            }),
            Request::Export("deferred.wav".into()),
            Request::ExportRange {
                path: "deferred-range.wav".into(),
                start: 128,
                frames: Some(256),
                signal: hex_session::service::ExportSignal::Main,
                control: control.clone(),
                format: hex_session::service::ExportFormat::Pcm16,
                publication: hex_session::service::ExportPublication::CreateNew,
            },
            Request::Undo,
            Request::Open("confirmed.hex".into()),
        ]);
        let effects = retire_failed_finalize_barriers(&mut deferred);
        assert_eq!(
            effects,
            FailedBarrierEffects {
                save_refused: true,
                export_refused: true,
                tuning_library_refused: true,
            }
        );
        assert!(control.is_cancelled());
        assert_eq!(deferred.len(), 2);
        assert!(matches!(deferred.pop_front(), Some(Request::Edit(_))));
        assert!(matches!(
            deferred.pop_front(),
            Some(
                Request::Open(_)
                    | Request::OpenWith { .. }
                    | Request::NewWithAudioMode { .. }
                    | Request::OpenWithAudioMode { .. }
            )
        ));
    }

    fn ack_without_reason() -> hex_session::device_gesture::GestureAck {
        hex_session::device_gesture::GestureAck {
            key: current_key(),
            phase: hex_session::device_gesture::GesturePhase::Change,
            sequence: 1,
            accepted: true,
            preview_retained: false,
            source_lost: false,
            reason: None,
        }
    }

    fn ack_with_reason() -> hex_session::device_gesture::GestureAck {
        let mut ack = ack_without_reason();
        ack.reason = Some("device gesture target was removed".into());
        ack
    }

    fn current_key() -> hex_session::device_gesture::GestureKey {
        hex_session::device_gesture::GestureKey {
            generation: 0,
            token: 1,
        }
    }

    #[test]
    fn admission_blocker_precedence_covers_all_three_guards() {
        use super::device_gesture_admission_error;
        assert_eq!(device_gesture_admission_error(false, false, false), Ok(()));
        assert_eq!(
            device_gesture_admission_error(true, true, true),
            Err("finish the audio recording before changing a device")
        );
        assert_eq!(
            device_gesture_admission_error(false, true, true),
            Err("close is pending · device gestures are unavailable")
        );
        assert_eq!(
            device_gesture_admission_error(false, false, true),
            Err("finish the current automation touch first")
        );
    }

    fn other_key() -> hex_session::device_gesture::GestureKey {
        hex_session::device_gesture::GestureKey {
            generation: 0,
            token: 2,
        }
    }
}

#[cfg(all(test, feature = "ui-test-support"))]
mod admission_tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use hex_devices::eq::{Node, NodeId};
    use hex_lattice_contract::ParameterId;
    use hex_project::{Command, Editor, Project, RelativeTrackGains};

    #[gpui::test]
    fn all_device_and_track_gain_gesture_entries_refuse_under_all_three_blockers(
        cx: &mut TestAppContext,
    ) {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
        editor
            .execute(Command::InsertTrackWithDevice {
                name: "a".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap();
        editor
            .execute(Command::AddTrack { name: "b".into() })
            .unwrap();
        let tracks: Vec<_> = editor.project().tracks().iter().map(|t| t.id()).collect();
        let device = editor.project().tracks()[0].instrument().unwrap().id();
        let group = RelativeTrackGains::capture(editor.project(), tracks[0], &tracks).unwrap();
        let project = editor.project().clone();

        let (handle, view) = crate::editor::rendered_test_fixture::editor_window(
            cx,
            project,
            std::sync::Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        let first = ParameterId::new(1).unwrap();
        let second = ParameterId::new(2).unwrap();
        let snapshot =
            DeviceParameterSnapshot::new(&[(first, ParameterValue::Float(0.5))]).unwrap();
        let eq_node = Node::new(NodeId::new(1).unwrap(), 1_000.0, 0.0, 0).unwrap();
        let eq_creation =
            EqNodeCreationSnapshot::new(eq_node, hex_project::DeviceVariant::A, 1).unwrap();
        let track = tracks[0];

        let assert_all_refused = |view: &mut EditorView, expected: &'static str| {
            assert_eq!(
                view.begin_device_gesture(
                    GestureTarget::TrackGain { track },
                    ParameterValue::Float(1.0),
                ),
                Err(expected)
            );
            assert_eq!(
                view.begin_device_pair_gesture(
                    device,
                    first,
                    second,
                    ParameterPairValue {
                        first: ParameterValue::Float(0.5),
                        second: ParameterValue::Float(0.5),
                    },
                ),
                Err(expected)
            );
            assert_eq!(
                view.begin_device_snapshot_gesture(device, snapshot),
                Err(expected)
            );
            assert_eq!(
                view.begin_eq_creation_gesture(device, eq_creation),
                Err(expected)
            );
            assert_eq!(view.begin_track_gains_gesture(group), Err(expected));
            assert!(view.device_gesture.snapshot().is_none());
            assert!(view.device_gesture.next_request().is_none());
        };

        // 1. Audio recording active
        view.update(&mut visual, |view, _| {
            view.audio_recorder.set_test_blocks_edits(track, true);
            assert!(view.audio_recorder.blocks_edits());
            assert_all_refused(view, "finish the audio recording before changing a device");
            view.audio_recorder.set_test_blocks_edits(track, false);
        });

        // 2. Native automation touch busy
        view.update(&mut visual, |view, _| {
            view.native_automation_recorder.set_test_busy(device, true);
            assert!(view.native_automation_recorder.busy());
            assert_all_refused(view, "finish the current automation touch first");
            view.native_automation_recorder.set_test_busy(device, false);
        });

        // 3. Automation close preflight frozen
        handle
            .update(&mut visual, |_, window, cx| {
                view.update(cx, |view, cx| {
                    view.request_quit(window, cx);
                    assert!(view.automation_close_preflight.frozen());
                    assert_all_refused(view, "close is pending · device gestures are unavailable");
                });
            })
            .unwrap();
    }

    #[gpui::test]
    fn refuse_device_gesture_barriers_and_commit_rebase_handle_vst3_and_device_requests(
        cx: &mut gpui::TestAppContext,
    ) {
        use super::*;
        use gpui::VisualTestContext;
        use hex_engine::NativeDeviceClass;
        use hex_lattice_contract::DeviceTypeId;
        use hex_project::{Command, DeviceChainOwner, DeviceSelectionIds, Project};
        use hex_session::device_gesture::GestureTarget;
        use hex_session::service::{
            ArrangementDeviceDestination, DeviceClipboardAction, NativeDeviceDropSource,
            PluginEffectDestination,
        };

        let recv_event = |service: &hex_session::service::Service| {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                match service.try_event() {
                    Ok(event) => return event,
                    Err(std::sync::mpsc::TryRecvError::Empty)
                        if std::time::Instant::now() < deadline =>
                    {
                        std::thread::sleep(std::time::Duration::from_millis(1));
                    }
                    Err(error) => panic!("service response unavailable: {error}"),
                }
            }
        };

        let (handle, view) = crate::editor::rendered_test_fixture::editor_window(
            cx,
            Project::new(48_000).unwrap(),
            std::sync::Arc::default(),
        );
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        let utility = DeviceTypeId::from_static("hex.utility");
        let utility_state = hex_engine::default_native_device_state(utility).unwrap();
        let (track, device) = view.update(&mut visual, |view, _| {
            while let Ok(event) = view.service.try_event() {
                view.receive(event);
            }
            view.service
                .submit(Request::Edit(Command::AddTrack {
                    name: "audio".into(),
                }))
                .unwrap();
            let event = recv_event(&view.service);
            view.receive(event);
            let track = view.state.as_ref().unwrap().project.tracks()[0].id();
            view.service
                .submit(Request::Edit(Command::InsertDevice {
                    track,
                    before: None,
                    state: utility_state,
                }))
                .unwrap();
            let event = recv_event(&view.service);
            view.receive(event);
            let device = view.state.as_ref().unwrap().project.tracks()[0].devices()[0].id();
            (track, device)
        });

        let plugin_selection = hex_plugin_catalog::PluginSelection {
            module: std::path::PathBuf::from("/tmp/test.vst3"),
            executable_relative_path: "Contents/MacOS/test".into(),
            cid: [0x11; 16],
            class_name: "test vst3".into(),
            role: hex_plugin_catalog::PluginRole::Effect,
            executable_sha256: "a".repeat(64),
            info_sha256: "b".repeat(64),
        };

        // 1. Refusing a queued Request::InsertPluginEffect retires pending_plugin_effect cleanly.
        view.update(&mut visual, |view, cx| {
            view.begin_device_gesture(
                GestureTarget::TrackGain { track },
                ParameterValue::Float(1.0),
            )
            .unwrap();
            let _ = view.device_gesture.next_request();
            view.device_gesture.mark_submitted();
            view.insert_plugin_effect(
                &plugin_selection,
                PluginEffectDestination::Track(track),
                None,
                cx,
            );
            assert!(view.pending_plugin_effect.is_some());
            assert_eq!(view.device_gesture_barriers.len(), 1);

            view.refuse_device_gesture_barriers("project worker busy");
            assert!(
                view.pending_plugin_effect.is_none(),
                "refuse_device_gesture_barriers must retire pending_plugin_effect"
            );
            assert!(view.device_gesture_barriers.is_empty());
            view.device_gesture.reset();
        });

        // 2. Committing a gesture rebases all 7 newly covered revision-stamped device requests (R -> R+1).
        let selected = DeviceSelectionIds::new(&[device]).unwrap();
        let parameter = hex_lattice_contract::ParameterId::from_static(1);
        let initial = view.read_with(&visual, |view, _| {
            hex_engine::native_device_parameter_for_class(
                NativeDeviceClass::Effect,
                view.state
                    .as_ref()
                    .unwrap()
                    .project
                    .device(device)
                    .unwrap()
                    .state(),
                parameter,
            )
            .unwrap()
        });
        let mut commit_event = None;
        let mut start_rev = 0;
        view.update(&mut visual, |view, cx| {
            start_rev = view.state.as_ref().unwrap().revision;
            let generation = view.project_generation;
            view.begin_device_gesture(
                GestureTarget::DeviceParameter { device, parameter },
                initial,
            )
            .unwrap();
            assert!(view.pump_device_gesture());
            let begin_event = recv_event(&view.service);
            assert!(
                begin_event.gesture_ack.as_ref().unwrap().accepted,
                "begin ack refused: {:?}",
                begin_event.gesture_ack
            );
            view.receive(begin_event);

            view.device_gesture
                .finish(ParameterValue::Float(6.0))
                .unwrap();
            assert!(view.pump_device_gesture());

            assert!(
                view.enqueue_device_barrier(Request::InsertBrowserInstrumentTrack {
                    project_generation: generation,
                    revision: start_rev,
                    class: NativeDeviceClass::Instrument,
                    type_id: DeviceTypeId::from_static("hex.instrument.sine"),
                    new_track_gain: None,
                    control: None,
                })
            );
            assert!(
                view.enqueue_device_barrier(Request::DropBrowserDeviceOnArrangement {
                    project_generation: generation,
                    revision: start_rev,
                    class: NativeDeviceClass::Effect,
                    type_id: utility,
                    destination: ArrangementDeviceDestination::ExistingTrack {
                        track,
                        instrument: None,
                    },
                    new_track_gain: None,
                    control: None,
                })
            );
            assert!(view.enqueue_device_barrier(Request::DropNativeDevice {
                project_generation: generation,
                revision: start_rev,
                source: NativeDeviceDropSource::Browser {
                    class: NativeDeviceClass::Effect,
                    type_id: utility,
                },
                track,
                before: Some(device),
                replace: None,
                control: None,
            }));
            assert!(view.enqueue_device_barrier(Request::MoveTrackDevices {
                project_generation: generation,
                revision: start_rev,
                source: track,
                selected: selected.clone(),
                destination: track,
            }));
            assert!(view.enqueue_device_barrier(Request::ReorderDevices {
                project_generation: generation,
                revision: start_rev,
                owner: DeviceChainOwner::Track(track),
                selected: selected.clone(),
                before: Some(device),
            }));
            assert!(view.enqueue_device_barrier(Request::DeviceClipboard {
                project_generation: generation,
                revision: start_rev,
                action: DeviceClipboardAction::Duplicate {
                    source: DeviceChainOwner::Track(track),
                    selected: selected.clone(),
                    destination: DeviceChainOwner::Track(track),
                    before: None,
                },
            }));
            view.insert_plugin_effect(
                &plugin_selection,
                PluginEffectDestination::Track(track),
                None,
                cx,
            );
            assert_eq!(view.device_gesture_barriers.len(), 7);

            commit_event = Some(recv_event(&view.service));
        });

        view.update(&mut visual, |view, _| {
            let event = commit_event.take().unwrap();
            let next_rev = start_rev + 1;
            assert_eq!(event.state.revision, next_rev);
            view.receive(event);
            assert!(view.device_gesture_barriers.is_empty());
            assert_eq!(view.pending_requests.len(), 7);

            for request in &view.pending_requests {
                match request {
                    Request::InsertBrowserInstrumentTrack { revision, .. }
                    | Request::DropBrowserDeviceOnArrangement { revision, .. }
                    | Request::DropNativeDevice { revision, .. }
                    | Request::MoveTrackDevices { revision, .. }
                    | Request::ReorderDevices { revision, .. }
                    | Request::DeviceClipboard { revision, .. } => {
                        assert_eq!(*revision, next_rev, "revision must rebase for {request:?}");
                    }
                    Request::InsertPluginEffect(insertion) => {
                        assert_eq!(insertion.context.editor_revision, next_rev);
                    }
                    other => panic!("unexpected request: {other:?}"),
                }
            }
        });
    }
}
