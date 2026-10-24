//! Control-side queries of the actual prepared EQ API; no reference/parity claim.
use hex_devices::eq8::{EQ8_DEFINITION, parameter};
use hex_lattice::{NativeDeviceDefinition, PreparedNativeProcessor};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use std::{error::Error, fmt::Write, process::Command};

const MODES: [&str; 3] = ["stereo", "lr", "ms"];
const BANKS: [&str; 5] = ["stereo", "left", "right", "mid", "side"];
const SHAPES: [&str; 8] = [
    "bell",
    "low_shelf",
    "high_shelf",
    "low_cut_12",
    "low_cut_48",
    "high_cut_12",
    "high_cut_48",
    "notch",
];
const USAGE: &str = "eq8_response RATE MODE BANK SHAPE FREQUENCY GAIN Q OUTPUT_GAIN PROBE_HZ... (1..8192 probes; adaptive Q unsupported)";

type Result<T> = std::result::Result<T, Box<dyn Error>>;
struct Query {
    rate: u32,
    mode: usize,
    bank: usize,
    shape: usize,
    values: [f32; 4],
    probes: Vec<f32>,
}
fn choice(value: &str, choices: &[&str]) -> Result<usize> {
    choices
        .iter()
        .position(|v| *v == value)
        .ok_or_else(|| USAGE.into())
}
fn finite(value: &str) -> Result<f32> {
    let v: f32 = value.parse()?;
    if !v.is_finite() {
        return Err("nonfinite input".into());
    }
    Ok(v)
}
impl Query {
    fn parse(args: &[String]) -> Result<Self> {
        if !(9..=8200).contains(&args.len()) {
            return Err(USAGE.into());
        }
        let rate: u32 = args[0].parse()?;
        if !(8000..=192000).contains(&rate) {
            return Err("rate outside current EQ contract".into());
        }
        let mode = choice(&args[1], &MODES)?;
        let bank = choice(&args[2], &BANKS)?;
        if !matches!((mode, bank), (0, 0) | (1, 1 | 2) | (2, 3 | 4)) {
            return Err("bank is not selected by this mode".into());
        }
        let shape = choice(&args[3], &SHAPES)?;
        let values = [
            finite(&args[4])?,
            finite(&args[5])?,
            finite(&args[6])?,
            finite(&args[7])?,
        ];
        let probes = args[8..]
            .iter()
            .map(|s| finite(s))
            .collect::<Result<Vec<_>>>()?;
        if probes
            .iter()
            .any(|hz| *hz <= 0. || *hz >= rate as f32 * 0.5)
            || probes.windows(2).any(|p| p[0] >= p[1])
        {
            return Err("probes must be strictly increasing, positive and below Nyquist".into());
        }
        Ok(Self {
            rate,
            mode,
            bank,
            shape,
            values,
            probes,
        })
    }
    fn prepared(&self) -> Result<Box<dyn PreparedNativeProcessor>> {
        let mut state = EQ8_DEFINITION.default_state()?;
        for (id, value) in [
            (parameter::MODE, ParameterValue::Choice(self.mode as u32)),
            (parameter::enabled(self.bank, 0), ParameterValue::Bool(true)),
            (
                parameter::kind(self.bank, 0),
                ParameterValue::Choice(self.shape as u32),
            ),
            (
                parameter::frequency(self.bank, 0),
                ParameterValue::Float(self.values[0]),
            ),
            (
                parameter::gain(self.bank, 0),
                ParameterValue::Float(self.values[1]),
            ),
            (
                parameter::q(self.bank, 0),
                ParameterValue::Float(self.values[2]),
            ),
            (
                parameter::OUTPUT_GAIN,
                ParameterValue::Float(self.values[3]),
            ),
        ] {
            state = EQ8_DEFINITION.patch_parameter(&state, id, value)?;
        }
        EQ8_DEFINITION
            .prepare_processor(
                &state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: self.rate,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )?
            .ok_or_else(|| "missing prepared EQ".into())
    }
    fn export(&self, source: &str, dirty: bool) -> Result<String> {
        let prepared = self.prepared()?;
        let mut out = format!(
            "# {{\"schema\":1,\"producer\":\"hex.eq8 prepared response\",\"state_version\":{},\"source_sha\":\"{}\",\"dirty\":{},\"case\":{{\"sample_rate\":{},\"mode\":\"{}\",\"bank\":\"{}\",\"shape\":\"{}\",\"frequency_hz\":{:?},\"gain_db\":{:?},\"q\":{:?},\"output_gain\":{:?},\"adaptive_q\":false}}}}\nquery_hz,lane0_linear,lane1_linear\n",
            EQ8_DEFINITION.state_version(),
            source,
            dirty,
            self.rate,
            MODES[self.mode],
            BANKS[self.bank],
            SHAPES[self.shape],
            self.values[0],
            self.values[1],
            self.values[2],
            self.values[3]
        );
        for hz in &self.probes {
            let response = prepared
                .frequency_response_at_hz(*hz)
                .ok_or("provider declined frequency")?;
            if response.iter().any(|v| !v.is_finite() || *v < 0.) {
                return Err("invalid provider magnitude".into());
            }
            writeln!(&mut out, "{},{},{}", hz, response[0], response[1])?;
        }
        Ok(out)
    }
}
fn git(args: &[&str]) -> Result<String> {
    // Attribute data to this helper's source checkout, even when invoked from
    // another repository. This is source metadata, not binary attestation.
    let output = Command::new("git")
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .args(args)
        .output()?;
    if !output.status.success() {
        return Err("source provenance unavailable; run inside canonical checkout".into());
    }
    Ok(String::from_utf8(output.stdout)?.trim().to_owned())
}
fn main() -> Result<()> {
    let q = Query::parse(&std::env::args().skip(1).collect::<Vec<_>>())?;
    let source = git(&["rev-parse", "HEAD"])?;
    if source.len() != 40 || !source.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("invalid source identity".into());
    }
    let dirty = !git(&["status", "--porcelain", "--untracked-files=normal"])?.is_empty();
    print!("{}", q.export(&source, dirty)?);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    fn args() -> Vec<String> {
        "48000 stereo stereo bell 1000 6 0.70710677 1 100 1000 10000"
            .split_whitespace()
            .map(str::to_owned)
            .collect()
    }
    #[test]
    fn actual_provider_peak_and_inactive_lane_are_exported() {
        let q = Query::parse(&args()).unwrap();
        let p = q.prepared().unwrap();
        assert!(
            (p.frequency_response_at_hz(1000.).unwrap()[0] - 10_f32.powf(6. / 20.)).abs() < 0.00001
        );
        let mut a = args();
        a[1] = "lr".into();
        a[2] = "left".into();
        let lr = Query::parse(&a).unwrap().prepared().unwrap();
        assert_eq!(lr.frequency_response_at_hz(1000.).unwrap()[1], 1.);
        assert!(
            q.export(&"a".repeat(40), false)
                .unwrap()
                .contains("\"adaptive_q\":false")
        );
    }
    #[test]
    fn exported_configuration_retains_signed_zero() {
        let mut a = args();
        a[5] = "-0".into();
        let q = Query::parse(&a).unwrap();
        assert!(
            q.export(&"a".repeat(40), false)
                .unwrap()
                .contains("\"gain_db\":-0.0")
        );
    }
    #[test]
    fn all_shapes_selected_banks_and_boundary_rates_use_the_provider() {
        for rate in [8000, 44100, 192000] {
            for (mode, bank) in [
                ("stereo", "stereo"),
                ("lr", "left"),
                ("lr", "right"),
                ("ms", "mid"),
                ("ms", "side"),
            ] {
                for shape in SHAPES {
                    let a = [
                        rate.to_string(),
                        mode.into(),
                        bank.into(),
                        shape.into(),
                        "1000".into(),
                        "6".into(),
                        "0.70710677".into(),
                        "1".into(),
                        "100".into(),
                        "1000".into(),
                    ];
                    let q = Query::parse(&a).unwrap();
                    assert!(q.export(&"a".repeat(40), false).unwrap().lines().count() == 4);
                }
            }
        }
    }
    #[test]
    fn malformed_queries_refuse_before_export() {
        for (index, bad) in [
            (0, "384000"),
            (2, "right"),
            (3, "unknown"),
            (4, "NaN"),
            (8, "24000"),
            (9, "100"),
        ] {
            let mut a = args();
            a[index] = bad.into();
            assert!(Query::parse(&a).is_err());
        }
        let mut a = args();
        a[6] = "0".into();
        assert!(Query::parse(&a).unwrap().prepared().is_err());
    }
}
