//! Deterministic bipolar sample-and-hold. One SplitMix64 step is consumed
//! at construction and after each complete cycle. Playback partitioning
//! does not affect the sequence. At zero hertz the first value holds.
const STEP: u64 = 0x9e37_79b9_7f4a_7c15;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidRandomHold;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RandomHold {
    seed: u64,
    cursor: u64,
    held: f32,
    phase: f64,
    increment: f64,
}

impl RandomHold {
    /// Prepare a free-running rate from zero to Nyquist in hertz.
    pub fn new(seed: u64, sample_rate: u32, hz: f64) -> Result<Self, InvalidRandomHold> {
        if sample_rate == 0 || !(0.0..=f64::from(sample_rate) * 0.5).contains(&hz) {
            return Err(InvalidRandomHold);
        }
        let cursor = seed.wrapping_add(STEP);
        Ok(Self {
            seed,
            cursor,
            held: bipolar(cursor),
            phase: 0.0,
            increment: hz / f64::from(sample_rate),
        })
    }

    /// Same-seed handoff preserves the held value and cycle phase while the
    /// prepared rate takes effect. A new seed starts its sequence immediately.
    pub fn inherit_history(&mut self, previous: &Self) {
        if self.seed == previous.seed {
            self.cursor = previous.cursor;
            self.held = previous.held;
            self.phase = previous.phase;
        }
    }

    pub fn render(&mut self, output: &mut [f32]) {
        for sample in output {
            *sample = self.held;
            self.phase += self.increment;
            if self.phase >= 1.0 {
                self.phase -= 1.0;
                self.cursor = self.cursor.wrapping_add(STEP);
                self.held = bipolar(self.cursor);
            }
        }
    }
}

fn bipolar(mut value: u64) -> f32 {
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    let uniform = (value ^ (value >> 31)) >> 40;
    uniform as f32 / 8_388_608.0 - 1.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn independently_calculated_seed_sequences_hold_at_exact_sample_offsets() {
        let steps = [0x3f44_4150, 0xbe0c_3b10, 0xbf72_7746, 0x3f71_1770];
        let mut whole = RandomHold::new(0, 8, 2.0).unwrap();
        let mut split = whole;
        let mut output = [0.0; 16];
        whole.render(&mut output);
        for (index, value) in output.into_iter().enumerate() {
            assert_eq!(value.to_bits(), steps[index / 4]);
        }
        let mut partitioned = [0.0; 16];
        for chunk in partitioned.chunks_mut(3) {
            split.render(chunk);
        }
        assert_eq!(partitioned, output);
        let mut other = RandomHold::new(1, 8, 0.0).unwrap();
        other.render(&mut output);
        assert!(output.iter().all(|sample| sample.to_bits() == 0x3e08_5168));
    }

    #[test]
    fn same_seed_retarget_preserves_phase_and_new_seed_restarts() {
        let mut initial = RandomHold::new(0, 8, 2.0).unwrap();
        initial.render(&mut [0.0; 3]);
        let mut faster = RandomHold::new(0, 8, 4.0).unwrap();
        faster.inherit_history(&initial);
        let mut output = [0.0; 2];
        faster.render(&mut output);
        assert_eq!(output.map(f32::to_bits), [0x3f44_4150, 0xbe0c_3b10]);

        let mut reseeded = RandomHold::new(1, 8, 4.0).unwrap();
        reseeded.inherit_history(&faster);
        reseeded.render(&mut output);
        assert_eq!(output.map(f32::to_bits), [0x3e08_5168; 2]);
        for hz in [f64::NAN, f64::INFINITY, -0.1, 4.1] {
            assert!(RandomHold::new(0, 8, hz).is_err());
        }
        assert!(RandomHold::new(0, 0, 0.0).is_err());
    }
}
