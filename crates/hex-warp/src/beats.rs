//! Transient-anchored, stereo-linked control-side WSOLA.
//! Forward: physical guards and logical envelopes have separate schedules.
mod analysis;
mod envelope;
mod groups;
mod onsets;
mod plan;
mod stop;
mod synthesis;
use super::*;
use resample::{Bandlimited, bank_bytes};

// Detected source onsets are independent of authored warp and envelope points.
// Admit one full projected schedule's worth without changing either map limit.
const MAX_DETECTED_TRANSIENTS: usize = MAX_MAP_SEGMENTS;
// The full-rate stereo spectral pass alone can exceed the shared work budget
// on a long clip. Keep a separate ceiling for Beats analysis and synthesis.
pub(super) const MAX_RENDER_WORK_UNITS: u64 = 4 * MAX_WORK_UNITS;

#[derive(Clone, Copy)]
struct Anchor {
    source: f64,
    output: usize,
    before: usize,
    after: usize,
    attack: bool,
}

fn plan_bytes(request: &Request<'_>) -> usize {
    MAX_DETECTED_TRANSIENTS * std::mem::size_of::<u64>()
        + (MAX_MAP_SEGMENTS + 2) * std::mem::size_of::<Anchor>()
        + plan::scratch(request)
}

pub(super) fn render(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Frame>, Error> {
    if request.attacks.is_empty()
        && let Mode::Beats {
            division_frames: Some(division),
            ..
        } = request.mode
        && (division == 0 || (request.input.region_length - 1) / division + 1 > MAX_POINTS as u64)
    {
        return Err(Error::Invalid("beat divisions exceed bounded source plan"));
    }
    let Mode::Beats { decay, .. } = request.mode else {
        return Err(Error::Invalid("beats mode mismatch"));
    };
    let unity = pitch == 1.0
        && request.map.segments().iter().all(|s| {
            s.source_end_q32 - s.source_start_q32 == i128::from(s.output_end - s.output_start) * Q32
        })
        && request.map.segments()[0].source_start_q32.rem_euclid(Q32) == 0;
    // Untouched unity is waveform-exact, including arbitrary authored source
    // attacks, trim/phase/loop edges. No analysis may manufacture grid fades.
    if unity && decay == 100 {
        return copy(request, work);
    }
    let max_grain = (request.input.sample_rate as usize / 20).max(4);
    let ring = max_grain * 2 + 4;
    let plan_bytes = plan_bytes(request);
    // Work::scratch is a maximum recorder, not an adder. Quote the complete
    // concurrently live plan/search/filter/ring family before its first Vec.
    work.scratch(
        plan_bytes
            + onsets::scratch(request.input.sample_rate)
            + bank_bytes(work.resampler)
            + ring * 24
            + synthesis::SEARCH_BYTES,
    )?;
    let anchors = analysis::prepare(request, pitch, work)?;
    let schedule = plan::Schedule::prepare(request, &anchors, work)?;
    if unity {
        let mut pcm = copy(request, work)?;
        envelope::apply(&schedule, &mut pcm, decay, work)?;
        return Ok(pcm);
    }
    let bank = Bandlimited::new(pitch, work)?;
    if matches!(schedule, plan::Schedule::Chosen(_)) {
        let mut pcm = groups::render(request, &anchors, &schedule, &bank, pitch, work)?;
        envelope::apply(&schedule, &mut pcm, decay, work)?;
        return Ok(pcm);
    }
    if matches!(
        request.mode,
        Mode::Beats {
            tail: Tail::Off,
            ..
        }
    ) {
        let mut pcm = stop::render(request, &anchors, &schedule, &bank, pitch, work)?;
        envelope::apply(&schedule, &mut pcm, decay, work)?;
        return Ok(pcm);
    }
    let mut acc = Accumulator::new(
        request.output_frames,
        ring,
        work,
        plan_bytes + bank_bytes(work.resampler) + synthesis::SEARCH_BYTES,
    )?;
    let mut output = 0;
    let mut source = anchors[0].source;
    let mut previous = 0;
    for (i, anchor) in anchors.iter().enumerate().take(anchors.len() - 1) {
        if i > 0 && !anchor.attack {
            continue;
        }
        let next = (i + 1..anchors.len())
            .find(|j| anchors[*j].attack || *j == anchors.len() - 1)
            .expect("the final anchor always terminates the search");
        let start = anchor.output.saturating_sub(anchor.before);
        let end = (anchor.output + anchor.after).min(request.output_frames);
        synthesis::bed(
            request,
            &bank,
            work,
            &mut acc,
            plan::Span {
                start: output,
                end: start,
                source_start: source,
                source_end: anchor.source - anchor.before as f64 * pitch,
                locked_end: anchor.attack,
            },
            pitch,
            &schedule,
        )?;
        for frame in start..end {
            if frame % 256 == 0 {
                work.check(256 * 64)?;
            }
            let position = anchor.source + (frame as f64 - anchor.output as f64) * pitch;
            let (lower, upper) = if frame < anchor.output {
                (
                    anchors[previous].source.floor() as i128,
                    anchor.source.ceil() as i128 - 1,
                )
            } else {
                (
                    anchor.source.floor() as i128,
                    anchors[next].source.ceil() as i128 - 1,
                )
            };
            let sample = bank.sample_segment(request.input, position, lower, upper.max(lower));
            acc.add(frame as i128, sample, 1.0)?;
        }
        acc.finish_until(end)?;
        output = end;
        source = anchor.source + anchor.after as f64 * pitch;
        previous = i;
    }
    synthesis::bed(
        request,
        &bank,
        work,
        &mut acc,
        plan::Span {
            start: output,
            end: request.output_frames,
            source_start: source,
            source_end: anchors.last().expect("anchors indexed above").source,
            locked_end: false,
        },
        pitch,
        &schedule,
    )?;
    let mut pcm = acc.finish()?;
    envelope::apply(&schedule, &mut pcm, decay, work)?;
    Ok(pcm)
}
fn copy(request: &Request<'_>, work: &mut Work<'_>) -> Result<Vec<Frame>, Error> {
    let mut out = buffer(request.output_frames, [0.0; 2])?;
    for (i, frame) in out.iter_mut().enumerate() {
        if i % 1024 == 0 {
            work.check(1024)?;
        }
        *frame = finite(
            request
                .input
                .frame(request.map.position_q32(i as i128)? / Q32),
        )?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detected_transients_beyond_the_authored_point_limit_still_anchor_audio() {
        const ATTACK: Frame = [0.9, -0.45];
        let mut frames = vec![[0.0; 2]; 132_300];
        for frame in frames.iter_mut().skip(100).step_by(100) {
            *frame = ATTACK;
        }
        let output_frames = frames.len() * 11 / 10;
        let map = TimeMap::new(
            vec![MapSegment {
                output_start: 0,
                output_end: output_frames as u64,
                source_start_q32: 0,
                source_end_q32: frames.len() as i128 * Q32,
            }],
            output_frames,
        )
        .unwrap();
        let rendered = crate::render(
            &Request {
                attacks: &[],
                input: Input {
                    frames: &frames,
                    sample_rate: 44_100,
                    region_start: 0,
                    region_length: frames.len() as u64,
                    phase: 0,
                    looping: false,
                    envelope: &[],
                },
                map: &map,
                output_frames,
                pitch_cents: 0,
                mode: Mode::Beats {
                    division_frames: None,
                    tail: Tail::Forward,
                    decay: 100,
                },
            },
            &mut || false,
        )
        .unwrap();
        assert_eq!(rendered.frames.len(), output_frames);
        for attack in [100, 120_000, 132_200] {
            assert_eq!(rendered.frames[attack * 11 / 10], ATTACK);
        }
        assert!(rendered.peak_scratch_bytes <= MAX_SCRATCH_BYTES);
    }
}
