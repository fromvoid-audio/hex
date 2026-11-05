//! Scalar base-placement geometry and a directed finite window. The domain,
//! processing clock, warp, events and envelopes remain owned by AudioClipEdit.
use super::*;
use crate::ContentWindow;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioContentView {
    pub restore_plain: bool,
    pub base_view_start: u64,
    pub base_view_end: u64,
    pub base_placement: Option<AudioViewPlacement>,
    /// base reference = current reference + delta, in the current clip clock.
    pub base_start_delta: i128,
    pub window: ContentWindow,
    /// Law that reconstructs fractional base reads. Only musical-clock views
    /// read fractional root positions; frame-clock views copy integer samples,
    /// which every law reproduces exactly. New views take the current default.
    pub read_resampler: WarpResampler,
}
impl AudioContentView {
    /// Law implied when a saved view does not name one. Musical-clock views
    /// were rendered by the frozen v1 law before views were versioned and keep
    /// it; frame-clock views read integer samples and take the current default.
    pub fn implied_read_resampler(clock: AudioDomainClock) -> WarpResampler {
        match clock {
            AudioDomainClock::Beats(_) => WarpResampler::V1,
            AudioDomainClock::Frames(_) => WarpResampler::default(),
        }
    }
    /// Whether this view's law differs from the one its clip clock implies.
    pub fn has_explicit_read_resampler(self, clock: AudioDomainClock) -> bool {
        self.read_resampler != Self::implied_read_resampler(clock)
    }
    pub fn base_length(self) -> Result<u64, Invalid> {
        self.base_placement.map_or_else(
            || {
                self.base_view_end
                    .checked_sub(self.base_view_start)
                    .filter(|n| *n > 0)
                    .ok_or(Invalid("audio base crop must be positive"))
            },
            |p| Ok(p.outer_length.length()),
        )
    }
    pub fn validate(self, edit: &AudioClipEdit) -> Result<(), Invalid> {
        let length = self.window.length()?;
        let audible = edit
            .placement
            .map_or(edit.view_end.checked_sub(edit.view_start), |p| {
                Some(p.outer_length.length())
            });
        if audible != Some(length) {
            return Err(Invalid("audio directed read length differs from placement"));
        }
        if self.base_view_start >= self.base_view_end
            || self.base_view_end > edit.domain.clock.length()
            || self.window.end > self.base_length()?
            || length == 0
        {
            return Err(Invalid(
                "audio directed window exceeds its retained placement",
            ));
        }
        if let Some(p) = self.base_placement
            && (matches!(p.outer_length, AudioDomainClock::Beats(_))
                != matches!(edit.domain.clock, AudioDomainClock::Beats(_))
                || p.outer_length.length() == 0
                || matches!(p.mode, AudioViewMode::OnceWithLead(_))
                || (p.mode == AudioViewMode::Repeat
                    && p.cycle_phase >= self.base_view_end - self.base_view_start))
        {
            return Err(Invalid("audio directed base placement is invalid"));
        }
        Ok(())
    }
}

pub(super) fn trim_content(
    clip: &Clip,
    edge: TrimEdge,
    frame: u64,
    tempo: &TempoMap,
) -> Result<Clip, Invalid> {
    let old = clip
        .audio_edit()
        .ok_or(Invalid("audio content domain missing"))?;
    let mut content = old
        .content_view
        .ok_or(Invalid("audio content view missing"))?;
    let old_start = old.reference_start_raw().unwrap_or(i128::from(clip.start));
    let position = if old.reference_start_raw().is_some() {
        old.reference_beat_at_destination(clip, frame, tempo)?
    } else {
        i128::from(frame)
    };
    // Preserve a separately valid ordinary placement carrier, including the
    // existing retained-processing residual law. It owns extent/edit controls;
    // the independent base window owns actual content reads.
    let mut carrier = clip.clone();
    carrier
        .audio_edit_mut()
        .ok_or(Invalid("audio content domain missing"))?
        .content_view = None;
    let mut next = super::commands::trim_view(&carrier, edge, frame, tempo)?;
    // Size the read window from the carrier's audible extent. Retained
    // processing crops in its frozen clock, which may round one raw beat
    // apart from the destination reference position.
    let length = {
        let edit = next
            .audio_edit()
            .ok_or(Invalid("audio content domain missing"))?;
        edit.placement
            .map_or(edit.view_end - edit.view_start, |p| p.outer_length.length())
    };
    if length == 0 {
        return Err(Invalid("audio trim must retain a positive interval"));
    }
    // Reference displacement keeps the base anchored; the window shift is in
    // the same read clock as the carrier crop.
    let (delta, shift) = match edge {
        TrimEdge::Left => (
            position - old_start,
            i128::from(content.window.length()?) - i128::from(length),
        ),
        TrimEdge::Right => (0, 0),
    };
    let resized = content.window.resized(shift, length);
    if let Ok(window) = resized {
        if window.end <= content.base_length()? {
            content.window = window;
        } else if content.base_placement.is_none() {
            let extra = window.end - content.base_length()?;
            content.base_view_end = content
                .base_view_end
                .checked_add(extra)
                .filter(|n| *n <= old.domain.clock.length())
                .ok_or(Invalid("audio extension exceeds retained processing root"))?;
            content.window = window;
        } else {
            return Err(Invalid(
                "audio extension exceeds retained directed outer placement; resize its loop first",
            ));
        }
    } else if content.base_placement.is_none() {
        // A reversed right extension may reveal retained source before the
        // captured crop. Rebase the finite law without moving its root clock.
        let read_end = if content.window.reversed {
            i128::from(content.window.end) - shift
        } else {
            i128::from(content.window.start) + shift + i128::from(length)
        };
        let read_start = read_end - i128::from(length);
        let rewind = u64::try_from(
            read_start
                .checked_neg()
                .ok_or(Invalid("audio extension coordinate overflow"))?,
        )
        .map_err(|_| Invalid("audio extension coordinate overflow"))?;
        content.base_view_start = content
            .base_view_start
            .checked_sub(rewind)
            .ok_or(Invalid("audio extension precedes retained processing root"))?;
        content.base_start_delta = content
            .base_start_delta
            .checked_sub(i128::from(rewind))
            .ok_or(Invalid("audio base displacement overflow"))?;
        content.window = crate::ContentWindow::new(0, length, content.window.reversed)?;
    } else {
        return Err(Invalid(
            "audio extension precedes retained directed outer placement; resize its loop first",
        ));
    }
    content.base_start_delta = content
        .base_start_delta
        .checked_sub(delta)
        .ok_or(Invalid("audio base displacement overflow"))?;
    next.audio_edit_mut()
        .ok_or(Invalid("audio content domain missing"))?
        .content_view = Some(content);
    super::commands::project_clip(&mut next, tempo)?;
    Ok(next)
}

impl AudioClipEdit {
    /// An explicit cycle/outer edit authors a new base law while retaining its
    /// reading direction. Ordinary trims use trim_content and never recapture.
    pub(super) fn rebase_content(&mut self) -> Result<(), Invalid> {
        if let Some(old) = self.content_view {
            let length = self
                .placement
                .map_or(self.view_end - self.view_start, |p| p.outer_length.length());
            self.content_view = Some(AudioContentView {
                restore_plain: false,
                base_view_start: self.view_start,
                base_view_end: self.view_end,
                base_placement: self.placement,
                base_start_delta: 0,
                window: crate::ContentWindow::new(0, length, old.window.reversed)?,
                read_resampler: old.read_resampler,
            });
        }
        Ok(())
    }
}

impl AudioClipEdit {
    pub(super) fn sync_content_length(&mut self) -> Result<(), Invalid> {
        if let Some(view) = self.content_view {
            let length = view.window.length()?;
            let clock = match self.domain.clock {
                AudioDomainClock::Frames(_) => AudioDomainClock::Frames(length),
                AudioDomainClock::Beats(_) => AudioDomainClock::Beats(BeatTime::from_raw(length)),
            };
            if let Some(p) = &mut self.placement {
                p.outer_length = clock;
            } else if self.view_end - self.view_start != length {
                self.placement = Some(AudioViewPlacement {
                    mode: AudioViewMode::Once,
                    outer_length: clock,
                    cycle_phase: 0,
                });
            }
        }
        Ok(())
    }
}
