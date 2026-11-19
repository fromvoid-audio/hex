//! Bounded coarse range queries over the same eligible frame-source law.
use super::*;
use hex_project::AudioViewMode;

impl FrameFadePreview<'_> {
    /// Visit source unions for a wide Arrangement column. `remaining` is shared
    /// by the caller's entire geometry, not reset per column. One unit covers
    /// each emitted source interval or visited silence context; intersections
    /// use binary search, and cycles collapse into at most two root intervals.
    /// Preflight refuses with `None` and sets the budget to zero before calling
    /// `visit` at all. The caller must discard an unavailable geometry, not paint
    /// partial data. Success reports whether genuine zero joins its extrema.
    pub fn visit_timeline_ranges(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if range.start > range.end {
            return Err(Invalid("fade preview range is reversed"));
        }
        if range.is_empty() {
            return Ok(Some(FrameFadeCoverage::default()));
        }
        let clip_end = self.clip.start() + self.clip.duration();
        let mut first = range.start.max(self.clip.start());
        let mut last = range.end.min(clip_end);
        if let Some(fade) = &self.directed_legacy {
            first = first.max(fade.destination.start);
            last = last.min(fade.destination.end);
        }
        let mut silence = first != range.start || last != range.end;
        let roots = if first >= last {
            [None, None]
        } else if let Some(projection) = &self.projection {
            let roots = projection
                .root_ranges_for_frames(first..last)
                .map_err(|error| Invalid(error.0))?;
            if let Some(view) = self.clip.audio_edit().and_then(|edit| edit.content_view) {
                // Base occurrence bounds are in read order. In a finite Once
                // window the directed root union counts exactly the audible
                // frames. Repeat has no placement holes and can reuse frames.
                if view
                    .base_placement
                    .is_none_or(|p| p.mode != AudioViewMode::Repeat)
                {
                    let audible = roots.iter().flatten().map(|r| r.end - r.start).sum::<u64>();
                    silence |= audible < last - first;
                }
            } else if projection.has_silent_repeat_gap() {
                let edit = self
                    .clip
                    .audio_edit()
                    .ok_or(Invalid("fade preview root is missing"))?;
                let placement = edit
                    .placement
                    .ok_or(Invalid("fade preview placement is missing"))?;
                let period = edit
                    .repeat_period
                    .ok_or(Invalid("fade preview period is missing"))?;
                let audible = edit.view_end - edit.view_start;
                let phase = ((u128::from(placement.cycle_phase)
                    + u128::from(first - self.clip.start()))
                    % u128::from(period)) as u64;
                silence |= phase >= audible || last - first > audible - phase;
            } else if projection.occurrence_count() == 0 {
                silence = true;
            } else if projection.occurrence_count() == 1 {
                let occurrence = projection.occurrence(0).map_err(|error| Invalid(error.0))?;
                silence |=
                    first < occurrence.destination_start || last > occurrence.destination_end;
            }
            roots
        } else {
            [
                Some(first - self.clip.start()..last - self.clip.start()),
                None,
            ]
        };
        // Root unions can collapse several occurrences. The instance envelope
        // belongs to the column's destination midpoint, never any root phase.
        let gain = if first < last {
            self.timeline_gain(first + (last - first - 1) / 2)?
        } else {
            None
        };
        let mut visit = visit;
        self.visit_admitted(roots, silence, remaining, |mut part| {
            if let Some(gain) = gain {
                part.sample.gains[2] = gain;
            }
            visit(part);
        })
    }

    /// Root-destination frames before outer placement, for the event editor.
    /// Uses the same shared-budget and all-or-unavailable contract as above.
    pub fn visit_root_ranges(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if range.start > range.end {
            return Err(Invalid("fade preview range is reversed"));
        }
        if range.is_empty() {
            return Ok(Some(FrameFadeCoverage::default()));
        }
        let length = self
            .clip
            .audio_edit()
            .map_or(self.clip.duration(), |edit| edit.domain.clock.length());
        let first = range.start.min(length);
        let last = range.end.min(length);
        self.visit_admitted(
            [(first < last).then_some(first..last), None],
            range.end > length,
            remaining,
            visit,
        )
    }
    fn visit_admitted(
        &self,
        roots: [Option<Range<u64>>; 2],
        initial_silence: bool,
        remaining: &mut usize,
        mut visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if *remaining == 0 && roots.iter().any(Option::is_some) {
            return Ok(None);
        }
        let mut used = 0usize;
        let (complete, silence) = self.walk_ranges(&roots, |part| {
            let cost = part
                .as_ref()
                .map_or(1, |part| part.source.iter().flatten().count());
            if cost > remaining.saturating_sub(used) {
                return false;
            }
            used += cost;
            true
        })?;
        if !complete {
            *remaining = 0;
            return Ok(None);
        }
        *remaining -= used;
        self.walk_ranges(&roots, |part| {
            if let Some(part) = part {
                visit(part);
            }
            true
        })?;
        Ok(Some(FrameFadeCoverage {
            has_silence: initial_silence || silence,
        }))
    }
    fn walk_ranges(
        &self,
        roots: &[Option<Range<u64>>; 2],
        mut visit: impl FnMut(Option<FrameFadeRange>) -> bool,
    ) -> Result<(bool, bool), Invalid> {
        let mut silence = false;
        for root in roots.iter().flatten() {
            if let Some(events) = self.events {
                let first = events
                    .events
                    .partition_point(|event| event.end <= root.start);
                let mut cursor = root.start;
                for event in &events.events[first..] {
                    if event.start >= root.end {
                        break;
                    }
                    let start = event.start.max(root.start);
                    let end = event.end.min(root.end);
                    if cursor < start {
                        silence = true;
                        if !visit(None) {
                            return Ok((false, silence));
                        }
                    }
                    let part = if event.active {
                        let (part, missing) = self.source_part(start..end, Some(event))?;
                        silence |= missing;
                        part
                    } else {
                        silence = true;
                        None
                    };
                    if !visit(part) {
                        return Ok((false, silence));
                    }
                    cursor = end;
                }
                if cursor < root.end {
                    silence = true;
                    if !visit(None) {
                        return Ok((false, silence));
                    }
                }
            } else {
                let (part, missing) = self.source_part(root.clone(), None)?;
                silence |= missing;
                if !visit(part) {
                    return Ok((false, silence));
                }
            }
        }
        Ok((true, silence))
    }
    fn source_part(
        &self,
        mut root: Range<u64>,
        event: Option<&AudioEvent>,
    ) -> Result<(Option<FrameFadeRange>, bool), Invalid> {
        let read = match event {
            Some(event) => event
                .source_start
                .checked_add(root.start - event.start)
                .ok_or(Invalid("fade preview source frame overflow"))?,
            None => root.start,
        };
        let mut length = root.end - root.start;
        let mut silence = false;
        if !self.source.looping {
            let available = self.source.length.saturating_sub(read);
            silence = length > available;
            length = length.min(available);
            if length == 0 {
                return Ok((None, true));
            }
            root.end = root.start + length;
        }
        let offset = self.source.offset;
        let period = self.source.length;
        let source = if self.source.looping && length >= period {
            [Some(offset..offset + period), None]
        } else {
            let phase = if self.source.looping {
                ((u128::from(self.source.phase) + u128::from(read)) % u128::from(period)) as u64
            } else {
                read
            };
            let first = length.min(period - phase);
            [
                Some(offset + phase..offset + phase + first),
                (first < length).then_some(offset..offset + length - first),
            ]
        };
        let midpoint = root.start + (length - 1) / 2;
        let sample = self
            .event_root_sample(midpoint, event)?
            .ok_or(Invalid("fade preview midpoint is outside source"))?;
        Ok((
            Some(FrameFadeRange {
                root,
                source,
                sample,
            }),
            silence,
        ))
    }
}

impl FrameFadeEvent<'_, '_> {
    /// Wide event-editor column in the same retained source-root clock as
    /// `sample()`. A source slip is translated by the owner, not inferred by UI.
    pub fn visit_ranges(
        &self,
        range: Range<u64>,
        remaining: &mut usize,
        visit: impl FnMut(FrameFadeRange),
    ) -> Result<Option<FrameFadeCoverage>, Invalid> {
        if range.start > range.end {
            return Err(Invalid("fade preview range is reversed"));
        }
        if range.is_empty() {
            return Ok(Some(FrameFadeCoverage::default()));
        }
        let Some(event) = self.event else {
            return self.preview.visit_root_ranges(range, remaining, visit);
        };
        let source_end = event.source_start + event.length();
        let first = range.start.max(event.source_start);
        let last = range.end.min(source_end);
        let silence = first != range.start || last != range.end;
        let root = if first < last {
            Some(
                event.start + (first - event.source_start)
                    ..event.start + (last - event.source_start),
            )
        } else {
            None
        };
        self.preview
            .visit_admitted([root, None], silence, remaining, visit)
    }
}
