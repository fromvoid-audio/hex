//! Track/return sample-peak display units; fader values and audio are untouched.
pub(super) const FLOOR_AMPLITUDE: f32 = 0.000_316_227_76;
const FLOOR_DB: f64 = -70.0;
const CEILING_DB: f64 = 6.0;
const REFERENCE_HEIGHT: f64 = 1080.0;
const UNITY_HEIGHT: f64 = 969.0;
pub(super) const UNITY_FRACTION: f32 = (UNITY_HEIGHT / REFERENCE_HEIGHT) as f32;

// Heights above the bottom of the expanded meter in Ableton's Live 12 manual
// (Mixing, "Additional Mixer Features", ProSessionMixerSizes.png). The image
// shows labeled ticks down to -60 dB. Keep Hex's endpoints and use an exact
// midpoint at -24 dB, within one pixel of the reference tick. Interpolation
// matches the visible scale without assuming Live's internal meter law.
const TICKS: [(f64, f64); 13] = [
    (FLOOR_DB, 0.0),
    (-60.0, 61.0),
    (-54.0, 121.0),
    (-48.0, 185.0),
    (-42.0, 257.0),
    (-36.0, 337.0),
    (-30.0, 433.0),
    (-24.0, REFERENCE_HEIGHT / 2.0),
    (-18.0, 647.0),
    (-12.0, 755.0),
    (-6.0, 863.0),
    (0.0, UNITY_HEIGHT),
    (CEILING_DB, REFERENCE_HEIGHT),
];

pub(super) fn fraction(amplitude: f32) -> f32 {
    if !amplitude.is_finite() || amplitude <= FLOOR_AMPLITUDE {
        return 0.0;
    }
    let db = 20.0 * f64::from(amplitude).log10();
    for (&(lower_db, lower_height), &(upper_db, upper_height)) in TICKS.iter().zip(&TICKS[1..]) {
        if db <= upper_db {
            let step = ((db - lower_db) / (upper_db - lower_db)).max(0.0);
            return ((lower_height + (upper_height - lower_height) * step) / REFERENCE_HEIGHT)
                as f32;
        }
    }
    1.0
}

pub(super) fn readout(amplitude: f32) -> String {
    if !amplitude.is_finite() || amplitude < 0.0 {
        return "invalid".into();
    }
    if amplitude == 0.0 {
        return "−∞ dbfs".into();
    }
    if amplitude < FLOOR_AMPLITUDE {
        return "<−70 dbfs".into();
    }
    let db = if amplitude == FLOOR_AMPLITUDE {
        FLOOR_DB
    } else {
        20.0 * f64::from(amplitude).log10()
    };
    if db.abs() < 0.05 {
        "0.0 dbfs".into()
    } else if db > 0.0 {
        format!("+{db:.1} dbfs")
    } else {
        format!("{db:.1} dbfs")
    }
}
