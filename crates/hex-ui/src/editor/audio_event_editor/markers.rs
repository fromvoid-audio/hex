//! Waveform anchors edit the existing warp map, scoped to a direct clip or a
//! selected event. The edit receipt owns the marker map after release.
use super::{EditorView, Gesture, Kind, transients};
use crate::{editor::arrangement_audio_panel::controls, theme};
use gpui::{Context, Div, MouseButton, Stateful, Window, div, prelude::*, px, relative, rgb};
use hex_project::{
    AudioClipEdit, AudioDomainClock, AudioEvent, AudioEventEdit, AudioEvents, BeatTime, Command,
    Invalid, TempoMap, WarpMarker, WarpMode, WarpSettings,
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

#[derive(Clone, Copy)]
enum EventWarpClock<'a> {
    Beats,
    RetainedFrames {
        edit: &'a AudioClipEdit,
        tempo: &'a TempoMap,
        view_origin: u64,
    },
}

/// Map the selected source's markers into only this event's destination.
pub(super) struct EventWarp<'a> {
    pub(super) event: AudioEvent,
    pub(super) markers: &'a [WarpMarker],
    mode: WarpMode,
    clock: EventWarpClock<'a>,
}

impl<'a> EventWarp<'a> {
    pub(super) fn new(
        events: &'a AudioEvents,
        root: &'a AudioClipEdit,
        selected: u64,
    ) -> Option<Self> {
        let event = *events
            .events
            .iter()
            .find(|event| event.id == selected && event.active)?;
        if events.sources.is_empty()
            && matches!(root.domain.clock, AudioDomainClock::Beats(_))
            && transients::supports_root(root)
        {
            let base = root
                .domain
                .warp
                .as_ref()
                .filter(|_| root.domain.warp_enabled)?;
            let markers = if event.source == 0 {
                root.event_warp_markers(0)?
            } else {
                events.warp_source(event.source)?.markers.as_ref()
            };
            return Some(Self {
                event,
                markers,
                mode: base.mode,
                clock: EventWarpClock::Beats,
            });
        }
        let source = events
            .sources
            .iter()
            .find(|source| source.id == event.source)?;
        let edit = source.clip.audio_edit()?;
        if source.clip.looping()
            || source.clip.has_retained_audio_roots()
            || edit.content_view.is_some()
            || edit.placement.is_some()
            || edit.processing_projection.is_some()
            || !edit.domain.warp_enabled
        {
            return None;
        }
        let settings = edit.domain.warp.as_ref()?;
        let view_origin = edit.root_frame_at(edit.view_start, &source.tempo).ok()?;
        Some(Self {
            event,
            markers: &settings.markers,
            mode: settings.mode,
            clock: EventWarpClock::RetainedFrames {
                edit,
                tempo: &source.tempo,
                view_origin,
            },
        })
    }

    pub(super) fn settings(&self) -> WarpSettings {
        WarpSettings {
            mode: self.mode,
            markers: self.markers.to_vec(),
        }
    }

    pub(super) fn position(&self, beat: BeatTime) -> Option<u64> {
        let source = match self.clock {
            EventWarpClock::Beats => beat.raw(),
            EventWarpClock::RetainedFrames {
                edit,
                tempo,
                view_origin,
            } => edit
                .root_frame_at(beat.raw(), tempo)
                .ok()?
                .checked_sub(view_origin)?,
        };
        let offset = source.checked_sub(self.event.source_start)?;
        (offset < self.event.length()).then(|| self.event.start + offset)
    }

    fn drag_beat(&self, source_frame: u64, anchor: u64, position: u64) -> Option<BeatTime> {
        let settings = self.settings();
        let original = settings.beat_for_source_frame(source_frame).ok()?;
        let destination = (i128::from(self.position(original)?) + i128::from(position)
            - i128::from(anchor))
        .clamp(i128::from(self.event.start), i128::from(self.event.end - 1))
            as u64;
        let source = self
            .event
            .source_start
            .checked_add(destination - self.event.start)?;
        let beat = match self.clock {
            EventWarpClock::Beats => source,
            EventWarpClock::RetainedFrames {
                edit,
                tempo,
                view_origin,
            } => edit
                .root_position_at(view_origin.checked_add(source)?, tempo)
                .ok()?,
        };
        settings
            .marker_target_beat(source_frame, BeatTime::from_raw(beat))
            .ok()
    }
}

pub(super) fn marker_handle(
    source_frame: u64,
    event: Option<u64>,
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
                    if let Some(event) = event {
                        view.commit_event_marker(event, source_frame, None);
                    } else {
                        view.edit_event_warp_marker(source_frame, authored);
                    }
                } else {
                    view.begin_event_warp_drag(
                        source_frame,
                        event,
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

    fn warp_event_clip(&self) -> Option<&hex_project::Clip> {
        self.state()?.project.clip(self.detail_clip_id()?)
    }

    pub(super) fn drag_scoped_marker(
        &self,
        event: u64,
        source_frame: u64,
        anchor: u64,
        position: u64,
    ) -> Option<BeatTime> {
        let clip = self.warp_event_clip()?;
        EventWarp::new(
            &clip.editable_audio_events().ok()?,
            &clip.audio_event_root(),
            event,
        )?
        .drag_beat(source_frame, anchor, position)
    }

    pub(super) fn commit_event_marker(
        &mut self,
        event: u64,
        source_frame: u64,
        beat: Option<BeatTime>,
    ) {
        let Some(clip) = self.warp_event_clip() else {
            return;
        };
        let Ok(events) = clip.editable_audio_events() else {
            return;
        };
        let root = clip.audio_event_root();
        let Some(target) = EventWarp::new(&events, &root, event) else {
            return;
        };
        let mut settings = target.settings();
        let changed = match beat {
            Some(beat) => settings.insert_marker(source_frame).and_then(|inserted| {
                settings
                    .move_marker(source_frame, beat)
                    .map(|moved| inserted || moved)
            }),
            None if settings
                .markers
                .binary_search_by_key(&source_frame, |marker| marker.source_frame)
                .is_ok() =>
            {
                settings.remove_marker(source_frame).map(|()| true)
            }
            None => settings.insert_marker(source_frame),
        };
        match changed {
            Ok(true) => self.event_commit(AudioEventEdit::SetRetainedWarp { event, settings }),
            Ok(false) => {}
            Err(error) => self.error = Some(error.0.into()),
        }
    }

    fn begin_event_warp_drag(
        &mut self,
        source_frame: u64,
        event: Option<u64>,
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
        if self.audio_clip_controls.has_pending() || self.audio_event_editor.pending.is_some() {
            return;
        }
        let beat = if let Some(event) = event {
            clip.editable_audio_events().ok().and_then(|events| {
                let target = EventWarp::new(&events, &root, event)?;
                target.settings().beat_for_source_frame(source_frame).ok()
            })
        } else {
            direct_event(&root, clip)
                .filter(|_| root.domain.warp_enabled)
                .and(root.domain.warp.as_ref())
                .and_then(|warp| warp.beat_for_source_frame(source_frame).ok())
        };
        let Some(beat) = beat else {
            return;
        };
        let Some(anchor) = self.event_hit_position(&root, x) else {
            return;
        };
        let kind = Kind::WarpMarker {
            source_frame,
            beat,
            event,
        };
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
