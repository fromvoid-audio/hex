//! Offline original filter overhead; not a CPAL deadline or Live comparison.
#[allow(dead_code)]
#[path = "support/operator_feedback_fixture.rs"]
mod fixture;
use hex_devices::operator_state::OPERATOR_METADATA;
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice::NativeInstrumentDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{Command, Editor};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
fn main() -> Result<(), Box<dyn Error>> {
    let initial =
        fixture::state_with_operators(OPERATOR_METADATA.default_state()?, 0.4, 0.5, true)?;
    for filtered in [false, true] {
        let f = fixture::project(8, 16, &initial)?;
        let mut editor = Editor::new(f.project, 16)?;
        if filtered {
            for device in f.instrument_ids {
                let mut state = initial.clone();
                for (id, value) in [
                    (20001, ParameterValue::Bool(true)),
                    (20002, ParameterValue::Choice(4)),
                    (20006, ParameterValue::Float(80.0)),
                    (20007, ParameterValue::Float(35.0)),
                ] {
                    state = hex_devices::operator_state::OPERATOR_METADATA.patch_parameter(
                        &state,
                        ParameterId::from_static(id),
                        value,
                    )?;
                }
                editor.execute(Command::ReplaceDeviceState { device, state })?;
            }
        }
        let state = editor.project().tracks()[0]
            .instrument()
            .ok_or("instrument")?
            .state();
        let factory = hex_devices::operator_state::OPERATOR_METADATA
            .prepare_instrument(
                state,
                &hex_lattice::DevicePreparationContext {
                    sample_rate: 48000,
                    tuning: &hex_lattice::TuningTable::default(),
                    resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
                },
            )?
            .prepare_mutable_runtime()?
            .ok_or("factory")?;
        let start = Instant::now();
        let revision = Arc::new(PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )?);
        let preparation = start.elapsed();
        let mut playback = SessionPlayback::new(revision)?;
        playback.apply(SessionPlaybackCommand::Play)?;
        let mut pcm = [[0.0; 2]; 128];
        for _ in 0..100 {
            playback.render(&mut pcm)?;
        }
        let mut block_us = [0.0_f64; 1000];
        let start = Instant::now();
        for sample in &mut block_us {
            let block_start = Instant::now();
            playback.render(&mut pcm)?;
            black_box(&pcm);
            *sample = block_start.elapsed().as_secs_f64() * 1_000_000.0;
        }
        let elapsed = start.elapsed();
        block_us.sort_by(f64::total_cmp);
        println!(
            "filter={filtered} tracks=8 voices_per_track=16 operators=4 block=128 blocks=1000 prepare_ms={:.3} total_ms={:.3} mean_block_us={:.3} p50_us={:.3} p95_us={:.3} p99_us={:.3} max_us={:.3} pair_bytes={} arch={} os={}",
            preparation.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0,
            block_us[500],
            block_us[950],
            block_us[990],
            block_us[999],
            factory.declared_twin_bytes() * 2,
            std::env::consts::ARCH,
            std::env::consts::OS
        );
    }
    Ok(())
}
