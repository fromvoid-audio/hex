//! Control-prepared Arrangement curves for a foreign PCM parameter queue.
//! The host supplies exact-source SDK facts and proves the origin-run limit
//! before admission. This owner neither loads a plug-in nor publishes a graph.
use crate::Invalid;
use hex_lattice_contract::{DeviceAutomationTarget, ForeignParameterPoint};
use hex_project::{AutomationInterpolation, AutomationPoint, DeviceAutomation, TempoMap};

const MAX_QUANTUM_FRAMES: u16 = 256;
const MAX_POINTS: usize = 64;

pub use hex_lattice::ForeignParameterCapability as ParameterCapability;

/// One physical processing quantum's admitted extent. A run ends at an SDK
/// slice cut, a missing origin or any origin that does not advance by one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct QuantumProfile {
    pub frames: u16,
    pub max_runs: u16,
}

#[derive(Debug)]
pub struct Program {
    lanes: Vec<Lane>,
    profile: QuantumProfile,
    point_budget: usize,
}

#[derive(Debug)]
struct Lane {
    id: u32,
    knots: Vec<Knot>,
}

#[derive(Clone, Copy, Debug)]
struct Knot {
    frame: u64,
    value: f64,
    linear: bool,
}

/// An engine-owned program paired to the graph's original worker epoch.
#[derive(Debug)]
pub struct BoundProgram {
    program: Program,
    binding: hex_lattice_contract::ForeignProcessorBinding,
    revision: std::num::NonZeroU64,
}

impl BoundProgram {
    /// The graph owner supplies its proven origin-run bound. Source facts
    /// supply the class, rate, quantum extent and admissible SDK parameter
    /// IDs. The graph owner must use a fresh revision when the program changes.
    pub fn prepare(
        lanes: &[DeviceAutomation],
        source: hex_lattice::ForeignAutomationSource<'_>,
        tempo: &TempoMap,
        max_runs: u16,
        revision: std::num::NonZeroU64,
    ) -> Result<Option<Self>, Invalid> {
        if !source.binding.is_valid() {
            return Err(Invalid("invalid foreign automation source binding"));
        }
        if source.sample_rate != tempo.sample_rate() {
            return Err(Invalid("foreign automation source and tempo rates differ"));
        }
        Ok(Program::prepare(
            lanes,
            source.class,
            source.parameters,
            tempo,
            QuantumProfile {
                frames: source.quantum_frames,
                max_runs,
            },
        )?
        .map(|program| Self {
            program,
            binding: source.binding,
            revision,
        }))
    }

    pub fn borrow(&self) -> hex_lattice::ForeignAutomation<'_> {
        hex_lattice::ForeignAutomation {
            binding: self.binding,
            revision: self.revision,
            program: &self.program,
        }
    }
}

impl Program {
    /// Compile the authored read selection only. A stepped SDK parameter uses
    /// quantized holds. The control owner accounts for the complete retained
    /// program before handing it to a callback.
    pub fn prepare(
        lanes: &[DeviceAutomation],
        class: [u8; 16],
        capabilities: &[ParameterCapability],
        tempo: &TempoMap,
        profile: QuantumProfile,
    ) -> Result<Option<Self>, Invalid> {
        if class == [0; 16]
            || profile.frames == 0
            || profile.frames > MAX_QUANTUM_FRAMES
            || profile.max_runs == 0
            || profile.max_runs > profile.frames
            || capabilities.windows(2).any(|pair| pair[0].id >= pair[1].id)
        {
            return Err(Invalid("invalid foreign automation admission profile"));
        }
        hex_project::validate_device_automation(lanes).map_err(|error| Invalid(error.0))?;
        let authored = lanes.iter().filter_map(|lane| match lane.target {
            DeviceAutomationTarget::Vst3 { parameter } if lane.is_reading() => {
                Some((parameter, lane))
            }
            _ => None,
        });
        let lane_count = authored.clone().count();
        if lane_count == 0 {
            return Ok(None);
        }
        if lane_count > MAX_POINTS {
            return Err(Invalid(
                "foreign automation exceeds the quantum point budget",
            ));
        }
        let mut prepared = Vec::new();
        prepared
            .try_reserve_exact(lane_count)
            .map_err(|_| Invalid("could not reserve foreign automation lanes"))?;
        let mut point_budget = 0;
        for (id, lane) in authored {
            if lane.vst3_class != Some(class) {
                return Err(Invalid("foreign automation differs from the loaded class"));
            }
            let index = capabilities
                .binary_search_by_key(&id, |parameter| parameter.id)
                .map_err(|_| Invalid("foreign automation parameter is unavailable"))?;
            let knots = prepare_knots(&lane.points, capabilities[index].steps, tempo)?;
            point_budget += point_bound(&knots, profile);
            if point_budget > MAX_POINTS {
                return Err(Invalid(
                    "foreign automation exceeds the quantum point budget",
                ));
            }
            prepared.push(Lane { id, knots });
        }
        Ok(Some(Self {
            lanes: prepared,
            profile,
            point_budget,
        }))
    }

    pub fn point_budget(&self) -> usize {
        self.point_budget
    }

    /// Retained lane and knot allocation bytes. The outer owner accounts for
    /// its own inline or boxed storage separately.
    pub fn backing_bytes(&self) -> Option<usize> {
        self.lanes.iter().try_fold(
            self.lanes
                .capacity()
                .checked_mul(std::mem::size_of::<Lane>())?,
            |size, lane| {
                size.checked_add(
                    lane.knots
                        .capacity()
                        .checked_mul(std::mem::size_of::<Knot>())?,
                )
            },
        )
    }

    /// Fill caller-owned storage in physical frame order. The SDK interpolates
    /// between points for one parameter. Points on both sides of each adjacent
    /// discontinuity prevent an unintended ramp. Missing origins emit nothing.
    /// An interior missing span requires an SDK slice cut before chasing again.
    pub fn render_quantum(
        &self,
        origins: &[Option<u64>],
        slice_cuts: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, Invalid> {
        if origins.len() != usize::from(self.profile.frames)
            || output.len() < self.point_budget
            || slice_cuts.windows(2).any(|pair| pair[0] >= pair[1])
            || slice_cuts.first() == Some(&0)
            || slice_cuts
                .last()
                .is_some_and(|&cut| cut >= self.profile.frames)
        {
            return Err(Invalid(
                "foreign automation quantum differs from its admission",
            ));
        }

        let mut run_count = 0;
        let mut unavailable = false;
        let mut seen_origin = false;
        for (index, origin) in origins.iter().enumerate() {
            if origin.is_none() {
                unavailable |= seen_origin;
                continue;
            }
            let cut = slice_cuts.binary_search(&(index as u16)).is_ok();
            if unavailable && !cut {
                return Err(Invalid(
                    "foreign automation origin resumed without a slice cut",
                ));
            }
            if cut || index == 0 || !continuous(origins[index - 1], *origin) {
                run_count += 1;
            }
            unavailable = false;
            seen_origin = true;
        }
        if run_count > self.profile.max_runs {
            return Err(Invalid("foreign automation origin runs exceed admission"));
        }

        let mut count = 0;
        let mut start = 0;
        while start < origins.len() {
            let Some(origin) = origins[start] else {
                start += 1;
                continue;
            };
            let mut end = start + 1;
            while end < origins.len()
                && continuous(origins[end - 1], origins[end])
                && slice_cuts.binary_search(&(end as u16)).is_err()
            {
                end += 1;
            }
            for lane in &self.lanes {
                lane.render_run(start, end, origin, output, &mut count)?;
            }
            start = end;
        }
        output[..count].sort_unstable_by_key(|point| (point.frame, point.id));
        Ok(count)
    }
}

impl hex_lattice::ForeignAutomationProgram for Program {
    fn render_quantum(
        &self,
        origins: &[Option<u64>],
        slice_cuts: &[u16],
        output: &mut [ForeignParameterPoint],
    ) -> Result<usize, hex_lattice_contract::DeviceContractError> {
        self.render_quantum(origins, slice_cuts, output)
            .map_err(|_| hex_lattice_contract::DeviceContractError::ForeignProcessingFailed)
    }
}

fn continuous(previous: Option<u64>, next: Option<u64>) -> bool {
    previous
        .and_then(|frame| frame.checked_add(1))
        .is_some_and(|frame| Some(frame) == next)
}

fn prepare_knots(
    points: &[AutomationPoint],
    steps: u32,
    tempo: &TempoMap,
) -> Result<Vec<Knot>, Invalid> {
    let mut knots = Vec::<Knot>::new();
    let mut push = |beat, value, linear| -> Result<(), Invalid> {
        let frame = tempo
            .beat_to_frame(beat)
            .map_err(|error| Invalid(error.0))?;
        let knot = Knot {
            frame,
            value,
            linear,
        };
        if let Some(last) = knots.last_mut().filter(|last| last.frame == frame) {
            *last = knot;
        } else {
            knots
                .try_reserve(1)
                .map_err(|_| Invalid("could not reserve foreign automation knots"))?;
            knots.push(knot);
        }
        Ok(())
    };
    for (index, point) in points.iter().enumerate() {
        let value = f64::from(point.value.get());
        let next = points.get(index + 1);
        let linear =
            steps == 0 && point.outgoing == AutomationInterpolation::Linear && next.is_some();
        let value = if steps == 0 {
            value
        } else {
            let steps = f64::from(steps);
            (value * steps).round() / steps
        };
        push(point.beat, value, linear)?;
        if linear && let Some(next) = next {
            let markers = tempo.points();
            let first = markers.partition_point(|marker| marker.beat() <= point.beat);
            let end = markers.partition_point(|marker| marker.beat() < next.beat);
            for marker in &markers[first..end] {
                let fraction = (marker.beat().raw() - point.beat.raw()) as f64
                    / (next.beat.raw() - point.beat.raw()) as f64;
                let marker_value = value + (f64::from(next.value.get()) - value) * fraction;
                push(marker.beat(), marker_value, true)?;
            }
        }
    }
    Ok(knots)
}

fn point_bound(knots: &[Knot], profile: QuantumProfile) -> usize {
    // Each run starts with a chase and has at most two points per section.
    // A sliding window bounds the number of additional sections visited by
    // any contiguous run of this quantum size, regardless of timeline start.
    let mut start = 0;
    let mut most_knots = 0;
    for (end, knot) in knots.iter().enumerate() {
        while knot.frame - knots[start].frame >= u64::from(profile.frames) {
            start += 1;
        }
        most_knots = most_knots.max(end - start + 1);
    }
    let sections = if knots.len() == 1 { 1 } else { most_knots + 1 };
    usize::from(profile.frames).min(2 * sections * usize::from(profile.max_runs))
}

impl Lane {
    fn render_run(
        &self,
        start: usize,
        end: usize,
        origin: u64,
        output: &mut [ForeignParameterPoint],
        count: &mut usize,
    ) -> Result<(), Invalid> {
        let mut offset = start;
        let mut frame = origin;
        let mut next = if self.knots.len() == 1 {
            1
        } else {
            self.knots.partition_point(|knot| knot.frame <= frame)
        };
        while offset < end {
            let width = self.knots.get(next).map_or(end - offset, |knot| {
                (end - offset).min(usize::try_from(knot.frame - frame).unwrap_or(usize::MAX))
            });
            let last = frame
                .checked_add((width - 1) as u64)
                .ok_or(Invalid("foreign automation origin exceeds frame range"))?;
            self.emit(offset, self.value(frame, next), output, count)?;
            if width > 1 {
                self.emit(offset + width - 1, self.value(last, next), output, count)?;
            }
            offset += width;
            if offset < end {
                frame = last + 1;
                next += 1;
            }
        }
        Ok(())
    }

    fn value(&self, frame: u64, next: usize) -> f64 {
        if next == 0 {
            return self.knots[0].value;
        }
        let left = self.knots[next - 1];
        if !left.linear {
            return left.value;
        }
        let Some(right) = self.knots.get(next) else {
            return left.value;
        };
        let fraction = (frame - left.frame) as f64 / (right.frame - left.frame) as f64;
        (left.value + (right.value - left.value) * fraction).clamp(0.0, 1.0)
    }

    fn emit(
        &self,
        offset: usize,
        normalized: f64,
        output: &mut [ForeignParameterPoint],
        count: &mut usize,
    ) -> Result<(), Invalid> {
        let slot = output
            .get_mut(*count)
            .ok_or(Invalid("foreign automation output exceeds admission"))?;
        *slot = ForeignParameterPoint {
            frame: offset as u16,
            id: self.id,
            normalized,
        };
        *count += 1;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
