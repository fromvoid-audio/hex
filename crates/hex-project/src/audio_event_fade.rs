//! Event-owned fade geometry in its retained source root's clock. A split or
//! clipboard crop retains the same source domain; moving a placement does not
//! move its envelope through the source. Explicit trim/edit reanchoring follows
//! the ordinary clip law. Absence is exact unity, without an implicit ramp.
use crate::{ClipFade, Invalid};

/// A typed source-clock envelope. `shape` uses root-clock distances (frames or
/// Q32.32 beats), never an assumed sample rate. Consumers must project it to
/// sample frames before evaluating gain. Storage retains all shape coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioEventFade {
    source_start: u64,
    shape: ClipFade,
}
impl AudioEventFade {
    pub fn new(source_start: u64, shape: ClipFade) -> Result<Self, Invalid> {
        if shape.offset() != 0 || source_start.checked_add(shape.span()).is_none() {
            return Err(Invalid("audio event fade domain is invalid"));
        }
        Ok(Self {
            source_start,
            shape,
        })
    }
    pub fn source_start(self) -> u64 {
        self.source_start
    }
    /// Geometry only; lengths have the enclosing audio root's explicit units.
    pub fn shape(self) -> ClipFade {
        self.shape
    }

    /// Straighten one ramp without changing the source window.
    #[must_use]
    pub fn straighten_ramp(self, out: bool) -> Self {
        Self {
            shape: self.shape.straighten_ramp(out),
            ..self
        }
    }
    pub fn validate(self, root_length: u64, source_start: u64, length: u64) -> Result<(), Invalid> {
        let end = self.source_start.checked_add(self.shape.span());
        let source_end = source_start.checked_add(length);
        if self.shape.offset() != 0
            || length == 0
            || source_start < self.source_start
            || end.is_none_or(|end| end > root_length || source_end.is_none_or(|s| s > end))
        {
            return Err(Invalid("audio event fade does not cover its source window"));
        }
        Ok(())
    }
    pub fn reanchor(self, source_start: u64, length: u64) -> Result<Self, Invalid> {
        Self::new(source_start, self.shape.reanchor(length)?)
    }
    /// Project complete source knots together, preserving bends and horizontal
    /// positions. Used for frame/beat authoring conversions and control-side
    /// compilation to sample frames. A collapsed/reversed domain refuses.
    pub fn reproject(
        self,
        mut position: impl FnMut(u64) -> Result<u64, Invalid>,
    ) -> Result<Self, Invalid> {
        let point = |relative: u64| {
            self.source_start
                .checked_add(relative)
                .ok_or(Invalid("audio event fade position overflow"))
        };
        let start = position(self.source_start)?;
        let end = position(point(self.shape.span())?)?;
        let in_start = position(point(self.shape.in_start())?)?;
        let in_end = position(point(self.shape.in_start() + self.shape.fade_in())?)?;
        let out_end = position(point(self.shape.span() - self.shape.out_end_offset())?)?;
        let out_start = position(point(
            self.shape.span() - self.shape.out_end_offset() - self.shape.fade_out(),
        )?)?;
        let difference = |a: u64, b: u64| {
            a.checked_sub(b)
                .ok_or(Invalid("audio event fade projection is reversed"))
        };
        Self::new(
            start,
            ClipFade::new(
                difference(end, start)?,
                0,
                difference(in_end, in_start)?,
                difference(out_end, out_start)?,
            )?
            .with_shape(
                difference(in_start, start)?,
                difference(end, out_end)?,
                self.shape.in_curve(),
                self.shape.out_curve(),
            )?
            .with_curve_positions(self.shape.in_position(), self.shape.out_position())?
            .with_curve_laws(self.shape.curve_laws()),
        )
    }
}

#[cfg(test)]
#[path = "audio_event_fade_tests.rs"]
mod tests;
