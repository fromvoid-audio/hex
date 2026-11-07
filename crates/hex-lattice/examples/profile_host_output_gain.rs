//! Isolated canonical host-output-stage measurements, excluding preparation,
//! allocation, source I/O and physical output. Compare the same file on both
//! source revisions; this is a benchmark fixture, not native acceptance.
use hex_dsp::StereoMatrix;
use hex_lattice::{
    NativeRuntimeChain, NativeTransientInput, NativeTransientOperation, NativeTransientProcessing,
    NativeTransientValue, PreparedProcessorInstance, PreparedRuntimeInstance, ProcessorChainOwner,
    ProcessorInstanceIdentity, RefuseForeign,
};
use hex_lattice_contract::{
    DeviceAutomationTarget::HostOutputGain, DeviceTypeId, HostGain, NativeDeviceState,
};
use std::{error::Error, hint::black_box, sync::Arc, time::Instant};
fn chain(db: f32) -> Result<NativeRuntimeChain, Box<dyn Error>> {
    chain_count(db, 1)
}
fn chain_count(db: f32, count: usize) -> Result<NativeRuntimeChain, Box<dyn Error>> {
    NativeRuntimeChain::new(instances(db, count)?).map_err(Into::into)
}
fn instances(db: f32, count: usize) -> Result<Vec<PreparedProcessorInstance>, Box<dyn Error>> {
    let kind = DeviceTypeId::from_static("test.host-gain-profile");
    let state = Arc::new(NativeDeviceState::new(kind, 1, vec![1])?.with_output_gain_db(db)?);
    let mut instances = Vec::new();
    instances.try_reserve_exact(count)?;
    for i in 0..count {
        instances.push(PreparedProcessorInstance {
            meter: Default::default(),
            owner: ProcessorChainOwner::Track(1),
            device_id: u64::try_from(i + 1)?,
            device_type: kind,
            matrix: StereoMatrix::identity(),
            bypassed: false,
            sample_rate: 48000,
            configuration: Some(Arc::clone(&state)),
            definition: None,
        });
    }
    Ok(instances)
}
fn profile_transient(block: usize) -> Result<(), Box<dyn Error>> {
    let mut instance =
        PreparedRuntimeInstance::from(instances(0.0, 1)?.pop().ok_or("missing fixture")?);
    instance.automation.arm_host_gain_input(HostOutputGain)?;
    let mut current = NativeRuntimeChain::new_prepared(vec![instance])?;
    let identity = ProcessorInstanceIdentity::new(ProcessorChainOwner::Track(1), 1)?;
    let gain = HostGain::from_db(-6.0)?;
    let mut pcm = vec![[0.25, -0.125]; block];
    let mut samples = Vec::with_capacity(4096);
    for i in 0..5120 {
        pcm.fill(black_box([0.25, -0.125]));
        let operation = if i % 2 == 0 {
            NativeTransientOperation::Set(NativeTransientValue::HostOutputGain(gain))
        } else {
            NativeTransientOperation::Clear(HostOutputGain)
        };
        let mut context = NativeTransientProcessing::new(NativeTransientInput {
            identity,
            operation,
        });
        let start = Instant::now();
        current.begin_block();
        current.process_range_tapped_with_foreign_and_input(
            0..1,
            black_box(&mut pcm),
            0,
            None,
            None,
            &mut RefuseForeign,
            Some(black_box(&mut context)),
            &mut |_, _, _, _, _, _| {},
        )?;
        current.commit_block();
        let ns = start.elapsed().as_nanos();
        black_box((&pcm, context.applied));
        if i >= 1024 {
            samples.push(ns);
        }
    }
    samples.sort_unstable();
    let mean = samples.iter().sum::<u128>() / samples.len() as u128;
    println!(
        "mode=transient-set-clear block={block} samples={} mean_ns={mean} p50_ns={} p99_ns={}",
        samples.len(),
        samples.get(2048).ok_or("p50")?,
        samples.get(4055).ok_or("p99")?
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    for block in [64, 128, 512] {
        profile_transient(block)?;
        for mode in ["steady", "settled", "active"] {
            let old = chain(0.0)?;
            let mut current = chain(-6.0)?;
            if mode != "steady" {
                current.inherit_from(&old);
            }
            let mut pcm = vec![[0.25, -0.125]; block];
            if mode != "active" {
                for _ in 0..1024 {
                    current.begin_block();
                    current.process_block(&mut pcm)?;
                    current.commit_block();
                    pcm.fill([0.25, -0.125]);
                }
            }
            let mut samples = Vec::with_capacity(4096);
            for i in 0..5120 {
                if mode == "active" {
                    current = chain(-6.0)?;
                    current.inherit_from(&old);
                }
                pcm.fill(black_box([0.25, -0.125]));
                let start = Instant::now();
                current.begin_block();
                current.process_block(black_box(&mut pcm))?;
                current.commit_block();
                let ns = start.elapsed().as_nanos();
                black_box(&pcm);
                if i >= 1024 {
                    samples.push(ns);
                }
            }
            samples.sort_unstable();
            let mean = samples.iter().sum::<u128>() / samples.len() as u128;
            println!(
                "mode={mode} block={block} samples={} mean_ns={mean} p50_ns={} p99_ns={} max_ns={} accounted_bytes={:?}",
                samples.len(),
                samples.get(2048).ok_or("p50")?,
                samples.get(4055).ok_or("p99")?,
                samples.last().ok_or("max")?,
                current.accounted_storage_bytes()
            );
        }
    }
    for count in [1, 16, 64, 256] {
        let old = chain_count(0.0, count)?;
        let mut samples = Vec::with_capacity(4096);
        for i in 0..5120 {
            let mut candidate = chain_count(-6.0, count)?;
            let start = Instant::now();
            candidate.inherit_from(black_box(&old));
            let ns = start.elapsed().as_nanos();
            black_box(&candidate);
            if i >= 1024 {
                samples.push(ns);
            }
        }
        samples.sort_unstable();
        let mean = samples.iter().sum::<u128>() / samples.len() as u128;
        println!(
            "mode=handoff entries={count} samples={} mean_ns={mean} p50_ns={} p99_ns={} max_ns={}",
            samples.len(),
            samples.get(2048).ok_or("p50")?,
            samples.get(4055).ok_or("p99")?,
            samples.last().ok_or("max")?
        );
    }
    Ok(())
}
