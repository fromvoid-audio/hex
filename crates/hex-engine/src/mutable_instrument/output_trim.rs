//! Sample-rate host trim applies to an isolated instrument contribution.
//! A detached release uses the last accepted gain and leaves the bank parked.
use crate::device_modulation::INSTRUMENT_MODULATION_CHUNK_FRAMES;
use hex_lattice::StereoFrame;
use hex_lattice_contract::HostGain;

#[derive(Clone, Copy)]
pub(super) struct OutputTrim {
    pub(super) base: HostGain,
    pub(super) committed: HostGain,
    pub(super) scratch: HostGain,
}

impl OutputTrim {
    pub(super) const fn new(base: HostGain) -> Self {
        Self {
            base,
            committed: base,
            scratch: base,
        }
    }
}

pub(super) fn render_contribution<E>(
    output: &mut [StereoFrame],
    gain: impl Fn(usize) -> f32,
    over_budget: E,
    render: impl FnOnce(&mut [StereoFrame]) -> Result<(), E>,
) -> Result<(), E> {
    let mut scratch = [[0.0; 2]; INSTRUMENT_MODULATION_CHUNK_FRAMES];
    let contribution = scratch.get_mut(..output.len()).ok_or(over_budget)?;
    render(contribution)?;
    for (offset, (destination, source)) in output.iter_mut().zip(contribution).enumerate() {
        let gain = gain(offset);
        destination[0] += source[0] * gain;
        destination[1] += source[1] * gain;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_accepted_contribution_is_scaled_and_mixed() {
        let mut output = [[2.0, -2.0], [3.0, -3.0]];
        let original = output;
        assert_eq!(
            render_contribution(
                &mut output,
                |_| 2.0,
                "too long",
                |scratch| {
                    scratch.fill([0.5, -0.5]);
                    Err("refused")
                }
            ),
            Err("refused")
        );
        assert_eq!(output, original);
        render_contribution(
            &mut output,
            |offset| (offset + 2) as f32,
            "too long",
            |scratch| {
                assert_eq!(scratch, &[[0.0; 2]; 2]);
                scratch.fill([0.5, -0.5]);
                Ok(())
            },
        )
        .unwrap();
        assert_eq!(output, [[3.0, -3.0], [4.5, -4.5]]);

        let mut overflow = [[1.0; 2]; INSTRUMENT_MODULATION_CHUNK_FRAMES + 1];
        assert_eq!(
            render_contribution(
                &mut overflow,
                |_| 1.0,
                "too long",
                |_| panic!("rendered overflow")
            ),
            Err("too long")
        );
        assert!(overflow.iter().all(|frame| *frame == [1.0; 2]));
    }
}
