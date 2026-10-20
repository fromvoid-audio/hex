//! Clip-local waveform editing. Only selection, viewport and previews live here.
#[cfg(all(test, feature = "ui-test-support"))]
mod tests;
mod view;
use super::{EditorView, musical_time::SnapGrid, viewport::Viewport};
use gpui::{Bounds, Pixels};
use hex_project::{
    AudioClipDomainFacts, AudioClipRootFacts, AudioEvent, AudioEventEdit, AudioEvents, ClipId,
    Invalid, SourceGainPoint, TempoPoint, WarpMarker,
};
use hex_session::snapshot_exports::{SnapshotRead, SnapshotSourceKey, SnapshotTransportError};
pub(super) use view::view;

#[derive(Default)]
pub(super) struct State {
    owner: Option<(ClipId, u64)>,
    revision: u64,
    source_key: Option<SnapshotSourceKey>,
    pub(super) source_view: bool,
    viewport: Option<Viewport>,
    cursor: u64,
    selected: Option<u64>,
    range: Option<(u64, u64)>,
    grid: SnapGrid,
    bounds: Option<Bounds<Pixels>>,
    pub(super) gesture: Option<Gesture>,
    clipboard: Option<Clipboard>,
}
struct Clipboard {
    owner: (ClipId, u64),
    domain: DomainToken,
    length: u64,
    events: Vec<AudioEvent>,
}
/// Exact domain equality, excluding only events and view/placement metadata.
/// These bounded scalar lists are copied into preallocated storage; no root owner
/// or accepted waveform/PCM is retained by the editor.
#[derive(Debug, PartialEq, Eq)]
struct DomainToken {
    facts: AudioClipDomainFacts,
    warp: Vec<WarpMarker>,
    gain: Vec<SourceGainPoint>,
    tempo: Vec<TempoPoint>,
}
impl DomainToken {
    fn empty(facts: AudioClipDomainFacts) -> Result<Self, Invalid> {
        let tempo_count = facts.fade_projection.map_or(0, |f| f.tempo_point_count);
        if facts.warp_marker_count > hex_project::MAX_AUDIO_POINTS
            || facts.source_gain_point_count > hex_project::MAX_AUDIO_POINTS
            || tempo_count > hex_project::MAX_TEMPO_POINTS
        {
            return Err(Invalid(
                "audio event metadata exceeds current format limits",
            ));
        }
        Ok(Self {
            facts,
            warp: Vec::with_capacity(facts.warp_marker_count),
            gain: Vec::with_capacity(facts.source_gain_point_count),
            tempo: Vec::with_capacity(tempo_count),
        })
    }
    fn fill(&mut self, read: &hex_project::AudioClipRootRead<'_>) -> Result<(), Invalid> {
        if read.facts().domain != self.facts {
            return Err(Invalid("audio event domain changed during capture"));
        }
        fill_exact(
            &mut self.warp,
            read.warp_markers(),
            self.facts.warp_marker_count,
        )?;
        fill_exact(
            &mut self.gain,
            read.source_gain_points(),
            self.facts.source_gain_point_count,
        )?;
        fill_exact(
            &mut self.tempo,
            read.fade_projection_tempo_points(),
            self.facts
                .fade_projection
                .map_or(0, |f| f.tempo_point_count),
        )
    }
}
#[derive(Debug)]
enum ReadError {
    Transport(SnapshotTransportError),
    Model(Invalid),
}
impl From<SnapshotTransportError> for ReadError {
    fn from(error: SnapshotTransportError) -> Self {
        Self::Transport(error)
    }
}
impl From<Invalid> for ReadError {
    fn from(error: Invalid) -> Self {
        Self::Model(error)
    }
}
type ReadResult<T> = Result<Option<T>, ReadError>;
struct EventData {
    clip: ClipId,
    root: AudioClipRootFacts,
    revision: u64,
    source_key: SnapshotSourceKey,
    events: AudioEvents,
    domain: DomainToken,
}
#[derive(Clone)]
pub(super) struct Gesture {
    anchor: u64,
    pointer_x: f32,
    origin_x: f32,
    moved: bool,
    kind: Kind,
    operation: Option<AudioEventEdit>,
}
#[derive(Clone, Copy)]
enum Kind {
    Select,
    RangeMove { start: u64, end: u64 },
    Move(AudioEvent),
    Leading(AudioEvent),
    Trailing(AudioEvent),
}

impl EditorView {
    fn event_source_key(&self) -> ReadResult<SnapshotSourceKey> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        state
            .source_key()
            .map(Some)
            .ok_or_else(|| SnapshotTransportError::Stale.into())
    }
    fn event_read<R>(
        &self,
        inspect: impl for<'a> FnOnce(SnapshotRead<'a>, hex_project::AudioClipRootRead<'a>) -> R,
    ) -> ReadResult<R> {
        let Some(state) = self.state.as_ref() else {
            return Ok(None);
        };
        let Some(clip) = self.selected_clip else {
            return Ok(None);
        };
        // Correlation only; the actual read below still checks availability.
        // An absent source key must never fall back to a numeric revision.
        state.source_key().ok_or(SnapshotTransportError::Stale)?;
        Ok(state.read_with(|snapshot| {
            snapshot.with_audio_event_root(clip, |root| inspect(snapshot, root))
        })??)
    }
    fn event_context(&self) -> ReadResult<(ClipId, AudioClipRootFacts, u64)> {
        let Some(clip) = self.selected_clip else {
            return Ok(None);
        };
        self.event_read(|snapshot, root| (clip, root.facts(), snapshot.revision()))
    }
    fn event_context_current(&self) -> Result<bool, ReadError> {
        let Some(source_key) = self.event_source_key()? else {
            return Ok(false);
        };
        Ok(self.event_context()?.is_some_and(|(clip, _, revision)| {
            self.audio_event_editor.owner == Some((clip, self.project_generation))
                && self.audio_event_editor.revision == revision
                && self.audio_event_editor.source_key == Some(source_key)
        }))
    }
    fn event_error(&mut self, error: ReadError) {
        match error {
            // Contention is retryable: keep the exact gesture/clipboard/range.
            ReadError::Transport(SnapshotTransportError::Busy) => {}
            ReadError::Transport(_) => {
                self.error = Some("audio event source is unavailable".into())
            }
            ReadError::Model(error) => self.error = Some(error.0.into()),
        }
    }
    fn event_data(&self) -> ReadResult<EventData> {
        let Some(clip) = self.selected_clip else {
            return Ok(None);
        };
        let Some(source_key) = self.event_source_key()? else {
            return Ok(None);
        };
        let Some((root, revision, next_id, count)) = self
            .event_read(|snapshot, root| {
                let events = root.editable_events()?;
                Ok::<_, Invalid>((
                    root.facts(),
                    snapshot.revision(),
                    events.next_id(),
                    events.len(),
                ))
            })?
            .transpose()?
        else {
            return Ok(None);
        };
        if count > hex_project::MAX_AUDIO_EVENTS + 2 {
            return Err(Invalid("audio event metadata exceeds current format limits").into());
        }
        // Allocate outside the read. The immutable retained capability is the
        // same in both passes, and every fill is capacity checked before push.
        let mut data = EventData {
            clip,
            root,
            revision,
            source_key,
            events: AudioEvents {
                next_id,
                events: Vec::with_capacity(count),
            },
            domain: DomainToken::empty(root.domain)?,
        };
        let filled = self
            .event_read(|snapshot, read| {
                let events = read.editable_events()?;
                if snapshot.revision() != revision
                    || read.facts() != root
                    || events.next_id() != next_id
                    || events.len() != count
                {
                    return Err(Invalid("audio event source changed during capture"));
                }
                fill_exact(&mut data.events.events, events.iter(), count)?;
                data.domain.fill(&read)
            })?
            .transpose()?;
        Ok(filled.map(|()| data))
    }
    fn event_commit(&mut self, operation: AudioEventEdit) -> bool {
        if self.edit_pending() {
            return false;
        }
        match self.event_context_current() {
            Ok(true) => {}
            Ok(false) => return false,
            Err(error) => {
                self.event_error(error);
                return false;
            }
        }
        let Some(clip) = self.selected_clip else {
            return false;
        };
        if !self.enqueue(hex_session::service::Request::Edit(
            hex_project::Command::EditAudioEvents { clip, operation },
        )) {
            return false;
        }
        self.audio_event_editor.gesture = None;
        true
    }
    fn event_range_in(&self, events: &AudioEvents) -> Option<(u64, u64)> {
        if let Some(range) = self.audio_event_editor.range.filter(|(a, b)| a < b) {
            return Some(range);
        }
        let selected = self.audio_event_editor.selected?;
        events
            .events
            .iter()
            .find(|e| e.id == selected)
            .map(|e| (e.start, e.end))
    }
    fn event_copy(&mut self, cut: bool) {
        if cut && self.edit_pending() {
            return;
        }
        let data = match self.event_data() {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let Some((start, end)) = self.event_range_in(&data.events) else {
            return;
        };
        let clipboard = Clipboard {
            owner: (data.clip, self.project_generation),
            domain: data.domain,
            length: end - start,
            events: data.events.copy_range(start, end),
        };
        // Cut must be admitted before publishing any local clipboard/cursor
        // mutation. A Busy read or refused enqueue leaves the old state intact.
        if cut && !self.event_commit(AudioEventEdit::Delete { start, end }) {
            return;
        }
        self.audio_event_editor.clipboard = Some(clipboard);
        self.audio_event_editor.cursor = start;
    }
    fn event_paste(&mut self, insert: bool) {
        let data = match self.event_data() {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let Some(clipboard) = self.audio_event_editor.clipboard.as_ref() else {
            return;
        };
        if clipboard.owner != (data.clip, self.project_generation)
            || data.domain != clipboard.domain
        {
            self.error = Some("audio event clipboard belongs to a different or changed clip; copy the events again".into());
            return;
        }
        if !event_paste_fits_view(&data.root, self.audio_event_editor.cursor, clipboard.length) {
            self.error = Some("audio event paste exceeds the visible clip".into());
            return;
        }
        let last_event_end = data.events.events.last().map_or(0, |event| event.end);
        if insert
            && last_event_end
                .checked_add(clipboard.length)
                .is_none_or(|end| end > data.root.view_end)
        {
            self.error = Some("audio event insert needs free time inside the visible clip".into());
            return;
        }
        let operation = if insert {
            AudioEventEdit::Insert {
                at: self.audio_event_editor.cursor,
                length: clipboard.length,
                events: clipboard.events.clone(),
            }
        } else {
            AudioEventEdit::Paste {
                at: self.audio_event_editor.cursor,
                length: clipboard.length,
                events: clipboard.events.clone(),
            }
        };
        self.event_commit(operation);
    }
    fn event_delete(&mut self) {
        let data = match self.event_data() {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        if let Some((start, end)) = self.event_range_in(&data.events)
            && self.event_commit(AudioEventEdit::Delete { start, end })
        {
            self.audio_event_editor.range = None;
            self.audio_event_editor.selected = None;
        }
    }
    fn event_split(&mut self) {
        let root = match self.event_context() {
            Ok(Some((_, root, _))) => root,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        if let Some((start, end)) = self.audio_event_editor.range.filter(|(a, b)| a < b) {
            self.event_commit(AudioEventEdit::SplitRange { start, end });
            return;
        }
        let at = self.audio_event_editor.cursor;
        if at > root.view_start && at < root.view_end {
            self.event_commit(AudioEventEdit::Split { at });
        }
    }
    fn event_frame_at(&self, position: u64) -> ReadResult<u64> {
        self.event_read(|_, root| root.root_frame_at(position))?
            .transpose()
            .map_err(Into::into)
    }
    fn event_viewport(&self, root: &AudioClipRootFacts) -> ReadResult<Viewport> {
        let Some(first) = self.event_frame_at(root.view_start)? else {
            return Ok(None);
        };
        let Some(end) = self.event_frame_at(root.view_end)? else {
            return Ok(None);
        };
        Ok(Some(
            self.audio_event_editor
                .viewport
                .filter(|_| {
                    self.audio_event_editor.owner
                        == self.selected_clip.map(|id| (id, self.project_generation))
                })
                .unwrap_or(Viewport {
                    start: 0,
                    length: end - first,
                })
                .clamp(end - first),
        ))
    }
    fn event_project_viewport(&self, root: &AudioClipRootFacts) -> ReadResult<Viewport> {
        let Some(local) = self.event_viewport(root)? else {
            return Ok(None);
        };
        self.event_read(|snapshot, _| {
            let clip = snapshot.model().clip(self.selected_clip?)?;
            Some(Viewport {
                start: clip.start().checked_add(local.start)?,
                length: local.length,
            })
        })
        .map(|value| value.flatten())
    }
    fn event_pointer_position(&self, x: f32, platform: bool) -> ReadResult<u64> {
        let Some((clip_id, root, _)) = self.event_context()? else {
            return Ok(None);
        };
        let Some(bounds) = self.audio_event_editor.bounds else {
            return Ok(None);
        };
        let width = f32::from(bounds.size.width);
        if !width.is_finite() || width <= 0.0 {
            return Ok(None);
        }
        let Some(viewport) = self.event_project_viewport(&root)? else {
            return Ok(None);
        };
        let fraction = ((x - f32::from(bounds.origin.x)) / width).clamp(0.0, 1.0);
        let candidate = viewport
            .start
            .checked_add((fraction as f64 * viewport.length as f64).round() as u64)
            .ok_or(Invalid("audio event pointer position overflows"))?;
        self.event_read(|snapshot, read| {
            let clip = snapshot
                .model()
                .clip(clip_id)
                .ok_or(Invalid("audio event clip is unavailable"))?;
            let Some(snapped) = self
                .audio_event_editor
                .grid
                .with_platform_modifier(platform)
                .snap_frame(&snapshot.model(), candidate, viewport, width)
            else {
                return Ok(None);
            };
            let first = read.root_frame_at(root.view_start)?;
            let local = snapped.saturating_sub(clip.start()).min(clip.duration());
            let frame = first
                .checked_add(local)
                .ok_or(Invalid("audio event pointer position overflows"))?;
            read.root_position_at(frame).map(Some)
        })?
        .transpose()
        .map(Option::flatten)
        .map_err(Into::into)
    }
    fn event_snap_position(&self, position: u64, platform: bool) -> ReadResult<u64> {
        let Some((clip_id, root, _)) = self.event_context()? else {
            return Ok(None);
        };
        let Some(viewport) = self.event_project_viewport(&root)? else {
            return Ok(None);
        };
        let Some(bounds) = self.audio_event_editor.bounds else {
            return Ok(None);
        };
        let width = f32::from(bounds.size.width);
        self.event_read(|snapshot, read| {
            let clip = snapshot
                .model()
                .clip(clip_id)
                .ok_or(Invalid("audio event clip is unavailable"))?;
            let first = read.root_frame_at(root.view_start)?;
            let frame = clip
                .start()
                .checked_add(read.root_frame_at(position)?.saturating_sub(first))
                .ok_or(Invalid("audio event snap position overflows"))?;
            let Some(snapped) = self
                .audio_event_editor
                .grid
                .with_platform_modifier(platform)
                .snap_frame(&snapshot.model(), frame, viewport, width)
            else {
                return Ok(None);
            };
            let local = first
                .checked_add(snapped.saturating_sub(clip.start()).min(clip.duration()))
                .ok_or(Invalid("audio event snap position overflows"))?;
            read.root_position_at(local).map(Some)
        })?
        .transpose()
        .map(Option::flatten)
        .map_err(Into::into)
    }
    fn event_update_pointer(&mut self, x: f32, platform: bool) -> bool {
        match self.event_context_current() {
            Ok(true) if !self.edit_pending() => {}
            Ok(_) => {
                self.audio_event_editor.gesture = None;
                return false;
            }
            Err(error) => {
                self.event_error(error);
                return false;
            }
        }
        let Some(mut gesture) = self.audio_event_editor.gesture.clone() else {
            return false;
        };
        gesture.pointer_x = x;
        if !matches!(gesture.kind, Kind::Select) && !gesture.moved {
            if (x - gesture.origin_x).abs() < 3.0 {
                self.audio_event_editor.gesture = Some(gesture);
                return true;
            }
            gesture.moved = true;
        }

        let root = match self.event_context() {
            Ok(Some((_, root, _))) => root,
            Ok(None) => return false,
            Err(error) => {
                self.event_error(error);
                return false;
            }
        };
        let Ok(Some(raw)) =
            self.event_pointer_position(x, self.audio_event_editor.grid != SnapGrid::Off)
        else {
            return false;
        };
        let Ok(Some(position)) = self.event_pointer_position(x, platform) else {
            return false;
        };
        match gesture.kind {
            Kind::Select => {
                self.audio_event_editor.range =
                    Some((gesture.anchor.min(position), gesture.anchor.max(position)));
                self.audio_event_editor.cursor = position;
            }
            Kind::RangeMove { start, end } => {
                let candidate = (i128::from(start) + i128::from(raw) - i128::from(gesture.anchor))
                    .clamp(
                        i128::from(root.view_start),
                        i128::from(
                            root.view_end
                                .saturating_sub(end - start)
                                .max(root.view_start),
                        ),
                    ) as u64;
                let Ok(Some(destination)) = self.event_snap_position(candidate, platform) else {
                    return false;
                };
                let destination = destination.clamp(
                    root.view_start,
                    root.view_end
                        .saturating_sub(end - start)
                        .max(root.view_start),
                );
                gesture.operation = Some(AudioEventEdit::MoveRange {
                    start,
                    end,
                    destination,
                });
            }
            Kind::Move(event) => {
                let candidate =
                    (i128::from(event.start) + i128::from(raw) - i128::from(gesture.anchor)).clamp(
                        i128::from(root.view_start),
                        i128::from(
                            root.view_end
                                .saturating_sub(event.length())
                                .max(root.view_start),
                        ),
                    ) as u64;
                let Ok(Some(start)) = self.event_snap_position(candidate, platform) else {
                    return false;
                };
                let start = start.clamp(
                    root.view_start,
                    root.view_end
                        .saturating_sub(event.length())
                        .max(root.view_start),
                );
                gesture.operation = Some(AudioEventEdit::Move {
                    event: event.id,
                    start,
                });
            }
            Kind::Leading(event) => {
                let start = position.clamp(root.view_start.min(event.end - 1), event.end - 1);
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start,
                    end: event.end,
                });
            }
            Kind::Trailing(event) => {
                let end = position.clamp(event.start + 1, root.view_end.max(event.start + 1));
                gesture.operation = Some(AudioEventEdit::Trim {
                    event: event.id,
                    start: event.start,
                    end,
                });
            }
        }
        self.audio_event_editor.gesture = Some(gesture);
        true
    }
    fn event_nudge(&mut self, right: bool, trim: bool, platform: bool) {
        let data = match self.event_data() {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let root = data.root;
        let Some(event) = data
            .events
            .events
            .iter()
            .find(|e| Some(e.id) == self.audio_event_editor.selected)
            .copied()
        else {
            return;
        };
        let position = if trim { event.end } else { event.start };
        let Ok(Some(viewport)) = self.event_project_viewport(&root) else {
            return;
        };
        let grid = self
            .audio_event_editor
            .grid
            .with_platform_modifier(platform);
        let width = self
            .audio_event_editor
            .bounds
            .map_or(640.0, |b| f32::from(b.size.width));
        let next = self.event_read(|snapshot, read| {
            let clip = snapshot
                .model()
                .clip(data.clip)
                .ok_or(Invalid("audio event clip is unavailable"))?;
            let first = read.root_frame_at(root.view_start)?;
            let frame = clip
                .start()
                .saturating_add(read.root_frame_at(position)?.saturating_sub(first));
            let next = if grid == SnapGrid::Off {
                Some(if right {
                    frame.saturating_add(1)
                } else {
                    frame.saturating_sub(1)
                })
            } else {
                grid.adjacent_frame(&snapshot.model(), frame, viewport, width, right)
            };
            let Some(next) = next else {
                return Ok(None);
            };
            let frame = first
                .checked_add(next.saturating_sub(clip.start()).min(clip.duration()))
                .ok_or(Invalid("audio event nudge position overflows"))?;
            read.root_position_at(frame).map(Some)
        });
        let next = match next {
            Ok(Some(Ok(Some(next)))) => next,
            Ok(Some(Err(error))) => {
                self.event_error(error.into());
                return;
            }
            Err(error) => {
                self.event_error(error);
                return;
            }
            _ => return,
        };
        if trim {
            if next > event.start {
                self.event_commit(AudioEventEdit::Trim {
                    event: event.id,
                    start: event.start,
                    end: next,
                });
            }
        } else {
            let start = next.clamp(
                root.view_start,
                root.view_end
                    .saturating_sub(event.length())
                    .max(root.view_start),
            );
            self.event_commit(AudioEventEdit::Move {
                event: event.id,
                start,
            });
        }
    }
    fn event_select_all(&mut self) {
        if let Ok(Some((_, root, _))) = self.event_context() {
            self.audio_event_editor.range = Some((root.view_start, root.view_end));
            self.audio_event_editor.cursor = root.view_start;
        }
    }
    fn event_duplicate(&mut self) {
        let data = match self.event_data() {
            Ok(Some(data)) => data,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let Some((start, end)) = self.event_range_in(&data.events) else {
            return;
        };
        if !event_paste_fits_view(&data.root, end, end - start) {
            self.error = Some("audio event duplicate exceeds the visible clip".into());
            return;
        }
        self.event_commit(AudioEventEdit::Paste {
            at: end,
            length: end - start,
            events: data.events.copy_range(start, end),
        });
    }
    pub(super) fn refresh_audio_event_modifiers(&mut self, platform: bool) -> bool {
        let Some(pointer) = self
            .audio_event_editor
            .gesture
            .as_ref()
            .map(|g| g.pointer_x)
        else {
            return false;
        };
        self.event_update_pointer(pointer, platform);
        true
    }
    fn event_zoom(&mut self, inward: bool) {
        let root = match self.event_context() {
            Ok(Some((_, root, _))) => root,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let Ok(Some(first)) = self.event_frame_at(root.view_start) else {
            return;
        };
        let Ok(Some(end)) = self.event_frame_at(root.view_end) else {
            return;
        };
        let Ok(Some(cursor)) = self.event_frame_at(self.audio_event_editor.cursor) else {
            return;
        };
        let Ok(Some(viewport)) = self.event_viewport(&root) else {
            return;
        };
        self.audio_event_editor.viewport = Some(viewport.zoom(
            end.saturating_sub(first),
            cursor.saturating_sub(first),
            inward,
        ));
        self.audio_event_editor.gesture = None;
    }
    fn event_pan(&mut self, right: bool) {
        let root = match self.event_context() {
            Ok(Some((_, root, _))) => root,
            Ok(None) => return,
            Err(error) => {
                self.event_error(error);
                return;
            }
        };
        let Ok(Some(first)) = self.event_frame_at(root.view_start) else {
            return;
        };
        let Ok(Some(end)) = self.event_frame_at(root.view_end) else {
            return;
        };
        let Ok(Some(viewport)) = self.event_viewport(&root) else {
            return;
        };
        self.audio_event_editor.viewport = Some(viewport.pan_half(end - first, right));
        self.audio_event_editor.gesture = None;
    }
}

fn fill_exact<T>(
    buffer: &mut Vec<T>,
    values: impl Iterator<Item = T>,
    count: usize,
) -> Result<(), Invalid> {
    if buffer.capacity() < count || !buffer.is_empty() {
        return Err(Invalid("audio event capture buffer is invalid"));
    }
    for value in values {
        if buffer.len() >= count {
            return Err(Invalid("audio event capture count changed"));
        }
        buffer.push(value);
    }
    if buffer.len() != count {
        return Err(Invalid("audio event capture count changed"));
    }
    Ok(())
}

fn event_paste_fits_view(root: &AudioClipRootFacts, at: u64, length: u64) -> bool {
    length > 0
        && at >= root.view_start
        && at
            .checked_add(length)
            .is_some_and(|end| end <= root.view_end)
}
