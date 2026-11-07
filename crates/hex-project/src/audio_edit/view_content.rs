//! Full directed-read geometry and exact frame crops. Keeping this immutable
//! partition lets control-side filtering retain its phase across section cuts.
use super::*;

impl AudioViewProjection<'_> {
    /// Full directed output and its exact integer-frame crop. Filtering the
    /// complete immutable map before cropping preserves interpolation phase at
    /// arbitrary section fences. Virtual pre-zero context is translated only.
    pub fn full_content_projection(&self) -> Result<(Self, std::ops::Range<u64>), Invalid> {
        let (window, current, displacement) =
            self.content.ok_or(Invalid("audio content view missing"))?;
        let length = u64::try_from(
            self.end
                .checked_sub(self.start)
                .ok_or(Invalid("audio base length overflow"))?,
        )
        .map_err(|_| Invalid("audio base length overflow"))?;
        let left = if window.reversed {
            length
                .checked_sub(window.end)
                .ok_or(Invalid("audio content exceeds base"))?
        } else {
            window.start
        };
        let reference = current
            .checked_sub(i128::from(left))
            .ok_or(Invalid("audio full reference overflow"))?;
        let frame = |position| -> Result<i128, Invalid> {
            let frame = if self.beat_clock {
                audio_projection_frame(self.tempo, position)?
            } else {
                position
            };
            frame
                .checked_add(displacement)
                .ok_or(Invalid("audio full displacement overflow"))
        };
        let first = frame(reference)?;
        let last = frame(
            reference
                .checked_add(i128::from(length))
                .ok_or(Invalid("audio full end overflow"))?,
        )?;
        let size = u64::try_from(
            last.checked_sub(first)
                .ok_or(Invalid("audio full span overflow"))?,
        )
        .map_err(|_| Invalid("audio full span invalid"))?;
        if size == 0 || size > MAX_AUDIO_DOMAIN_FRAMES {
            return Err(Invalid("audio full directed output exceeds frame limit"));
        }
        let shift = first
            .min(0)
            .checked_neg()
            .ok_or(Invalid("audio virtual displacement overflow"))?;
        let shifted = |value: i128| -> Result<u64, Invalid> {
            u64::try_from(
                value
                    .checked_add(shift)
                    .ok_or(Invalid("audio full boundary overflow"))?,
            )
            .map_err(|_| Invalid("audio full boundary exceeds frame range"))
        };
        let crop_start = u64::try_from(
            i128::from(self.placed_frames.0)
                .checked_sub(first)
                .ok_or(Invalid("audio crop offset overflow"))?,
        )
        .map_err(|_| Invalid("audio crop precedes full output"))?;
        let crop_end = u64::try_from(
            i128::from(self.placed_frames.1)
                .checked_sub(first)
                .ok_or(Invalid("audio crop offset overflow"))?,
        )
        .map_err(|_| Invalid("audio crop end invalid"))?;
        if crop_start > crop_end || crop_end > size {
            return Err(Invalid("audio crop exceeds full directed output"));
        }
        let mut full = *self;
        full.content = Some((
            crate::ContentWindow::new(0, length, window.reversed)?,
            reference,
            displacement
                .checked_add(shift)
                .ok_or(Invalid("audio full displacement overflow"))?,
        ));
        full.placed_frames = (shifted(first)?, shifted(last)?);
        full.displacement = full
            .displacement
            .checked_add(shift)
            .ok_or(Invalid("audio base displacement overflow"))?;
        full.outer_frames = (
            shifted(i128::from(full.outer_frames.0))?,
            shifted(i128::from(full.outer_frames.1))?,
        );
        full.first_boundary = full
            .first_boundary
            .checked_add(shift)
            .ok_or(Invalid("audio boundary overflow"))?;
        Ok((full, crop_start..crop_end))
    }
}
