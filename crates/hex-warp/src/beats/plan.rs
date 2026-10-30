//! Private Copy-only logical boundaries. They do not become physical attacks.
use super::*;
#[derive(Clone, Copy)]
pub(super) struct Boundary {
    pub(super) source: f64,
    pub(super) output: usize,
}
#[derive(Clone, Copy)]
pub(super) struct Span {
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) source_start: f64,
    pub(super) source_end: f64,
    pub(super) locked_end: bool,
}
pub(super) enum Schedule<'a> {
    Physical(&'a [Anchor]),
    Chosen(Vec<Boundary>),
}
pub(super) fn scratch(request: &Request<'_>) -> usize {
    if request.attacks.is_empty()
        && matches!(
            request.mode,
            Mode::Beats {
                division_frames: None,
                ..
            }
        )
    {
        0
    } else {
        (MAX_MAP_SEGMENTS + 2) * std::mem::size_of::<Boundary>()
    }
}
impl<'a> Schedule<'a> {
    pub(super) fn prepare(
        request: &Request<'_>,
        physical: &'a [Anchor],
        work: &mut Work<'_>,
    ) -> Result<Self, Error> {
        let Mode::Beats {
            division_frames, ..
        } = request.mode
        else {
            return Err(Error::Invalid("logical preserve mode mismatch"));
        };
        if request.attacks.is_empty() && division_frames.is_none() {
            return Ok(Self::Physical(physical));
        }
        let mut points = Vec::new();
        points
            .try_reserve_exact(MAX_MAP_SEGMENTS + 2)
            .map_err(|_| Error::Allocation)?;
        points.push(Boundary {
            source: request.map.segments()[0].source_start_q32 as f64 / Q32 as f64,
            output: 0,
        });
        let region = i128::from(request.input.region_length);
        let first = request.map.segments()[0]
            .source_start_q32
            .div_euclid(Q32)
            .div_euclid(region);
        let first = if request.input.looping {
            first
        } else {
            first.max(0)
        };
        let end = request
            .map
            .segments()
            .last()
            .expect("segments()[0] indexed above")
            .source_end_q32;
        let last = end.div_euclid(Q32).div_euclid(region);
        if last - first > MAX_MAP_SEGMENTS as i128 {
            return Err(Error::Invalid(
                "logical preserve cycle count exceeds bounded plan",
            ));
        }
        // Empty attacks with no division returned Physical above.
        let division = || division_frames.expect("division set when attacks are empty");
        let count = if request.attacks.is_empty() {
            ((request.input.region_length - 1) / division() + 1) as usize
        } else {
            request.attacks.len()
        };
        for cycle in first..=last {
            if cycle > 0 && !request.input.looping {
                break;
            }
            work.check(count as u64 * 16)?;
            for i in 0..count {
                let at = if request.attacks.is_empty() {
                    i as u64 * division()
                } else {
                    request.attacks[i]
                };
                let source = cycle * region + i128::from(at);
                if let Some(output) = request.map.output_for_source(source * Q32)
                    && output < request.output_frames as u64
                    && let Some(previous) = points.last()
                    && output >= previous.output as u64
                    && source as f64 > previous.source
                {
                    if points.len() == MAX_MAP_SEGMENTS {
                        return Err(Error::Invalid(
                            "logical preserve schedule exceeds bounded plan",
                        ));
                    }
                    points.push(Boundary {
                        source: source as f64,
                        output: output as usize,
                    });
                }
            }
        }
        points.push(Boundary {
            source: end as f64 / Q32 as f64,
            output: request.output_frames,
        });
        Ok(Self::Chosen(points))
    }
    pub(super) fn len(&self) -> usize {
        match self {
            Self::Physical(x) => x.len(),
            Self::Chosen(x) => x.len(),
        }
    }
    pub(super) fn point(&self, index: usize) -> Boundary {
        match self {
            Self::Physical(x) => Boundary {
                source: x[index].source,
                output: x[index].output,
            },
            Self::Chosen(x) => x[index],
        }
    }
    fn segment(&self, output: i128) -> (Boundary, Boundary) {
        let output = output.max(0) as usize;
        let mut lo = 0;
        let mut hi = self.len() - 1;
        while lo + 1 < hi {
            let mid = (lo + hi) / 2;
            if self.point(mid).output <= output {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        (self.point(lo), self.point(lo + 1))
    }
    /// Reflected safe tail after one native traversal. The direction is shared
    /// by stereo. Compression remains ordinary forward reconstruction.
    pub(super) fn reflected(&self, span: Span, output: i128, pitch: f64) -> Option<(f64, f64)> {
        let (a, b) = self.segment(output);
        let start = a.output.max(span.start);
        let end = b.output.min(span.end);
        let lower = a.source.max(span.source_start);
        let upper = b.source.min(span.source_end) - 1.0;
        let width = upper - lower;
        if width < 2.0 || (end.saturating_sub(start)) as f64 * pitch <= width + 1.0 {
            return None;
        }
        let position = lower + (output - start as i128) as f64 * pitch;
        if position <= upper {
            return Some((position, 1.0));
        }
        let phase = (position - upper).rem_euclid(width * 2.0);
        if phase < width {
            Some((upper - phase, -1.0))
        } else {
            Some((lower + phase - width, 1.0))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn reflected_schedule_finishes_native_pass_excludes_guard_and_has_no_compression_reverse() {
        let points = [
            Anchor {
                source: 0.0,
                output: 0,
                before: 0,
                after: 0,
                attack: false,
            },
            Anchor {
                source: 64.0,
                output: 256,
                before: 0,
                after: 0,
                attack: false,
            },
        ];
        let schedule = Schedule::Physical(&points);
        let span = Span {
            start: 0,
            end: 256,
            source_start: 0.0,
            source_end: 64.0,
            locked_end: false,
        };
        for at in 0..64 {
            assert_eq!(schedule.reflected(span, at, 1.0), Some((at as f64, 1.0)));
        }
        for (at, expected) in [
            (64, (62.0, -1.0)),
            (125, (1.0, -1.0)),
            (126, (0.0, 1.0)),
            (127, (1.0, 1.0)),
        ] {
            assert_eq!(schedule.reflected(span, at, 1.0), Some(expected));
        }
        let protected = Span {
            start: 8,
            source_start: 8.0,
            ..span
        };
        for at in 8..256 {
            assert!(schedule.reflected(protected, at, 1.0).unwrap().0 >= 8.0);
        }
        let compressed = Span { end: 32, ..span };
        for at in 0..32 {
            assert!(schedule.reflected(compressed, at, 1.0).is_none());
        }
    }
}
