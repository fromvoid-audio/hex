//! Offline original filter overhead; not a CPAL deadline or Live comparison.
#[allow(dead_code)]
#[path = "support/operator_feedback_fixture.rs"]
mod fixture;
use hex_engine::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SineTable};
use hex_lattice::NativeInstrumentDefinition;
use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
use hex_project::{Command, Editor};
use std::{collections::BTreeMap, error::Error, hint::black_box, sync::Arc, time::Instant};
fn main() -> Result<(), Box<dyn Error>> {
    for filtered in [false, true] {
        let f = fixture::project_with_operators(8, 16, 0.4, 0.5, true)?;
        let mut editor = Editor::new(f.project, 16)?;
        if filtered {
            for device in f.instrument_ids {
                let mut state = fixture::state_with_operators(0.4, 0.5, true)?;
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
            .prepare_instrument(state, 48000)?
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
        let start = Instant::now();
        for _ in 0..1000 {
            playback.render(&mut pcm)?;
            black_box(&pcm);
        }
        let elapsed = start.elapsed();
        println!(
            "filter={filtered} tracks=8 voices_per_track=16 operators=4 block=128 blocks=1000 prepare_ms={:.3} total_ms={:.3} mean_block_us={:.3} pair_bytes={} arch={} os={}",
            preparation.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0,
            factory.declared_twin_bytes() * 2,
            std::env::consts::ARCH,
            std::env::consts::OS
        );
    }
    Ok(())
}
