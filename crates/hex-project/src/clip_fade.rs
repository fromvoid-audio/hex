use crate::Invalid;

/// Sample-coordinate fade domain, independent of source looping.
/// Splits retain this domain; explicit trims reanchor it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipFade {
    span: u64,
    offset: u64,
    fade_in: u64,
    fade_out: u64,
    in_start: u64,
    out_end_offset: u64,
    in_curve: i16,
    out_curve: i16,
}

impl ClipFade {
    pub fn new(span: u64, offset: u64, fade_in: u64, fade_out: u64) -> Result<Self, Invalid> {
        if span == 0 {
            return Err(Invalid("fade span must be positive"));
        }
        let end = offset
            .checked_add(1)
            .ok_or(Invalid("fade offset overflow"))?;
        if end > span {
            return Err(Invalid("fade offset must be inside its span"));
        }
        if fade_in > span || fade_out > span {
            return Err(Invalid("fade lengths must not exceed their span"));
        }
        Ok(Self {
            span,
            offset,
            fade_in,
            fade_out,
            in_start: 0,
            out_end_offset: 0,
            in_curve: 0,
            out_curve: 0,
        })
    }

    pub fn neutral(span: u64) -> Result<Self, Invalid> {
        Self::new(span, 0, 0, 0)
    }

    pub fn span(self) -> u64 {
        self.span
    }
    pub fn offset(self) -> u64 {
        self.offset
    }
    pub fn fade_in(self) -> u64 {
        self.fade_in
    }
    pub fn fade_out(self) -> u64 {
        self.fade_out
    }

    pub fn validate_segment(self, duration: u64) -> Result<(), Invalid> {
        if duration == 0 {
            return Err(Invalid("clip fade segment exceeds its domain"));
        }
        let end = self
            .offset
            .checked_add(duration)
            .ok_or(Invalid("clip fade segment exceeds its domain"))?;
        if end > self.span {
            return Err(Invalid("clip fade segment exceeds its domain"));
        }
        Ok(())
    }

    pub fn with_offset(self, offset: u64, duration: u64) -> Result<Self, Invalid> {
        let fade = Self { offset, ..self };
        Self::new(self.span, offset, self.fade_in, self.fade_out)?;
        fade.validate_segment(duration)?;
        Ok(fade)
    }

    /// Add independent silent endpoints and rational curve bends (-1000..=1000).
    /// Zero bends retain the original sample-exact linear law.
    pub fn with_shape(
        mut self,
        in_start: u64,
        out_end_offset: u64,
        in_curve: i16,
        out_curve: i16,
    ) -> Result<Self, Invalid> {
        if in_start >= self.span
            || out_end_offset >= self.span
            || self.fade_in > self.span - in_start
            || self.fade_out > self.span - out_end_offset
        {
            return Err(Invalid("fade endpoints exceed their domain"));
        }
        if !(-1000..=1000).contains(&in_curve) || !(-1000..=1000).contains(&out_curve) {
            return Err(Invalid("fade curve must be within -1000..1000"));
        }
        self.in_start = in_start;
        self.out_end_offset = out_end_offset;
        self.in_curve = in_curve;
        self.out_curve = out_curve;
        Ok(self)
    }
    pub fn in_start(self) -> u64 {
        self.in_start
    }
    pub fn out_end_offset(self) -> u64 {
        self.out_end_offset
    }
    pub fn in_curve(self) -> i16 {
        self.in_curve
    }
    pub fn out_curve(self) -> i16 {
        self.out_curve
    }
    pub fn has_shape(self) -> bool {
        self.in_start != 0 || self.out_end_offset != 0 || self.in_curve != 0 || self.out_curve != 0
    }
    pub fn is_neutral(self) -> bool {
        self.fade_in == 0 && self.fade_out == 0 && self.in_start == 0 && self.out_end_offset == 0
    }
    /// Explicit trim/edit reanchoring, clamping endpoints and retaining curve bends.
    pub fn reanchor(self, duration: u64) -> Result<Self, Invalid> {
        let in_start = self.in_start.min(duration.saturating_sub(1));
        let out_end = self.out_end_offset.min(duration.saturating_sub(1));
        Self::new(
            duration,
            0,
            self.fade_in.min(duration - in_start),
            self.fade_out.min(duration - out_end),
        )?
        .with_shape(in_start, out_end, self.in_curve, self.out_curve)
    }
    /// Domain knots, including both sides of silent endpoint steps.
    pub fn breakpoints(self) -> [Option<u64>; 8] {
        let out_end = self.span - self.out_end_offset;
        let out_start = out_end - self.fade_out;
        [
            self.in_start.checked_sub(1),
            Some(self.in_start),
            self.in_start.checked_add(self.fade_in.saturating_sub(1)),
            out_start.checked_sub(1),
            Some(out_start),
            out_end.checked_sub(1),
            (out_end < self.span).then_some(out_end),
            self.span.checked_sub(1),
        ]
    }
    /// Allocation-free, coordinate-based gain; overlapping fades multiply.
    pub fn gain_at(self, frame: u64) -> f32 {
        let Some(position) = self.offset.checked_add(frame) else {
            return 0.0;
        };
        let out_end = self.span - self.out_end_offset;
        if position >= out_end || position < self.in_start {
            return 0.0;
        }
        let mut gain = 1.0_f64;
        let in_age = position - self.in_start;
        if self.fade_in > 1 && in_age < self.fade_in {
            gain *= bend(in_age as f64 / (self.fade_in - 1) as f64, self.in_curve);
        }
        if self.fade_out > 0 && position >= out_end - self.fade_out {
            gain *= if self.fade_out == 1 {
                0.0
            } else {
                bend(
                    (out_end - 1 - position) as f64 / (self.fade_out - 1) as f64,
                    self.out_curve,
                )
            };
        }
        gain as f32
    }
}
fn bend(t: f64, curve: i16) -> f64 {
    if curve == 0 {
        return t;
    }
    let c = f64::from(curve);
    t * (1001.0 - c) / (1001.0 + c - 2.0 * c * t)
}
