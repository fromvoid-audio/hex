//! CPU comparison of the same DSP with its previous host bypass capability
//! disabled versus reserved alignment. This is a microbenchmark, not a deadline.
use super::*;
use hex_lattice::{NativeRuntimeChain, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix};
use std::{hint::black_box, time::Instant};

struct PreviousCapability(Arc<dyn PreparedOwnedProcessorRuntime>);
impl PreparedOwnedProcessorRuntime for PreviousCapability {
    fn runtime_version(&self) -> u32 {
        self.0.runtime_version()
    }
    fn declared_pair_bytes(&self) -> usize {
        self.0.declared_pair_bytes()
    }
    fn create_runtime(&self) -> Result<Box<dyn OwnedProcessorRuntime>, DeviceContractError> {
        self.0.create_runtime()
    }
    fn output_tail(&self) -> hex_lattice::OutputTail {
        self.0.output_tail()
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.0.algorithmic_latency_frames()
    }
}

#[test]
#[ignore = "opt-in optimized CPU profile; no physical output deadline acceptance"]
fn reserved_bypass_cpu_comparison() {
    println!("optimized={}", !cfg!(debug_assertions));
    for (label, changes) in [
        ("standard", vec![(GAIN, ParameterValue::Float(12.0))]),
        (
            "true-peak-mid-side",
            vec![
                (GAIN, ParameterValue::Float(12.0)),
                (MODE, ParameterValue::Choice(2)),
                (ROUTING, ParameterValue::Choice(1)),
                (LOOKAHEAD, ParameterValue::Choice(2)),
            ],
        ),
    ] {
        for aligned in [false, true] {
            let provider = factory(48_000, &changes);
            let provider = if aligned {
                provider
            } else {
                Arc::new(PreviousCapability(provider))
            };
            let state = state(&changes);
            let mut chain = NativeRuntimeChain::new_prepared(vec![PreparedRuntimeInstance {
                modulation: None,
                automation: Default::default(),
                meter: Default::default(),
                owner: ProcessorChainOwner::Main,
                device_id: 1,
                device_type: state.type_id(),
                matrix: StereoMatrix::identity(),
                bypassed: true,
                sample_rate: 48_000,
                configuration: Some(Arc::new(state)),
                definition: Some(PreparedProcessorFactory::Owned(provider)),
            }])
            .unwrap();
            let input: [StereoFrame; 128] = std::array::from_fn(|i| {
                let phase = i as f32 * std::f32::consts::TAU / 128.0;
                [(phase * 3.0).sin() * 0.6, (phase * 5.0).cos() * -0.4]
            });
            let mut times = Vec::with_capacity(2048);
            for iteration in 0..2560 {
                let mut block = input;
                let start = Instant::now();
                chain.begin_block();
                chain.process_block(black_box(&mut block)).unwrap();
                chain.commit_block();
                let elapsed = start.elapsed().as_nanos();
                black_box(block);
                if iteration >= 512 {
                    times.push(elapsed);
                }
            }
            times.sort_unstable();
            println!(
                "{label} aligned={aligned} 48k/128,512warmup+2048measured mean={}ns p50={}ns p99={}ns max={}ns",
                times.iter().sum::<u128>() / times.len() as u128,
                times[1024],
                times[2027],
                times[2047]
            );
        }
    }
}
