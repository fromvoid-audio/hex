//! Authored decay belongs to logical Preserve segments, never overlap grains.
use super::*;
pub(super) fn apply(
    schedule: &plan::Schedule<'_>,
    pcm: &mut [Frame],
    decay: u8,
    work: &mut Work<'_>,
) -> Result<(), Error> {
    if decay == 100 {
        return Ok(());
    }
    for index in 0..schedule.len() - 1 {
        let start = schedule.point(index).output;
        let end = schedule.point(index + 1).output;
        for (age, frame) in pcm[start..end].iter_mut().enumerate() {
            if age % 1024 == 0 {
                work.check(1024 * 8)?;
            }
            let gain =
                (-12.0 * f64::from(100 - decay) / 100.0 * age as f64 / (end - start) as f64).exp();
            *frame = finite(frame.map(|x| f64::from(x) * gain))?;
        }
    }
    Ok(())
}
