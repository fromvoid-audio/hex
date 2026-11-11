//! Distinct physical attacks and exact-map protected neighborhoods.
use super::*;
fn push(
    anchors: &mut Vec<Anchor>,
    source: f64,
    output: usize,
    attack: bool,
    source_only: bool,
) -> Result<(), Error> {
    if let Some(last) = anchors.last_mut() {
        if source_only && (last.source - source).abs() < 1e-9 {
            last.attack |= attack;
            return Ok(());
        }
        if !source_only && last.output == output {
            if (last.source - source).abs() > 1e-9 && attack && last.attack {
                return Err(Error::Invalid(
                    "distinct attacks project onto the same output sample",
                ));
            }
            if attack && !last.attack {
                last.source = source;
                last.attack = true;
            }
            return Ok(());
        }
    }
    if anchors.len() > MAX_MAP_SEGMENTS {
        return Err(Error::Invalid("transient schedule exceeds bounded plan"));
    }
    anchors.try_reserve(1).map_err(|_| Error::Allocation)?;
    anchors.push(Anchor {
        source,
        output,
        before: 0,
        after: 0,
        attack,
    });
    Ok(())
}
pub(super) fn prepare(
    request: &Request<'_>,
    pitch: f64,
    work: &mut Work<'_>,
) -> Result<Vec<Anchor>, Error> {
    let rate = request.input.sample_rate;
    let source_only = !request.attacks.is_empty()
        || matches!(
            request.mode,
            Mode::Beats {
                division_frames: Some(_),
                ..
            }
        );
    let separation = u64::from((rate / 1000).max(1));
    let mut starts = Vec::new();
    starts
        .try_reserve_exact(MAX_DETECTED_TRANSIENTS)
        .map_err(|_| Error::Allocation)?;
    starts.push(0_u64);
    let warmup = request
        .input
        .region_length
        .min(u64::from((rate / 200).max(1)))
        .max(1);
    let mut slow = 0.0;
    for i in 0..warmup {
        if i % 1024 == 0 {
            work.check(1024)?;
        }
        let x = request.input.frame(i128::from(i));
        slow += (x[0] * x[0] + x[1] * x[1]) / warmup as f64;
    }
    let coefficient = 1.0 - (-1.0 / (f64::from(rate) * 0.020)).exp();
    let mut last = None;
    for i in 0..request.input.region_length {
        if i % 1024 == 0 {
            work.check(1024)?;
        }
        let x = request.input.frame(i128::from(i));
        let energy = x[0] * x[0] + x[1] * x[1];
        if last.is_none_or(|last| i >= last + separation) && energy > 1e-12 && energy > slow * 8.0 {
            if starts.len() == MAX_DETECTED_TRANSIENTS {
                return Err(Error::Invalid("transients exceed bounded source plan"));
            }
            if i != 0 {
                starts.push(i);
            }
            last = Some(i);
        }
        slow += (energy - slow) * coefficient;
    }
    onsets::detect(request.input, &mut starts, work)?;
    let first = request.map.segments()[0].source_start_q32;
    let end = request
        .map
        .segments()
        .last()
        .expect("segments()[0] indexed above")
        .source_end_q32;
    let region = i128::from(request.input.region_length);
    let first_cycle = first.div_euclid(Q32).div_euclid(region);
    let first_cycle = if request.input.looping {
        first_cycle
    } else {
        first_cycle.max(0)
    };
    let boundary_attack = onsets::cycle_boundary(request.input, work)?;
    let last_cycle = end.div_euclid(Q32).div_euclid(region);
    if last_cycle - first_cycle > MAX_MAP_SEGMENTS as i128 {
        return Err(Error::Invalid("transient cycle count exceeds bounded plan"));
    }
    let mut anchors = Vec::new();
    let capacity = ((last_cycle - first_cycle + 1).max(0) as usize)
        .saturating_mul(starts.len())
        .saturating_add(2)
        .min(MAX_MAP_SEGMENTS + 2);
    anchors
        .try_reserve_exact(capacity)
        .map_err(|_| Error::Allocation)?;
    push(
        &mut anchors,
        first as f64 / Q32 as f64,
        0,
        false,
        source_only,
    )?;
    for cycle in first_cycle..=last_cycle {
        work.check(starts.len() as u64)?;
        if cycle > 0 && !request.input.looping {
            break;
        }
        for at in &starts {
            let source = cycle * region + i128::from(*at);
            if let Some(output) = request.map.output_for_source(source * Q32)
                && output < (request.output_frames as u64)
            {
                let attack = *at != 0 || output != 0 && boundary_attack;
                push(
                    &mut anchors,
                    source as f64,
                    output as usize,
                    attack,
                    source_only,
                )?;
            }
        }
    }
    push(
        &mut anchors,
        end as f64 / Q32 as f64,
        request.output_frames,
        false,
        source_only,
    )?;
    // Chosen groups schedule the audible first pass. Here all physical source
    // barriers survive, even if a global map would collide or omit their
    // audible times. Their global output projections are not timing authority.
    if source_only {
        return Ok(anchors);
    }
    work.check(anchors.len() as u64 * 8)?;
    for i in 0..anchors.len() - 1 {
        if i > 0 && !anchors[i].attack {
            continue;
        }
        let previous = (0..i)
            .rev()
            .find(|j| *j == 0 || anchors[*j].attack)
            .unwrap_or(0);
        let next = (i + 1..anchors.len())
            .find(|j| anchors[*j].attack || *j == anchors.len() - 1)
            .expect("the final anchor always terminates the search");
        let before = if i == 0 {
            0
        } else {
            ((f64::from(rate) / 1000.0 / pitch).floor() as usize)
                .min((anchors[i].output - anchors[previous].output) / 3)
                .min(
                    ((anchors[i].source - anchors[previous].source) / pitch / 3.0)
                        .floor()
                        .max(0.0) as usize,
                )
        };
        let after = ((f64::from(rate) / 125.0 / pitch).floor() as usize)
            .max(1)
            .min((anchors[next].output - anchors[i].output).div_ceil(3))
            .min(
                ((anchors[next].source - anchors[i].source) / pitch / 3.0)
                    .ceil()
                    .max(1.0) as usize,
            );
        anchors[i].before = before;
        anchors[i].after = after;
    }
    Ok(anchors)
}
