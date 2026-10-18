//! Authored decay belongs to logical Preserve segments, never overlap grains.
use super::*;

pub(super) fn scratch(decay: u8) -> usize {
    if decay == 100 {
        0
    } else {
        (MAX_MAP_SEGMENTS + 2) * std::mem::size_of::<usize>()
    }
}
fn push(points: &mut Vec<usize>, at: usize) -> Result<(), Error> {
    if points.last().is_some_and(|old| *old >= at) {
        return Ok(());
    }
    if points.len() == MAX_MAP_SEGMENTS + 2 {
        return Err(Error::Invalid(
            "logical Preserve schedule exceeds bounded plan",
        ));
    }
    points.push(at);
    Ok(())
}
fn interval(
    pcm: &mut [Frame],
    start: usize,
    end: usize,
    decay: u8,
    work: &mut Work<'_>,
) -> Result<(), Error> {
    let duration = end - start;
    for (age, frame) in pcm[start..end].iter_mut().enumerate() {
        if age % 1024 == 0 {
            work.check(1024 * 8)?;
        }
        let gain = (-12.0 * f64::from(100 - decay) / 100.0 * age as f64 / duration as f64).exp();
        *frame = finite(frame.map(|x| f64::from(x) * gain))?;
    }
    Ok(())
}
pub(super) fn apply(
    request: &Request<'_>,
    physical: &[Anchor],
    pcm: &mut [Frame],
    decay: u8,
    work: &mut Work<'_>,
) -> Result<(), Error> {
    if decay == 100 {
        return Ok(());
    }
    let Mode::Beats {
        division_frames, ..
    } = request.mode
    else {
        return Err(Error::Invalid("logical Preserve mode mismatch"));
    };
    if request.attacks.is_empty() && division_frames.is_none() {
        for pair in physical.windows(2) {
            interval(pcm, pair[0].output, pair[1].output, decay, work)?;
        }
        return Ok(());
    }
    // Reserved once, before filling. These Copy output indices introduce no
    // audio owners or second PCM buffer. The parent quote includes all of them.
    let mut logical = Vec::new();
    logical
        .try_reserve_exact(MAX_MAP_SEGMENTS + 2)
        .map_err(|_| Error::Allocation)?;
    logical.push(0);
    let region = i128::from(request.input.region_length);
    let first = request.map.segments()[0]
        .source_start_q32
        .div_euclid(Q32)
        .div_euclid(region)
        .max(0);
    let last = request
        .map
        .segments()
        .last()
        .unwrap()
        .source_end_q32
        .div_euclid(Q32)
        .div_euclid(region);
    if last - first > MAX_MAP_SEGMENTS as i128 {
        return Err(Error::Invalid(
            "logical Preserve cycle count exceeds bounded plan",
        ));
    }
    let count = if request.attacks.is_empty() {
        ((request.input.region_length - 1) / division_frames.unwrap() + 1) as usize
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
                i as u64 * division_frames.unwrap()
            } else {
                request.attacks[i]
            };
            let source = cycle * region + i128::from(at);
            if let Some(output) = request.map.output_for_source(source * Q32)
                && output < pcm.len() as u64
            {
                push(&mut logical, output as usize)?;
            }
        }
    }
    push(&mut logical, pcm.len())?;
    for pair in logical.windows(2) {
        interval(pcm, pair[0], pair[1], decay, work)?;
    }
    Ok(())
}
