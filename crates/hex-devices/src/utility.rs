//! Original, immutable, zero-latency stereo Utility processing.
use hex_lattice::{InvalidMatrix, StereoFrame, StereoMatrix};
use std::fmt;

/// Maximum admitted linear Utility gain, equivalent to +35 dB.
pub const MAX_UTILITY_GAIN: f32 = 56.234_13;

/// Channel selection applied after input polarity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChannelMode {
    Stereo,
    Left,
    Right,
    Swap,
}

/// Invalid or unrepresentable Utility settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UtilityError(pub &'static str);

impl fmt::Display for UtilityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for UtilityError {}

impl From<InvalidMatrix> for UtilityError {
    fn from(_: InvalidMatrix) -> Self {
        Self("utility coefficient preparation overflowed")
    }
}

/// Validated control-side parameters for a memoryless stereo Utility processor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilitySettings {
    gain: f32,
    balance: f32,
    width: f32,
    channel_mode: ChannelMode,
    invert_left: bool,
    invert_right: bool,
    mono: bool,
    mute: bool,
    mid_side_balance: Option<f32>,
}

impl UtilitySettings {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        gain: f32,
        balance: f32,
        width: f32,
        channel_mode: ChannelMode,
        invert_left: bool,
        invert_right: bool,
        mono: bool,
        mute: bool,
        mid_side_balance: Option<f32>,
    ) -> Result<Self, UtilityError> {
        if !gain.is_finite() || !(0.0..=MAX_UTILITY_GAIN).contains(&gain) {
            return Err(UtilityError(
                "utility gain must be finite and between 0 and +35 dB",
            ));
        }
        if !balance.is_finite() || !(-1.0..=1.0).contains(&balance) {
            return Err(UtilityError(
                "utility balance must be finite and between -1 and 1",
            ));
        }
        if !width.is_finite() || !(0.0..=4.0).contains(&width) {
            return Err(UtilityError(
                "utility width must be finite and between 0 and 4",
            ));
        }
        if mid_side_balance
            .is_some_and(|value| !value.is_finite() || !(-1.0..=1.0).contains(&value))
        {
            return Err(UtilityError(
                "utility mid-side balance must be finite and between -1 and 1",
            ));
        }
        Ok(Self {
            gain,
            balance,
            width,
            channel_mode,
            invert_left,
            invert_right,
            mono,
            mute,
            mid_side_balance,
        })
    }

    pub const fn gain(self) -> f32 {
        self.gain
    }

    pub const fn balance(self) -> f32 {
        self.balance
    }

    pub const fn width(self) -> f32 {
        self.width
    }

    pub const fn channel_mode(self) -> ChannelMode {
        self.channel_mode
    }

    pub const fn mid_side_balance(self) -> Option<f32> {
        self.mid_side_balance
    }

    pub const fn invert_left(self) -> bool {
        self.invert_left
    }

    pub const fn invert_right(self) -> bool {
        self.invert_right
    }

    pub const fn mono(self) -> bool {
        self.mono
    }

    pub const fn mute(self) -> bool {
        self.mute
    }
}

impl Default for UtilitySettings {
    fn default() -> Self {
        Self {
            gain: 1.0,
            balance: 0.0,
            width: 1.0,
            channel_mode: ChannelMode::Stereo,
            invert_left: false,
            invert_right: false,
            mono: false,
            mute: false,
            mid_side_balance: None,
        }
    }
}

/// Control-prepared Utility coefficients. Processing is allocation-free and has
/// no runtime state, lookahead, filtering, or buffering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedUtility {
    settings: UtilitySettings,
    matrix: StereoMatrix,
}

impl PreparedUtility {
    pub fn new(settings: UtilitySettings) -> Result<Self, UtilityError> {
        let mut matrix = StereoMatrix::identity();
        matrix = matrix.then(polarity_matrix(settings)?)?;
        matrix = matrix.then(channel_matrix(settings.channel_mode)?)?;

        // Left/Right monitoring duplicates one input channel; width and M/S
        // controls have no meaningful independent signal there and are bypassed.
        if !matches!(
            settings.channel_mode,
            ChannelMode::Left | ChannelMode::Right
        ) {
            if let Some(balance) = settings.mid_side_balance {
                matrix = matrix.then(mid_side_balance_matrix(balance)?)?;
            } else {
                matrix = matrix.then(width_matrix(settings.width)?)?;
            }
        }
        if settings.mono {
            matrix = matrix.then(matrix_from_f64([[0.5, 0.5], [0.5, 0.5]])?)?;
        }
        matrix = matrix.then(scalar_matrix(settings.gain)?)?;
        matrix = matrix.then(balance_matrix(settings.balance)?)?;
        if settings.mute {
            matrix = matrix.then(matrix_from_f64([[0.0, 0.0], [0.0, 0.0]])?)?;
        }
        Ok(Self { settings, matrix })
    }

    pub const fn settings(self) -> UtilitySettings {
        self.settings
    }

    pub const fn matrix(self) -> StereoMatrix {
        self.matrix
    }

    pub const fn latency_frames(self) -> u32 {
        0
    }

    pub fn process_frame(self, input: StereoFrame) -> StereoFrame {
        self.matrix.apply(input)
    }

    /// Process a caller-owned block without allocation or hidden limiting.
    pub fn process_block(self, frames: &mut [StereoFrame]) {
        self.matrix.apply_block(frames);
    }
}

fn polarity_matrix(settings: UtilitySettings) -> Result<StereoMatrix, UtilityError> {
    matrix_from_f64([
        [if settings.invert_left { -1.0 } else { 1.0 }, 0.0],
        [0.0, if settings.invert_right { -1.0 } else { 1.0 }],
    ])
}

fn channel_matrix(mode: ChannelMode) -> Result<StereoMatrix, UtilityError> {
    let values = match mode {
        ChannelMode::Stereo => [[1.0, 0.0], [0.0, 1.0]],
        ChannelMode::Left => [[1.0, 0.0], [1.0, 0.0]],
        ChannelMode::Right => [[0.0, 1.0], [0.0, 1.0]],
        ChannelMode::Swap => [[0.0, 1.0], [1.0, 0.0]],
    };
    matrix_from_f64(values)
}

fn width_matrix(width: f32) -> Result<StereoMatrix, UtilityError> {
    let width = f64::from(width);
    matrix_from_f64([
        [(1.0 + width) * 0.5, (1.0 - width) * 0.5],
        [(1.0 - width) * 0.5, (1.0 + width) * 0.5],
    ])
}

/// Original M/S control law: -1 keeps only mid, 0 is neutral, +1 keeps only side.
fn mid_side_balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
    let balance = f64::from(balance);
    let (mid, side) = if balance < 0.0 {
        (1.0, 1.0 + balance)
    } else {
        (1.0 - balance, 1.0)
    };
    matrix_from_f64([
        [(mid + side) * 0.5, (mid - side) * 0.5],
        [(mid - side) * 0.5, (mid + side) * 0.5],
    ])
}

/// Center keeps both channels at unity; extremes attenuate one side to zero.
fn balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
    let balance = f64::from(balance);
    let left = if balance > 0.0 { 1.0 - balance } else { 1.0 };
    let right = if balance < 0.0 { 1.0 + balance } else { 1.0 };
    matrix_from_f64([[left, 0.0], [0.0, right]])
}

fn scalar_matrix(gain: f32) -> Result<StereoMatrix, UtilityError> {
    let gain = f64::from(gain);
    matrix_from_f64([[gain, 0.0], [0.0, gain]])
}

fn matrix_from_f64(values: [[f64; 2]; 2]) -> Result<StereoMatrix, UtilityError> {
    let mut coefficients = [[0.0_f32; 2]; 2];
    for row in 0..2 {
        for column in 0..2 {
            let value = values[row][column];
            if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                return Err(UtilityError("utility coefficient is not representable"));
            }
            coefficients[row][column] = value as f32;
        }
    }
    StereoMatrix::new(coefficients).map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[allow(clippy::too_many_arguments)]
    fn settings(
        gain: f32,
        balance: f32,
        width: f32,
        mode: ChannelMode,
        invert_left: bool,
        invert_right: bool,
        mono: bool,
        mute: bool,
        mid_side_balance: Option<f32>,
    ) -> UtilitySettings {
        UtilitySettings::new(
            gain,
            balance,
            width,
            mode,
            invert_left,
            invert_right,
            mono,
            mute,
            mid_side_balance,
        )
        .unwrap()
    }

    fn close(actual: f32, expected: f32) {
        assert!(
            (actual - expected).abs() < 0.000_01,
            "{actual} != {expected}"
        );
    }

    #[test]
    fn default_is_exact_identity_and_declares_zero_latency() {
        let utility = PreparedUtility::new(UtilitySettings::default()).unwrap();
        let input = [f32::from_bits(0x8000_0000), 0.75];
        let output = utility.process_frame(input);
        assert_eq!(output[0].to_bits(), input[0].to_bits());
        assert_eq!(output[1].to_bits(), input[1].to_bits());
        assert_eq!(utility.latency_frames(), 0);
    }

    #[test]
    fn channel_modes_polarity_and_stereo_width_have_defined_matrices() {
        let frame = [0.4, -0.2];
        let left = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Left,
            false,
            false,
            false,
            false,
            Some(1.0),
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(left, [0.4, 0.4]);
        let right = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Right,
            false,
            true,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(right, [0.2, 0.2]);
        let swapped_and_inverted = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Swap,
            true,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(swapped_and_inverted, [-0.2, -0.4]);
        let mono_width = PreparedUtility::new(settings(
            1.0,
            0.0,
            0.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        assert_eq!(mono_width, [0.1, 0.1]);
        let wide = PreparedUtility::new(settings(
            1.0,
            0.0,
            2.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap()
        .process_frame(frame);
        close(wide[0], 0.7);
        close(wide[1], -0.5);
    }

    #[test]
    fn mid_side_balance_endpoints_and_mono_are_explicit() {
        let input = [0.8, 0.2];
        let mid_only = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            Some(-1.0),
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(mid_only, [0.5, 0.5]);
        let side_only = PreparedUtility::new(settings(
            1.0,
            0.0,
            3.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            Some(1.0),
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(side_only, [0.3, -0.3]);
        let mono = PreparedUtility::new(settings(
            1.0,
            0.0,
            2.0,
            ChannelMode::Stereo,
            false,
            false,
            true,
            false,
            None,
        ))
        .unwrap()
        .process_frame(input);
        assert_eq!(mono, [0.5, 0.5]);
    }

    #[test]
    fn output_balance_gain_zero_mute_and_block_processing_are_defined() {
        let right_only = PreparedUtility::new(settings(
            2.0,
            1.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap();
        assert_eq!(right_only.process_frame([0.4, 0.3]), [0.0, 0.6]);
        let silent = PreparedUtility::new(settings(
            0.0,
            0.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            false,
            None,
        ))
        .unwrap();
        assert_eq!(silent.process_frame([1.0, -1.0]), [0.0, 0.0]);
        let muted = PreparedUtility::new(settings(
            1.0,
            0.0,
            1.0,
            ChannelMode::Stereo,
            false,
            false,
            false,
            true,
            None,
        ))
        .unwrap();
        assert_eq!(muted.process_frame([1.0, -1.0]), [0.0, 0.0]);

        let mut block = [[0.2, 0.4], [-0.6, 0.8], [0.0, 0.1]];
        let expected = block.map(|frame| right_only.process_frame(frame));
        right_only.process_block(&mut block);
        assert_eq!(block, expected);
        assert!(
            PreparedUtility::new(settings(
                MAX_UTILITY_GAIN,
                -1.0,
                4.0,
                ChannelMode::Stereo,
                true,
                true,
                false,
                false,
                None,
            ))
            .is_ok()
        );
    }

    #[test]
    fn invalid_utility_settings_are_rejected() {
        let invalid = [
            (f32::NAN, 0.0, 1.0, None),
            (f32::INFINITY, 0.0, 1.0, None),
            (-0.1, 0.0, 1.0, None),
            (MAX_UTILITY_GAIN + 1.0, 0.0, 1.0, None),
            (1.0, f32::NAN, 1.0, None),
            (1.0, 1.1, 1.0, None),
            (1.0, 0.0, f32::INFINITY, None),
            (1.0, 0.0, -0.1, None),
            (1.0, 0.0, 4.1, None),
            (1.0, 0.0, 1.0, Some(f32::NAN)),
            (1.0, 0.0, 1.0, Some(-1.1)),
            (1.0, 0.0, 1.0, Some(1.1)),
        ];
        for (gain, balance, width, ms) in invalid {
            assert!(
                UtilitySettings::new(
                    gain,
                    balance,
                    width,
                    ChannelMode::Stereo,
                    false,
                    false,
                    false,
                    false,
                    ms,
                )
                .is_err()
            );
        }
    }
}
