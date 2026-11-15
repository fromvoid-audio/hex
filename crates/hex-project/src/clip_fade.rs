use crate::Invalid;

/// Baseline used before the existing horizontal and vertical curve controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FadeCurveLaw {
    Linear,
    SmoothStep,
}

/// Bounded discrete-coordinate fade geometry and gain law, independent of source
/// looping. Arrangement gains use sample frames. AudioEventFade uses this shape
/// to validate source-clock distances, then projects to sample frames before gain.
/// Splits retain this domain; explicit trims reanchor it. Each ramp has an
/// independent vertical rational bend and horizontal position. The horizontal
/// value moves a C1 monotone time mapping through half progress; the rational
/// bend then determines its ordinate. Centered positions bypass the mapping to
/// preserve the original sample-exact law. Overlapping ramps still multiply.
/// All gain work is bounded, allocation-free scalar arithmetic; inversion is
/// drawing/control-only. No proprietary reference algorithm is implied.
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
    in_position: u16,
    out_position: u16,
    laws: [FadeCurveLaw; 2],
}

impl ClipFade {
    /// Symmetric cubic S curve. Mirrored ramps over the same span sum to unity.
    pub const DEFAULT_S_CURVE: FadeCurveLaw = FadeCurveLaw::SmoothStep;

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
            in_position: 500,
            out_position: 500,
            laws: [FadeCurveLaw::Linear; 2],
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
    /// Horizontal half-progress positions in thousandths of each ramp's forward
    /// time, before the rational bend. Strict interior bounds keep both endpoints
    /// defined at extreme positions; renderers invert the law for bounded handles.
    /// Centered positions preserve the original rational fade bit for bit.
    pub fn with_curve_positions(mut self, fade_in: u16, fade_out: u16) -> Result<Self, Invalid> {
        if !(1..=999).contains(&fade_in) || !(1..=999).contains(&fade_out) {
            return Err(Invalid("fade curve position must be within 1..999"));
        }
        self.in_position = fade_in;
        self.out_position = fade_out;
        Ok(self)
    }
    pub fn in_position(self) -> u16 {
        self.in_position
    }
    pub fn out_position(self) -> u16 {
        self.out_position
    }

    /// Laws in fade-in, fade-out order. The baseline preserves half progress,
    /// so the existing bend and position retain their control meaning.
    pub fn curve_laws(self) -> [FadeCurveLaw; 2] {
        self.laws
    }

    #[must_use]
    pub fn with_curve_laws(self, laws: [FadeCurveLaw; 2]) -> Self {
        Self { laws, ..self }
    }

    /// Activate default S curves only on newly drawn ramps. Length changes
    /// to already-authored ramps keep the caller's existing coordinates.
    #[must_use]
    pub fn with_new_ramp_defaults(mut self, previous: Self) -> Self {
        for (out, before, after) in [
            (false, previous.fade_in, self.fade_in),
            (true, previous.fade_out, self.fade_out),
        ] {
            if before == 0 && after > 0 {
                self = self.straighten_ramp(out);
                self.laws[usize::from(out)] = Self::DEFAULT_S_CURVE;
            }
        }
        self
    }

    /// Straighten one ramp without changing its lengths or silent endpoints.
    #[must_use]
    pub fn straighten_ramp(mut self, out: bool) -> Self {
        self.laws[usize::from(out)] = FadeCurveLaw::Linear;
        if out {
            self.out_curve = 0;
            self.out_position = 500;
        } else {
            self.in_curve = 0;
            self.in_position = 500;
        }
        self
    }

    pub fn ramp_is_curved(self, out: bool) -> bool {
        self.laws[usize::from(out)] != FadeCurveLaw::Linear
            || if out {
                self.out_curve != 0 || self.out_position != 500
            } else {
                self.in_curve != 0 || self.in_position != 500
            }
    }

    /// Gain at mapped half progress of the selected individual ramp. This
    /// ordinate is independent of horizontal position and the opposite ramp.
    pub fn curve_midpoint_gain(self, out: bool) -> f64 {
        bend(0.5, if out { self.out_curve } else { self.in_curve })
    }

    /// Drawing-only inverse; playback evaluates the bounded polynomial directly.
    /// `out` selects the fade-out's forward-time coordinate (gain decreases).
    pub fn position_at_gain(self, out: bool, gain: f64) -> Option<f64> {
        if !gain.is_finite() || !(0.0..=1.0).contains(&gain) {
            return None;
        }
        let (curve, position, target) = if out {
            (self.out_curve, 1000 - self.out_position, gain)
        } else {
            (self.in_curve, self.in_position, gain)
        };
        // Invert the rational ordinate analytically. Centered legacy curves
        // require no iterative work, even when sampled for the waveform.
        let c = f64::from(curve);
        let target = target * (1001.0 + c) / (1001.0 - c + 2.0 * c * target);
        let target = match self.laws[usize::from(out)] {
            FadeCurveLaw::Linear => target,
            FadeCurveLaw::SmoothStep => inverse_smoothstep(target),
        };
        if position == 500 {
            return Some(if out { 1.0 - target } else { target });
        }
        let p = f64::from(position) / 1000.0;
        let direct = if (target - 0.5).abs() < 1e-14 {
            Some(p)
        } else if target < 0.5 && position < 500 {
            Some(2.0 * target * p)
        } else if target > 0.5 && position > 500 {
            Some(1.0 - 2.0 * (1.0 - target) * (1.0 - p))
        } else {
            None
        };
        if let Some(t) = direct {
            return Some(if out { 1.0 - t } else { t });
        }
        let (width, progress, reflected) = if target < 0.5 {
            (position, 2.0 * target, false)
        } else {
            (1000 - position, 2.0 * (1.0 - target), true)
        };
        let other = 1000 - width;
        if width % other == 0 {
            // Exact integer powers have a direct inverse. Transcendentals are
            // confined to this control/drawing query, never the gain path.
            let u = progress.powf(1.0 / f64::from(width / other));
            let half = u * f64::from(width) / 1000.0;
            let t = if reflected { 1.0 - half } else { half };
            return Some(if out { 1.0 - t } else { t });
        }
        // Only the longer blended polynomial half needs an inverse search.
        let (mut low, mut high) = if target < 0.5 { (0.0, p) } else { (p, 1.0) };
        for _ in 0..40 {
            let mid = (low + high) * 0.5;
            if map_time(mid, position) < target {
                low = mid;
            } else {
                high = mid;
            }
        }
        let t = (low + high) * 0.5;
        Some(if out { 1.0 - t } else { t })
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
        self.in_start != 0
            || self.out_end_offset != 0
            || self.ramp_is_curved(false)
            || self.ramp_is_curved(true)
    }
    pub fn is_neutral(self) -> bool {
        self.fade_in == 0 && self.fade_out == 0 && self.in_start == 0 && self.out_end_offset == 0
    }
    /// Explicit trim/edit reanchoring, clamping endpoints and retaining both shape coordinates.
    pub fn reanchor(self, duration: u64) -> Result<Self, Invalid> {
        let in_start = self.in_start.min(duration.saturating_sub(1));
        let out_end = self.out_end_offset.min(duration.saturating_sub(1));
        Ok(Self::new(
            duration,
            0,
            self.fade_in.min(duration - in_start),
            self.fade_out.min(duration - out_end),
        )?
        .with_shape(in_start, out_end, self.in_curve, self.out_curve)?
        .with_curve_positions(self.in_position, self.out_position)?
        .with_curve_laws(self.laws))
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
            gain *= shaped(
                in_age as f64 / (self.fade_in - 1) as f64,
                self.in_curve,
                self.in_position,
                self.laws[0],
            );
        }
        if self.fade_out > 0 && position >= out_end - self.fade_out {
            gain *= if self.fade_out == 1 {
                0.0
            } else {
                shaped(
                    (out_end - 1 - position) as f64 / (self.fade_out - 1) as f64,
                    self.out_curve,
                    1000 - self.out_position,
                    self.laws[1],
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

// Monotone C1 time mapping through (position, 0.5). The shorter side is
// linear; the longer side blends neighboring integer powers. Both meet with
// slope 0.5 / min(p, 1-p), concentrating the transition near an edge rather
// than stretching a shallow tail across the remaining fade. Center bypasses
// retain the original law exactly. No transcendental playback operations.
fn shaped(t: f64, curve: i16, position: u16, law: FadeCurveLaw) -> f64 {
    let t = if position == 500 {
        t
    } else {
        map_time(t, position)
    };
    bend(
        match law {
            FadeCurveLaw::Linear => t,
            FadeCurveLaw::SmoothStep => t * t * (3.0 - 2.0 * t),
        },
        curve,
    )
}

fn inverse_smoothstep(gain: f64) -> f64 {
    // Analytic inverse, used only on the drawing/control side. Preserve the
    // endpoints and the handle's half-progress knot exactly.
    if gain == 0.0 || gain == 0.5 || gain == 1.0 {
        return gain;
    }
    0.5 - ((1.0 - 2.0 * gain).asin() / 3.0).sin()
}
// Both callers bypass this mapping for centered positions.
fn map_time(t: f64, position: u16) -> f64 {
    let p = f64::from(position) / 1000.0;
    let (u, width, reflected) = if t <= p {
        (t / p, position, false)
    } else {
        ((1.0 - t) / (1.0 - p), 1000 - position, true)
    };
    let progress = if width <= 500 {
        u
    } else {
        let other = 1000 - width;
        let power = width / other;
        let blend = f64::from(width % other) / f64::from(other);
        unit_power(u, power) * (1.0 - blend + blend * u)
    };
    let half = 0.5 * progress;
    (if reflected { 1.0 - half } else { half }).clamp(0.0, 1.0)
}
// Validated positions bound the exponent to 1..=999: at most ten squarings.
fn unit_power(mut value: f64, mut exponent: u16) -> f64 {
    let mut result = 1.0;
    for _ in 0..10 {
        if exponent == 0 {
            break;
        }
        if exponent & 1 != 0 {
            result *= value;
        }
        exponent >>= 1;
        if exponent != 0 {
            value *= value;
        }
    }
    result
}
