//! Editable scale and the MIDI-frequency table sent to MTS-ESP.

/// Maximum number of notes in one repeating period.
pub const MAX_DEGREES: usize = 48;
/// Minimum separation between neighboring degrees while dragging.
pub const MIN_GAP_CENTS: f32 = 1.0;

/// One repeating tuning period with a selected root degree.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scale {
    /// Number of active degrees per period.
    count: usize,
    /// Ascending cent positions within the period.
    cents: [f32; MAX_DEGREES],
    /// MIDI key that plays the root frequency.
    root_key: u8,
    /// Frequency of the root key.
    root_hz: f64,
    /// Degree assigned to the root key.
    root_degree: usize,
    /// Size of one repeating period in ordinary cents.
    period_cents: f32,
}

impl Default for Scale {
    fn default() -> Self {
        let mut scale = Self::equal(7);
        scale.set_major_tet();
        scale
    }
}

impl Scale {
    pub(crate) fn neutral() -> Self {
        Self {
            count: 12,
            cents: std::array::from_fn(|i| if i < 12 { i as f32 * 100.0 } else { 0.0 }),
            root_key: 69,
            root_hz: 440.0,
            root_degree: 9,
            period_cents: 1200.0,
        }
    }

    /// Validate a complete authored scale before admitting it.
    pub fn new(
        count: usize,
        cents: [f32; MAX_DEGREES],
        root_key: u8,
        root_hz: f64,
        root_degree: usize,
        period_cents: f32,
    ) -> Result<Self, crate::Invalid> {
        let scale = Self {
            count,
            cents,
            root_key,
            root_hz,
            root_degree,
            period_cents,
        };
        if scale.valid() {
            Ok(scale)
        } else {
            Err(crate::Invalid("invalid tuning scale"))
        }
    }
    pub fn count(&self) -> usize {
        self.count
    }
    pub fn cents(&self) -> &[f32; MAX_DEGREES] {
        &self.cents
    }
    pub fn root_key(&self) -> u8 {
        self.root_key
    }
    pub fn root_hz(&self) -> f64 {
        self.root_hz
    }
    pub fn root_degree(&self) -> usize {
        self.root_degree
    }
    pub fn period_cents(&self) -> f32 {
        self.period_cents
    }
    /// Set reference values together, preserving the old scale on refusal.
    pub fn set_reference(
        &mut self,
        root_key: u8,
        root_hz: f64,
        root_degree: usize,
    ) -> Result<(), crate::Invalid> {
        let next = Self::new(
            self.count,
            self.cents,
            root_key,
            root_hz,
            root_degree,
            self.period_cents,
        )?;
        *self = next;
        Ok(())
    }

    /// Construct an equal division of a 2:1 period.
    pub fn equal(count: usize) -> Self {
        let count = count.clamp(3, MAX_DEGREES);
        let mut cents = [0.0; MAX_DEGREES];
        for (index, cent) in cents.iter_mut().enumerate().take(count) {
            *cent = index as f32 * 1200.0 / count as f32;
        }
        Self {
            count,
            cents,
            root_key: 60,
            root_hz: 220.0,
            root_degree: 0,
            period_cents: 1200.0,
        }
    }

    /// Load the default seven-degree 12-TET major pattern.
    pub fn set_major_tet(&mut self) {
        self.count = 7;
        self.period_cents = 1200.0;
        self.cents[..7].copy_from_slice(&[0.0, 200.0, 400.0, 500.0, 700.0, 900.0, 1100.0]);
        self.root_degree = 0;
    }

    /// Load a seven-note five-limit example.
    pub fn set_just_major(&mut self) {
        self.count = 7;
        self.period_cents = 1200.0;
        self.root_degree = 0;
        let ratios: [f64; 7] = [
            1.0,
            9.0 / 8.0,
            5.0 / 4.0,
            4.0 / 3.0,
            3.0 / 2.0,
            5.0 / 3.0,
            15.0 / 8.0,
        ];
        for (index, ratio) in ratios.into_iter().enumerate() {
            self.cents[index] = (1200.0 * ratio.log2()) as f32;
        }
    }

    /// Reset the count and pitches to equal divisions.
    pub fn set_equal(&mut self, count: usize) {
        let root_key = self.root_key;
        let root_hz = self.root_hz;
        let root_degree = self.root_degree;
        let period_cents = self.period_cents;
        *self = Self::equal(count);
        self.root_key = root_key;
        self.root_hz = root_hz;
        self.root_degree = root_degree.min(self.count - 1);
        self.set_period(period_cents);
    }

    /// Change note count while retaining existing pitches and the selected root.
    pub fn set_count(&mut self, count: usize) {
        let target = count.clamp(3, MAX_DEGREES);
        while self.count < target {
            let mut widest = (0.0, self.count - 1);
            for index in 0..self.count {
                let next = if index + 1 == self.count {
                    self.period_cents
                } else {
                    self.cents[index + 1]
                };
                let gap = next - self.cents[index];
                if gap > widest.0 {
                    widest = (gap, index);
                }
            }
            let insert_at = widest.1 + 1;
            let next = if insert_at == self.count {
                self.period_cents
            } else {
                self.cents[insert_at]
            };
            let cents = (self.cents[widest.1] + next) * 0.5;
            self.cents.copy_within(insert_at..self.count, insert_at + 1);
            self.cents[insert_at] = cents;
            self.count += 1;
            if insert_at <= self.root_degree {
                self.root_degree += 1;
            }
        }
        while self.count > target {
            let remove_at = if self.root_degree == self.count - 1 {
                self.count - 2
            } else {
                self.count - 1
            };
            self.cents.copy_within(remove_at + 1..self.count, remove_at);
            self.count -= 1;
            self.cents[self.count] = 0.0;
            if remove_at < self.root_degree {
                self.root_degree -= 1;
            }
        }
    }

    /// Insert a degree halfway between the selected degree and its successor.
    ///
    /// The root remains attached to the same pitch when an earlier degree is
    /// inserted. The returned index identifies the new degree.
    pub fn insert_degree_after(&mut self, index: usize) -> Option<usize> {
        if self.count >= MAX_DEGREES || index >= self.count {
            return None;
        }
        let insert_at = index + 1;
        let next = if insert_at == self.count {
            self.period_cents
        } else {
            self.cents[insert_at]
        };
        let gap = next - self.cents[index];
        if gap < MIN_GAP_CENTS * 2.0 {
            return None;
        }
        let cents = (self.cents[index] + next) * 0.5;
        self.cents.copy_within(insert_at..self.count, insert_at + 1);
        self.cents[insert_at] = cents;
        self.count += 1;
        if insert_at <= self.root_degree {
            self.root_degree += 1;
        }
        Some(insert_at)
    }

    /// Insert a degree at a pitch within the repeat, preserving the root pitch.
    /// Reject endpoints, overlapping notes, and scales at the degree limit.
    pub fn insert_degree(&mut self, cents: f32) -> Option<usize> {
        if self.count >= MAX_DEGREES
            || !cents.is_finite()
            || cents <= 0.0
            || cents > self.period_cents - MIN_GAP_CENTS
        {
            return None;
        }
        let index = self.cents[..self.count].partition_point(|pitch| *pitch < cents);
        if (index > 0 && cents - self.cents[index - 1] < MIN_GAP_CENTS)
            || (index < self.count && self.cents[index] - cents < MIN_GAP_CENTS)
        {
            return None;
        }
        self.cents.copy_within(index..self.count, index + 1);
        self.cents[index] = cents;
        self.count += 1;
        if index <= self.root_degree {
            self.root_degree += 1;
        }
        Some(index)
    }

    /// Remove one degree while retaining at least three notes per period.
    ///
    /// The root degree cannot be removed. Indices after the removed degree are
    /// shifted down, preserving the root's pitch and MIDI assignment.
    pub fn remove_degree(&mut self, index: usize) -> bool {
        if self.count <= 3 || index >= self.count || index == self.root_degree {
            return false;
        }
        self.cents.copy_within(index + 1..self.count, index);
        self.count -= 1;
        self.cents[self.count] = 0.0;
        if index < self.root_degree {
            self.root_degree -= 1;
        }
        true
    }

    /// Set a degree's absolute pitch in hertz, constrained by its neighbors.
    /// Editing the root degree changes the reference frequency; other degrees
    /// retain the reference and move within their existing ordered interval.
    pub fn set_degree_frequency(&mut self, index: usize, hz: f64) -> bool {
        if index >= self.count || !hz.is_finite() || hz <= 0.0 {
            return false;
        }
        if index == self.root_degree {
            if !(10.0..=20_000.0).contains(&hz) {
                return false;
            }
            if (self.root_hz - hz).abs() < 0.000_1 {
                return false;
            }
            self.root_hz = hz;
            return true;
        }
        let cents = self.cents[self.root_degree] + (1200.0 * (hz / self.root_hz).log2()) as f32;
        self.move_degree(index, cents)
    }

    /// Return the absolute pitch of one degree in hertz.
    pub fn degree_frequency(&self, index: usize) -> Option<f64> {
        (index < self.count).then(|| {
            self.root_hz
                * 2.0_f64.powf(f64::from(self.cents[index] - self.cents[self.root_degree]) / 1200.0)
        })
    }

    /// Stretch or compress the existing pattern to a new period size.
    pub fn set_period(&mut self, period_cents: f32) {
        if !period_cents.is_finite() || !(600.0..=2400.0).contains(&period_cents) {
            return;
        }
        let factor = period_cents / self.period_cents;
        let mut candidate = *self;
        for cents in candidate.cents.iter_mut().take(candidate.count) {
            *cents *= factor;
        }
        candidate.period_cents = period_cents;
        if candidate.valid() {
            *self = candidate;
        }
    }

    /// Move one degree without changing order or the period endpoints.
    pub fn move_degree(&mut self, index: usize, cents: f32) -> bool {
        if index >= self.count || !cents.is_finite() {
            return false;
        }
        let lower = if index == 0 {
            0.0
        } else {
            self.cents[index - 1] + MIN_GAP_CENTS
        };
        let upper = if index + 1 == self.count {
            self.period_cents
        } else {
            self.cents[index + 1] - MIN_GAP_CENTS
        };
        let next = cents.clamp(lower, upper.max(lower));
        if (next - self.cents[index]).abs() < 0.001 {
            return false;
        }
        self.cents[index] = next;
        true
    }

    /// Return the absolute frequency assigned to one MIDI key.
    pub fn frequency(&self, key: u8) -> f64 {
        let offset = i32::from(key) - i32::from(self.root_key);
        let position = self.root_degree as i32 + offset;
        let period = position.div_euclid(self.count as i32);
        let degree = position.rem_euclid(self.count as i32) as usize;
        let cents = f64::from(self.cents[degree] - self.cents[self.root_degree])
            + f64::from(period) * f64::from(self.period_cents);
        self.root_hz * 2.0_f64.powf(cents / 1200.0)
    }

    /// Build the full single-channel MTS-ESP tuning table.
    pub fn frequencies(&self) -> [f64; 128] {
        std::array::from_fn(|key| self.frequency(key as u8))
    }

    /// Check persisted state before accepting it.
    pub fn valid(&self) -> bool {
        if !(3..=MAX_DEGREES).contains(&self.count)
            || self.root_key > 127
            || !self.root_hz.is_finite()
            || !(10.0..=20_000.0).contains(&self.root_hz)
            || self.root_degree >= self.count
            || !self.period_cents.is_finite()
            || !(600.0..=2400.0).contains(&self.period_cents)
            || !self.cents[0].is_finite()
            || self.cents[0] < 0.0
            || !self.cents.iter().all(|cent| cent.is_finite())
        {
            return false;
        }
        (1..self.count).all(|index| {
            self.cents[index].is_finite()
                && self.cents[index] >= self.cents[index - 1] + MIN_GAP_CENTS
                && self.cents[index] <= self.period_cents
        })
    }
}

/// An octave-reduced point of the fifth/third ratio lattice.
#[derive(Clone, Copy, Debug)]
pub struct LatticePoint {
    /// Number of 3:2 generator moves.
    fifths: i32,
    /// Number of 5:4 generator moves.
    thirds: i32,
    /// Octave-reduced position in cents.
    cents: f32,
}

impl LatticePoint {
    /// Create a point in the bounded fifth/third display grid.
    /// Coordinates clamp to ±8 fifths and ±4 thirds before ratio arithmetic.
    pub fn new(fifths: i32, thirds: i32) -> Self {
        let fifths = fifths.clamp(-8, 8);
        let thirds = thirds.clamp(-4, 4);
        let cents = (f64::from(fifths) * 1200.0 * (3.0_f64 / 2.0).log2()
            + f64::from(thirds) * 1200.0 * (5.0_f64 / 4.0).log2())
        .rem_euclid(1200.0) as f32;
        Self {
            fifths,
            thirds,
            cents,
        }
    }

    pub fn fifths(self) -> i32 {
        self.fifths
    }
    pub fn thirds(self) -> i32 {
        self.thirds
    }
    pub fn cents(self) -> f32 {
        self.cents
    }

    /// Return this landmark as an octave-reduced integer frequency ratio.
    pub fn ratio(self) -> (i64, i64) {
        let fifths = self.fifths;
        let thirds = self.thirds;
        let mut numerator = 1_i64;
        let mut denominator = 1_i64;
        if fifths >= 0 {
            numerator *= 3_i64.pow(fifths as u32);
            denominator *= 2_i64.pow(fifths as u32);
        } else {
            numerator *= 2_i64.pow((-fifths) as u32);
            denominator *= 3_i64.pow((-fifths) as u32);
        }
        if thirds >= 0 {
            numerator *= 5_i64.pow(thirds as u32);
            denominator *= 4_i64.pow(thirds as u32);
        } else {
            numerator *= 4_i64.pow((-thirds) as u32);
            denominator *= 5_i64.pow((-thirds) as u32);
        }
        while numerator < denominator {
            numerator *= 2;
        }
        while numerator >= denominator * 2 {
            denominator *= 2;
        }
        let divisor = gcd(numerator, denominator);
        (numerator / divisor, denominator / divisor)
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        let remainder = a % b;
        a = b;
        b = remainder;
    }
    a
}

/// Find a nearby simple-ratio landmark in a bounded fifth/third grid.
pub fn nearest_lattice(cents: f32) -> (LatticePoint, f32) {
    let mut best = LatticePoint::new(0, 0);
    let mut error = circular_cents(cents - best.cents);
    for fifths in -2..=2 {
        for thirds in -1..=1 {
            let candidate = LatticePoint::new(fifths, thirds);
            let candidate_error = circular_cents(cents - candidate.cents);
            let complexity = fifths.abs() + thirds.abs();
            let best_complexity = best.fifths.abs() + best.thirds.abs();
            if candidate_error.abs() + 0.01 < error.abs()
                || ((candidate_error.abs() - error.abs()).abs() <= 0.01
                    && complexity < best_complexity)
            {
                best = candidate;
                error = candidate_error;
            }
        }
    }
    (best, error)
}

fn circular_cents(delta: f32) -> f32 {
    (delta + 600.0).rem_euclid(1200.0) - 600.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn octave_mapping_and_negative_keys_are_stable() {
        let scale = Scale::equal(7);
        assert!((scale.frequency(67) / scale.frequency(60) - 2.0).abs() < 1e-12);
        assert!((scale.frequency(53) / scale.frequency(60) - 0.5).abs() < 1e-12);
        assert!(
            scale
                .frequencies()
                .iter()
                .all(|hz| hz.is_finite() && *hz > 0.0)
        );
    }

    #[test]
    fn degree_edit_stays_ordered() {
        let mut scale = Scale::equal(7);
        assert!(scale.move_degree(2, 500.0));
        assert!(scale.valid());
        assert!(scale.move_degree(0, 20.0));
        assert!(scale.valid());
    }

    #[test]
    fn selected_root_and_period_anchor_the_frequency_table() {
        let mut scale = Scale {
            root_degree: 4,
            root_hz: 330.0,
            ..Scale::default()
        };
        scale.set_period(1901.955);
        assert!((scale.frequency(60) - 330.0).abs() < 1e-9);
        assert!((scale.frequency(67) / scale.frequency(60) - 3.0).abs() < 0.0001);
        assert!((scale.frequency(53) / scale.frequency(60) - 1.0 / 3.0).abs() < 0.0001);
        assert!(scale.valid());
    }

    #[test]
    fn chromatic_keys_advance_one_note_in_a_24_note_period() {
        let mut scale = Scale::equal(24);
        scale.root_key = 60;
        scale.root_hz = 220.0;
        for offset in 0..24 {
            let expected = 220.0 * 2.0_f64.powf(offset as f64 / 24.0);
            assert!((scale.frequency(60 + offset as u8) - expected).abs() < 0.001);
        }
        assert!((scale.frequency(84) - 440.0).abs() < 0.001);
        assert!((scale.frequency(59) - 220.0 * 2.0_f64.powf(-1.0 / 24.0)).abs() < 0.001);
    }

    #[test]
    fn forty_eight_notes_span_one_period_in_keyboard_order() {
        let scale = Scale::equal(48);
        assert!((scale.frequency(60) - 220.0).abs() < 0.001);
        assert!(scale.frequency(61) > scale.frequency(60));
        assert!((scale.frequency(108) - 440.0).abs() < 0.001);
        assert!(scale.valid());
    }

    #[test]
    fn changing_note_count_keeps_existing_pitches_and_root() {
        let mut scale = Scale {
            root_degree: 4,
            ..Scale::default()
        };
        let original_root = scale.cents[4];
        scale.set_count(24);
        assert_eq!(scale.count, 24);
        assert_eq!(scale.cents[scale.root_degree], original_root);
        assert!(scale.valid());
        scale.set_count(7);
        assert_eq!(scale.count, 7);
        assert_eq!(scale.cents[scale.root_degree], original_root);
        assert!(scale.valid());
    }

    #[test]
    fn node_insert_and_remove_preserve_root_pitch_and_sorted_order() {
        let mut scale = Scale {
            root_degree: 4,
            ..Scale::default()
        };
        let root_cents = scale.cents[scale.root_degree];
        let root_hz = scale.frequency(scale.root_key);
        let inserted = scale.insert_degree_after(1).expect("wide interval");
        assert_eq!(inserted, 2);
        assert_eq!(scale.count, 8);
        assert_eq!(scale.root_degree, 5);
        assert_eq!(scale.cents[scale.root_degree], root_cents);
        assert_eq!(scale.frequency(scale.root_key), root_hz);
        assert!(scale.valid());

        assert!(scale.remove_degree(inserted));
        assert_eq!(scale.count, 7);
        assert_eq!(scale.root_degree, 4);
        assert_eq!(scale.cents[scale.root_degree], root_cents);
        assert_eq!(scale.frequency(scale.root_key), root_hz);
        assert!(scale.valid());
        assert!(!scale.remove_degree(scale.root_degree));
    }

    #[test]
    fn adjacent_node_insertion_respects_minimum_gap_and_degree_limit() {
        let mut scale = Scale::equal(48);
        assert!(scale.insert_degree_after(0).is_none());
        assert_eq!(scale.count, 48);

        let mut scale = Scale::equal(3);
        scale.cents[1] = 1.0;
        scale.cents[2] = 3.0;
        assert!(scale.insert_degree_after(0).is_none());
        assert!(scale.valid());
    }

    #[test]
    fn clicked_pitch_insertion_preserves_root_and_rejects_overlap() {
        let mut scale = Scale {
            root_degree: 4,
            ..Scale::default()
        };
        let root = scale.cents[scale.root_degree];
        assert_eq!(scale.insert_degree(250.0), Some(2));
        assert_eq!(scale.cents[2], 250.0);
        assert_eq!(scale.cents[scale.root_degree], root);
        assert!(scale.valid());
        for cents in [250.0, 250.5, -1.0, 1200.0, f32::NAN] {
            assert_eq!(scale.insert_degree(cents), None);
        }
        assert!(scale.remove_degree(2));
        assert_eq!(scale.count, 7);
        assert_eq!(scale.cents[scale.root_degree], root);
        assert_eq!(scale.insert_degree(1150.0), Some(7));
        assert!(scale.valid());
        assert_eq!(Scale::equal(48).insert_degree(12.0), None);
        assert!(!Scale::equal(3).remove_degree(1));
    }

    #[test]
    fn editing_node_hz_is_root_relative_and_updates_the_mts_frequency() {
        let mut scale = Scale {
            root_degree: 4,
            root_hz: 330.0,
            ..Scale::default()
        };
        assert!(scale.set_degree_frequency(5, 400.0));
        let expected_cents =
            scale.cents[scale.root_degree] + (1200.0 * (400.0_f64 / 330.0).log2()) as f32;
        assert!((scale.cents[5] - expected_cents).abs() < 0.01);
        assert!((scale.degree_frequency(5).unwrap() - 400.0).abs() < 0.01);
        assert!((scale.frequency(61) - 400.0).abs() < 0.01);

        assert!(scale.set_degree_frequency(4, 440.0));
        assert_eq!(scale.root_hz, 440.0);
        assert!(scale.valid());
    }

    #[test]
    fn degree_hz_fields_allow_valid_frequencies_outside_root_range() {
        let mut low_root = Scale {
            root_hz: 10.0,
            root_degree: 6,
            period_cents: 2400.0,
            ..Scale::default()
        };
        low_root.cents[..7].copy_from_slice(&[0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 2399.0]);
        assert!((low_root.degree_frequency(0).unwrap() - 2.500_7).abs() < 0.01);
        assert!(low_root.set_degree_frequency(0, 2.6));
        assert!((low_root.degree_frequency(0).unwrap() - 2.6).abs() < 0.01);

        let mut high_root = Scale {
            root_hz: 20_000.0,
            root_degree: 0,
            period_cents: 2400.0,
            ..Scale::default()
        };
        high_root.cents[..7].copy_from_slice(&[0.0, 100.0, 200.0, 300.0, 400.0, 500.0, 2399.0]);
        assert!(high_root.set_degree_frequency(6, 79_950.0));
        assert!((high_root.degree_frequency(6).unwrap() - 79_950.0).abs() < 0.1);
        assert!(high_root.valid());
    }

    #[test]
    fn lattice_finds_fifth() {
        let (point, error) = nearest_lattice(700.0);
        assert_eq!((point.fifths, point.thirds), (1, 0));
        assert_eq!(point.ratio(), (3, 2));
        assert!((error + 1.955).abs() < 0.01);
    }
}
