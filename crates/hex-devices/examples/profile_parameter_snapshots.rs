//! Control-only registry snapshot profile; no UI, audio callback or native deadline claim.
use hex_lattice_contract::{DeviceParameterSnapshot, DeviceTypeId, ParameterId, ParameterValue};
use std::{hint::black_box, time::Instant};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let registry = hex_devices::native_instrument_registry()?;
    let source = registry.default_state(DeviceTypeId::from_static("hex.instrument.operator"))?;
    let definition = registry
        .definition(source.type_id())
        .ok_or("operator missing")?;
    let values: Vec<_> = (93..157)
        .map(|id| {
            let value = match id {
                93 => 1.0,
                94 => 0.8,
                _ => 0.0,
            };
            (ParameterId::from_static(id), ParameterValue::Float(value))
        })
        .collect();
    let snapshot = DeviceParameterSnapshot::new(&values)?;
    let mut timings = Vec::with_capacity(16);
    for iteration in 0..20 {
        let started = Instant::now();
        let candidate =
            black_box(registry.patch_parameters(black_box(&source), black_box(&snapshot))?);
        let elapsed = started.elapsed().as_nanos();
        // Verification, caller destruction and source setup are outside timing.
        for &(id, expected) in snapshot.entries() {
            if definition.read_parameter(&candidate, id)? != expected {
                return Err("snapshot output differs from requested values".into());
            }
        }
        if iteration >= 4 {
            timings.push(elapsed);
        }
    }
    timings.sort_unstable();
    let p50 = timings
        .get(timings.len() / 2)
        .ok_or("profile has no samples")?;
    let p95 = timings.last().ok_or("profile has no samples")?;
    println!("profile,parameters,samples,p50_ns,p95_ns,max_ns");
    println!(
        "operator_harmonic_snapshot,{},{},{p50},{p95},{p95}",
        values.len(),
        timings.len()
    );
    Ok(())
}
