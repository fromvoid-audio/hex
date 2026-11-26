use super::*;

/// Control-prepared Utility coefficients. Processing is allocation-free and has
/// no runtime state, lookahead, filtering, or buffering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreparedUtility {
    pub(super) settings: UtilitySettings,
    pub(super) matrix: StereoMatrix,
}

impl PreparedUtility {
    pub fn new(settings: UtilitySettings) -> Result<Self, UtilityError> {
        let mut matrix = pre_gain_matrix(settings)?;
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

pub(super) fn pre_gain_matrix(settings: UtilitySettings) -> Result<StereoMatrix, UtilityError> {
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
    Ok(matrix)
}

pub(super) fn polarity_matrix(settings: UtilitySettings) -> Result<StereoMatrix, UtilityError> {
    matrix_from_f64([
        [if settings.invert_left { -1.0 } else { 1.0 }, 0.0],
        [0.0, if settings.invert_right { -1.0 } else { 1.0 }],
    ])
}

pub(super) fn channel_matrix(mode: ChannelMode) -> Result<StereoMatrix, UtilityError> {
    let values = match mode {
        ChannelMode::Stereo => [[1.0, 0.0], [0.0, 1.0]],
        ChannelMode::Left => [[1.0, 0.0], [1.0, 0.0]],
        ChannelMode::Right => [[0.0, 1.0], [0.0, 1.0]],
        ChannelMode::Swap => [[0.0, 1.0], [1.0, 0.0]],
    };
    matrix_from_f64(values)
}

pub(super) fn width_matrix(width: f32) -> Result<StereoMatrix, UtilityError> {
    let width = f64::from(width);
    matrix_from_f64([
        [(1.0 + width) * 0.5, (1.0 - width) * 0.5],
        [(1.0 - width) * 0.5, (1.0 + width) * 0.5],
    ])
}

/// Original M/S control law: -1 keeps only mid, 0 is neutral, +1 keeps only side.
pub(super) fn mid_side_balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
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
pub(super) fn balance_matrix(balance: f32) -> Result<StereoMatrix, UtilityError> {
    let balance = f64::from(balance);
    let left = if balance > 0.0 { 1.0 - balance } else { 1.0 };
    let right = if balance < 0.0 { 1.0 + balance } else { 1.0 };
    matrix_from_f64([[left, 0.0], [0.0, right]])
}

pub(super) fn scalar_matrix(gain: f32) -> Result<StereoMatrix, UtilityError> {
    let gain = f64::from(gain);
    matrix_from_f64([[gain, 0.0], [0.0, gain]])
}

pub(super) fn matrix_from_f64(values: [[f64; 2]; 2]) -> Result<StereoMatrix, UtilityError> {
    let mut coefficients = [[0.0_f32; 2]; 2];
    for row in 0..2 {
        for column in 0..2 {
            let value = values[row][column];
            if !value.is_finite() || value.abs() > f64::from(f32::MAX) {
                return Err(UtilityError("mix coefficient is not representable"));
            }
            coefficients[row][column] = value as f32;
        }
    }
    StereoMatrix::new(coefficients).map_err(Into::into)
}
