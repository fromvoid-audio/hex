//! Waveform anchors edit the existing validated clip warp map for one direct
//! event. The clip-detail receipt owns the frozen marker map after release.
use super::{EditorView, Gesture, Kind, transients};
use crate::{editor::arrangement_audio_panel::controls, theme};
use gpui::{Context, Div, MouseButton, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{
    AudioClipEdit, AudioDomainClock, AudioEvent, BeatTime, Command, Invalid, TempoMap, WarpSettings,
};

pub(super) fn direct_event(root: &AudioClipEdit, clip: &hex_project::Clip) -> Option<AudioEvent> {
    if !transients::supports_root(root) {
        return None;
    }
    let events = clip.editable_audio_events().ok()?;
    let [event] = events.events.as_slice() else {
        return None;
    };
    // The root owns the clock and warp settings. Do not retime another event
    // or reinterpret a partial, moved or independently retained read.
    (event.source == 0
        && event.active
        && event.start == 0
        && event.source_start == 0
        && event.end == root.domain.clock.length()
        && root.view_start == 0
        && root.view_end == event.end)
        .then_some(*event)
}

pub(super) fn asset_frame_at(root: &AudioClipEdit, position: u64, tempo: &TempoMap) -> Option<u64> {
    let offset = match root.domain.clock {
        AudioDomainClock::Frames(_) => position,
        AudioDomainClock::Beats(_) if root.domain.warp_enabled => {
            let source = root
                .domain
                .warp_source_position_q32(u128::from(position))
                .ok()?;
            u64::try_from(source >> 32).ok()?
        }
        AudioDomainClock::Beats(_) => root.event_source_frame_at(0, position, tempo).ok()?,
    };
    if offset >= root.domain.source_length {
        return None;
    }
    root.domain.source_offset.checked_add(offset)
}

pub(super) fn marker_handle(
    source_frame: u64,
    x: f32,
    top: f32,
    authored: bool,
    cx: &mut Context<EditorView>,
) -> Stateful<Div> {
    let role = if authored {
        theme::PaletteRole::Primary
    } else {
        theme::PaletteRole::Muted
    };
    div()
        .id(("event-warp-marker", source_frame))
        .debug_selector(move || {
            if authored {
                "audio-event-warp-marker".into()
            } else {
                "audio-event-transient-hit".into()
            }
        })
        .absolute()
        .left(relative(x))
        .ml(px(-3.0))
        .top(px(top))
        .w(px(7.0))
        .h(px(9.0))
        .cursor(gpui::CursorStyle::ResizeLeftRight)
        .tooltip(move |_, cx| {
            cx.new(|_| super::super::waveform::ClipTooltip {
                kind: crate::tooltips::Kind::control("warp marker"),
                name: if authored {
                    "drag to retime · double-click to remove"
                } else {
                    "double-click to pin this transient"
                }
                .into(),
            })
            .into()
        })
        .child(
            div()
                .debug_selector(move || {
                    if authored {
                        "audio-event-warp-marker-ink".into()
                    } else {
                        "audio-event-transient".into()
                    }
                })
                .mx_auto()
                .w(px(if authored { 5.0 } else { 1.0 }))
                .h_full()
                .bg(rgb(theme::color(role))),
        )
        .on_mouse_down(
            MouseButton::Left,
            cx.listener(move |view, down: &gpui::MouseDownEvent, window, cx| {
                if view.time_selection_key_held
                    || view.audio_event_editor.fade_key_held
                    || !view.audio_navigation_idle()
                {
                    return;
                }
                if down.click_count == 2 {
                    view.edit_event_warp_marker(source_frame, authored);
                } else {
                    view.begin_event_warp_drag(
                        source_frame,
                        f32::from(down.position.x),
                        window,
                        cx,
                    );
                }
                cx.stop_propagation();
                cx.notify();
            }),
        )
}

impl EditorView {
    fn edit_event_warp_marker(&mut self, source_frame: u64, remove: bool) {
        let Some((clip_id, root, _)) = self.event_context() else {
            return;
        };
        let Some(state) = self.state() else {
            return;
        };
        let Some(clip) = state.project.clip(clip_id) else {
            return;
        };
        if direct_event(&root, clip).is_none() || self.audio_clip_controls.has_pending() {
            return;
        }
        let change = state
            .project
            .audio_warp_settings_for_enable(clip_id)
            .and_then(|mut settings| {
                let changed = if remove {
                    settings.remove_marker(source_frame)?;
                    true
                } else {
                    settings.insert_marker(source_frame)?
                };
                Ok(changed.then_some(settings))
            });
        self.submit_event_warp(clip_id, change);
    }

    pub(super) fn insert_event_warp_marker_at(&mut self, position: u64) {
        let source = self.event_context().and_then(|(_, root, _)| {
            asset_frame_at(&root, position, self.state()?.project.tempo_map())
        });
        if let Some(source) = source {
            self.edit_event_warp_marker(source, false);
        }
    }

    fn begin_event_warp_drag(
        &mut self,
        source_frame: u64,
        x: f32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some((clip_id, root, _)) = self.event_context() else {
            return;
        };
        let Some(clip) = self.state().and_then(|s| s.project.clip(clip_id)) else {
            return;
        };
        if direct_event(&root, clip).is_none()
            || !root.domain.warp_enabled
            || self.audio_clip_controls.has_pending()
        {
            return;
        }
        let Some(beat) = root
            .domain
            .warp
            .as_ref()
            .and_then(|warp| warp.beat_for_source_frame(source_frame).ok())
        else {
            return;
        };
        let Some(anchor) = self.event_hit_position(&root, x) else {
            return;
        };
        let kind = Kind::WarpMarker { source_frame, beat };
        self.audio_event_focus.focus(window);
        if self.capture_audio_event_pointer(kind, window, cx) {
            self.audio_event_editor.gesture = Some(Gesture {
                anchor,
                pointer_x: x,
                origin_x: x,
                moved: false,
                kind,
                operation: None,
                placement: None,
            });
        }
    }

    pub(super) fn finish_event_warp_drag(&mut self, source_frame: u64, beat: BeatTime) {
        let Some((clip_id, root, _)) = self.event_context() else {
            return;
        };
        let Some(mut settings) = root.domain.warp.clone() else {
            return;
        };
        let changed = settings.insert_marker(source_frame).and_then(|inserted| {
            settings
                .move_marker(source_frame, beat)
                .map(|moved| inserted || moved)
        });
        self.submit_event_warp(clip_id, changed.map(|changed| changed.then_some(settings)));
    }

    fn submit_event_warp(
        &mut self,
        clip: hex_project::ClipId,
        change: Result<Option<WarpSettings>, Invalid>,
    ) {
        let generation = self.project_generation;
        match change {
            Ok(Some(settings)) => controls::queue(
                self,
                generation,
                Command::SetAudioClipWarp {
                    clip,
                    settings: Some(settings),
                },
            ),
            Ok(None) => {}
            Err(error) => self.error = Some(error.0.into()),
        }
    }
}

pub(super) fn drag_beat(
    root: &AudioClipEdit,
    source_frame: u64,
    pointer_start: u64,
    pointer: u64,
) -> Option<BeatTime> {
    let warp = root.domain.warp.as_ref()?;
    let original = warp.beat_for_source_frame(source_frame).ok()?.raw();
    let target = (i128::from(original) + i128::from(pointer) - i128::from(pointer_start))
        .clamp(0, i128::from(u64::MAX)) as u64;
    warp.marker_target_beat(source_frame, BeatTime::from_raw(target))
        .ok()
}
