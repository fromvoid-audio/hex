//! One first-load UI intent; the Session worker owns source/assignment state.
use super::*;
use gpui::{App, Pixels, Point};
use hex_session::service::SamplerLoadSlot;
use std::{
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

#[derive(Clone)]
pub(super) struct Target {
    pub(super) generation: u64,
    pub(super) revision: u64,
    pub(super) track: TrackId,
    pub(super) path: PathBuf,
    pub(super) slot: SamplerLoadSlot,
}
struct Pending {
    id: u64,
    target: Target,
    cancel: Arc<AtomicBool>,
}
#[derive(Default)]
pub(super) struct State {
    serial: u64,
    pending: Option<Pending>,
}
impl EditorView {
    pub(super) fn first_sampler_current(&self) -> bool {
        let Some(state) = self.state.as_ref() else {
            return false;
        };
        let Some(audio) = self.audio.as_ref() else {
            return false;
        };
        let Some(audible) = self.audible_playback.as_ref() else {
            return false;
        };
        self.document_audio_mode == hex_session::DocumentAudioMode::Resident
            && hex_engine::native_device_resources::requires_private_sampler_sources(&state.project)
            && self.applied_project_revision() == Some(state.revision)
            && audible.project_generation == self.project_generation
            && audible.state.revision == state.revision
            && audible.device_preview_identity.is_none()
            && audible.tuning_preview_identity.is_none()
            && audio.control.status().is_some_and(|status| {
                status.applied_revision.load(Ordering::Acquire) == audible.audio_revision
            })
    }
    pub(super) fn first_sampler_target(&self, path: PathBuf) -> Result<Target, &'static str> {
        let track = self
            .selected_track
            .ok_or("select a midi track before loading a sampler")?;
        self.sampler_target_on_track(track, path)
    }
    fn sampler_target_on_track(
        &self,
        track: TrackId,
        path: PathBuf,
    ) -> Result<Target, &'static str> {
        if self.edit_pending() {
            return Err("wait for the pending edit before loading a sampler");
        }
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            return Err("sampler loading requires a resident project");
        }
        let state = self.state.as_ref().ok_or("project is still loading")?;
        let track = state
            .project
            .track(track)
            .ok_or("select a midi track before loading a sampler")?;
        let slot = if let Some(device) = track.instrument() {
            hex_engine::validate_sampler_replacement_on_control(device.state())
                .map_err(|_| "sample replacement currently requires classic mode")?;
            SamplerLoadSlot::Existing {
                device: device.id(),
                expected_state: device.state().clone(),
            }
        } else {
            SamplerLoadSlot::New { replacing: None }
        };
        Ok(Target {
            generation: self.project_generation,
            revision: state.revision,
            track: track.id(),
            path,
            slot,
        })
    }
    pub(super) fn first_sampler_target_current(&self, target: &Target) -> bool {
        target.generation == self.project_generation
            && self.state.as_ref().is_some_and(|s| {
                s.revision == target.revision
                    && s.project
                        .track(target.track)
                        .is_some_and(|track| match &target.slot {
                            SamplerLoadSlot::New { replacing } => {
                                track.instrument().map(hex_project::Device::id) == *replacing
                            }
                            SamplerLoadSlot::Existing {
                                device,
                                expected_state,
                            } => track.instrument().is_some_and(|current| {
                                current.id() == *device
                                    && current.state() == expected_state
                                    && !current.bypassed()
                            }),
                        })
            })
    }
    pub(super) fn open_first_sampler_menu(
        &mut self,
        path: PathBuf,
        position: Point<Pixels>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use super::context_menu::EditorContextAction;
        use crate::context_menu::{MenuItem, PopupMenu};
        let (label, action) = if let Some(pending) = self.sampler_load.pending.as_ref() {
            (
                "cancel sampler load",
                EditorContextAction::CancelSamplerLoad {
                    request_id: pending.id,
                },
            )
        } else if let Some(id) = self.sampler_replacement_pending_id() {
            (
                "cancel sample replacement",
                EditorContextAction::CancelSamplerLoad { request_id: id },
            )
        } else {
            if !device_panel::finalize_controls(self, cx) {
                return;
            }
            let target = match self.first_sampler_target(path) {
                Ok(t) => t,
                Err(e) => {
                    self.error = Some(e.into());
                    cx.notify();
                    return;
                }
            };
            (
                if matches!(&target.slot, SamplerLoadSlot::Existing { expected_state, .. }
                        if !hex_engine::is_empty_sampler_state_on_control(expected_state))
                {
                    "replace sampler sample"
                } else {
                    "load into sampler"
                },
                EditorContextAction::LoadFirstSampler(target),
            )
        };
        let previous_focus = self
            .context_menu
            .take()
            .and_then(|m| m.previous_focus)
            .or_else(|| window.focused(cx));
        let focus = cx.focus_handle();
        window.focus(&focus);
        self.context_menu = Some(PopupMenu {
            scroll: gpui::ScrollHandle::new(),
            position,
            icon_grid: None,
            selected: 0,
            focus,
            previous_focus,
            items: vec![MenuItem {
                label,
                swatch: None,
                checked: false,
                action,
            }],
        });
        cx.notify();
    }
    fn browser_sampler_drag_current(&self, drag: &drag::BrowserAudioDrag) -> bool {
        self.active_browser_audio_drag
            .as_ref()
            .is_some_and(|active| Rc::ptr_eq(&active.destination, &drag.destination))
            && !self.opening_project
            && !self.close_prompt_open
            && Some(self.browser.scan_generation()) == drag.browser_generation
            && drag.project_generation == self.project_generation
            && self
                .state
                .as_ref()
                .is_some_and(|state| state.revision == drag.revision)
    }

    pub(super) fn browser_sampler_drop_track(
        &self,
        drag: &drag::BrowserAudioDrag,
        track: Option<TrackId>,
        cx: &App,
    ) -> Result<&hex_project::Track, &'static str> {
        if !self.browser_sampler_drag_current(drag) {
            return Err("sample drop canceled · project or browser changed");
        }
        if self.sampler_load.pending.is_some() || self.sampler_replacement_pending_id().is_some() {
            return Err("sampler load is already pending");
        }
        let track = track.ok_or("drop the sample on an ordinary track")?;
        let target = self.check_device_append_target(track, cx)?;
        if self.edit_pending() {
            return Err("wait for the pending edit before loading a sampler");
        }
        if self.document_audio_mode != hex_session::DocumentAudioMode::Resident {
            return Err("sampler loading requires a resident project");
        }
        if target.is_group() || (target.instrument().is_some() && !target.receives_source_audio()) {
            return Err("drop the sample on an ordinary receiving track");
        }
        Ok(target)
    }

    /// Panel and header drops own the painted track's instrument slot. An
    /// occupied slot follows native header placement, preserving its effects.
    pub(super) fn drop_browser_sample_on_track(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        track: Option<TrackId>,
        cx: &mut Context<Self>,
    ) {
        let target = self
            .browser_sampler_drop_track(drag, track, cx)
            .map(|track| Target {
                generation: drag.project_generation,
                revision: drag.revision,
                track: track.id(),
                path: drag.path.clone(),
                slot: SamplerLoadSlot::New {
                    replacing: track.instrument().map(hex_project::Device::id),
                },
            });
        self.finish_browser_sampler_drop(drag, target, cx);
    }

    /// A sampler card edits the exact device and preserves its existing settings.
    pub(super) fn drop_browser_sample_on_sampler(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        track: TrackId,
        device: hex_project::DeviceId,
        cx: &mut Context<Self>,
    ) {
        let target = self.browser_sampler_drop_track(drag, Some(track), cx).and_then(|_| {
            let target = self.sampler_target_on_track(track, drag.path.clone())?;
            if matches!(&target.slot, SamplerLoadSlot::Existing { device: id, .. } if *id == device) {
                Ok(target)
            } else {
                Err("sample drop canceled · sampler changed")
            }
        });
        self.finish_browser_sampler_drop(drag, target, cx);
    }

    fn finish_browser_sampler_drop(
        &mut self,
        drag: &drag::BrowserAudioDrag,
        target: Result<Target, &'static str>,
        cx: &mut Context<Self>,
    ) {
        // The worker owns an admitted load. The retired drag must never reach
        // an underlying Arrangement/Console clip-drop receiver.
        self.clear_browser_audio_drag(drag);
        match target {
            Ok(target) => {
                self.load_first_sampler(target, cx);
                if self.sampler_load.pending.is_some()
                    || self.sampler_replacement_pending_id().is_some()
                {
                    self.stop_browser_preview_for_drop();
                }
            }
            Err(reason) => {
                self.error = Some(reason.into());
                cx.notify();
            }
        }
    }

    pub(super) fn load_first_sampler(&mut self, target: Target, cx: &mut Context<Self>) {
        if self.sampler_load.pending.is_some() {
            self.error = Some("sampler load is already pending".into());
            return;
        }
        if !self.first_sampler_target_current(&target) {
            self.error = Some("sampler load target changed".into());
            return;
        }
        if matches!(&target.slot, SamplerLoadSlot::Existing { expected_state, .. }
            if !hex_engine::is_empty_sampler_state_on_control(expected_state))
        {
            self.submit_sampler_replacement(target);
            cx.notify();
            return;
        }
        let Some(id) = self.sampler_load.serial.checked_add(1) else {
            self.error = Some("sampler load request id exhausted".into());
            return;
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let request = Request::LoadFirstSampler {
            request_id: id,
            project_generation: target.generation,
            revision: target.revision,
            track: target.track,
            slot: target.slot.clone(),
            path: target.path.clone(),
            cancel: Arc::clone(&cancel),
        };
        if self.enqueue(request) {
            self.sampler_load.serial = id;
            let name = target
                .path
                .file_name()
                .map(|n| n.to_string_lossy())
                .unwrap_or_default();
            self.status = format!("loading {name} into sampler");
            self.error = None;
            self.sampler_load.pending = Some(Pending { id, target, cancel });
        }
        cx.notify();
    }
    pub(super) fn cancel_first_sampler_load(&mut self, request_id: Option<u64>) {
        self.cancel_sampler_replacement(request_id);
        if let Some(pending) = self.sampler_load.pending.as_ref()
            && request_id.is_none_or(|id| id == pending.id)
        {
            pending.cancel.store(true, Ordering::Release);
            self.status = "cancelling sampler load".into();
        }
    }
    pub(super) fn retire_first_sampler_load(&mut self) {
        self.cancel_first_sampler_load(None);
        self.sampler_load.pending = None;
    }
    pub(super) fn first_sampler_pending_matches(&self, id: u64) -> bool {
        self.sampler_load
            .pending
            .as_ref()
            .is_some_and(|p| p.id == id)
            || self.sampler_replacement_pending_id() == Some(id)
    }
    pub(super) fn first_sampler_load_complete(
        &mut self,
        completion: Option<hex_session::service::SamplerLoadCompletion>,
        succeeded: bool,
    ) {
        let Some(done) = completion else { return };
        if !self.sampler_load.pending.as_ref().is_some_and(|p| {
            p.id == done.request_id
                && p.target.generation == done.project_generation
                && p.target.track == done.track
        }) {
            return;
        }
        let pending = self.sampler_load.pending.take();
        if done.project_generation != self.project_generation {
            return;
        }
        if !succeeded {
            self.status = if pending
                .as_ref()
                .is_some_and(|p| p.cancel.load(Ordering::Acquire))
            {
                "sampler load cancelled"
            } else {
                "sampler load refused"
            }
            .into();
            return;
        }
        // Display canonical identity, not the path retained by the request. Do
        // not select a track/device, move the viewport or focus any control.
        let name = self
            .state
            .as_ref()
            .and_then(|s| {
                let reference = s
                    .project
                    .track(done.track)?
                    .instrument()?
                    .state()
                    .asset_references()
                    .first()?;
                s.project
                    .assets()
                    .find(|a| a.id().value() == reference.asset.value())?
                    .path()
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
            })
            .unwrap_or_else(|| "sample".into());
        self.status = format!("loaded {name} into sampler");
    }
}

pub(super) fn track_drop_listener(
    cx: &Context<EditorView>,
    track: Option<TrackId>,
) -> impl Fn(&drag::BrowserAudioDrag, &mut Window, &mut App) + 'static {
    cx.listener(move |view, drag: &drag::BrowserAudioDrag, _, cx| {
        view.drop_browser_sample_on_track(drag, track, cx);
    })
}

#[cfg(feature = "ui-test-support")]
pub(super) struct FixturePaths {
    pub(super) home: PathBuf,
    pub(super) directory: PathBuf,
}
