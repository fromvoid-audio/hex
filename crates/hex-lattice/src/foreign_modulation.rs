//! Sample-local normalized composition for a continuous foreign effect target.
//! The audio host admits the actual source and capability, owns the borrowed
//! output scratch and decides when the surrounding callback commits.
use crate::{
    ForeignModulationRoute, ModulationRuntime, NativeInstrumentClock, PreparedModulation,
    PreparedModulationSource, PreparedModulationState, SourceModulationRoute, StereoFrame,
};
use hex_lattice_contract::DeviceContractError as Error;

/// One foreign raw ID and its exact restored f64 base, paired with a validated
/// effect-local bank. The provider host still proves that this target exists,
/// is continuous and accepts sample-rate input on the actual prepared source.
#[derive(Clone, Debug)]
pub struct PreparedForeignModulation {
    raw_id: u32,
    normalized_base: f64,
    state: PreparedModulationState,
}

impl PreparedForeignModulation {
    pub fn new(
        raw_id: u32,
        normalized_base: f64,
        sources: Vec<PreparedModulationSource>,
        routes: &[ForeignModulationRoute],
        source_routes: &[SourceModulationRoute],
        chunk_frames: usize,
    ) -> Result<Self, Error> {
        if !(0.0..=1.0).contains(&normalized_base) {
            return Err(Error::InvalidParameterValue);
        }
        if sources
            .iter()
            .any(PreparedModulationSource::reads_note_events)
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        let program = PreparedModulation::for_foreign_target(routes, sources.len())?;
        let state = PreparedModulationState::with_source_routes(
            sources,
            program,
            chunk_frames,
            source_routes,
        )?;
        Ok(Self {
            raw_id,
            normalized_base,
            state,
        })
    }

    pub fn quoted_runtime_heap_bytes(&self) -> Option<usize> {
        self.state.quoted_runtime_heap_bytes()
    }

    pub fn prepare(&self, sample_rate: u32) -> Result<ForeignModulationRuntime, Error> {
        self.state.validate_sample_rate(sample_rate)?;
        Ok(ForeignModulationRuntime {
            raw_id: self.raw_id,
            normalized_base: self.normalized_base,
            runtime: self.state.prepare()?,
        })
    }
}

/// Separate from native provider automation so a raw ID cannot masquerade as
/// a native typed parameter or grant access to a provider's controls.
pub struct ForeignModulationRuntime {
    raw_id: u32,
    normalized_base: f64,
    runtime: ModulationRuntime,
}

impl ForeignModulationRuntime {
    pub fn begin(&mut self) {
        self.runtime.begin();
    }

    /// Lend one normalized value per input frame. A refused consumer abandons
    /// the speculative source history for the entire surrounding transaction.
    pub fn render_chunk(
        &mut self,
        input: &mut [StereoFrame],
        clock: Option<&dyn NativeInstrumentClock>,
        output: &mut [f64],
        consume: impl FnOnce(&mut [StereoFrame], u32, &[f64]) -> Result<(), Error>,
    ) -> Result<(), Error> {
        let base = self.normalized_base;
        let raw_id = self.raw_id;
        self.runtime
            .render_audio_chunk(input, None, None, clock, |input, values| {
                values.compose_foreign_target(base, output)?;
                consume(input, raw_id, output)
            })
    }

    pub fn commit(&mut self) -> Result<(), Error> {
        self.runtime.commit()
    }

    pub fn rollback(&mut self) {
        self.runtime.rollback();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ModulationSourceId;

    const BASE: f64 = 0.500_000_000_000_000_1;
    const ROUTES: [ForeignModulationRoute; 2] = [
        ForeignModulationRoute {
            source: 0,
            amount: -0.5,
            range: [-1.0, 1.0],
        },
        ForeignModulationRoute {
            source: 0,
            amount: 0.25,
            range: [-0.5, 0.5],
        },
    ];

    fn sources() -> Vec<PreparedModulationSource> {
        vec![
            PreparedModulationSource::free(
                ModulationSourceId::new(1).unwrap(),
                48_000.0,
                12_000.0,
                0.0,
            )
            .unwrap(),
        ]
    }

    fn read(runtime: &mut ForeignModulationRuntime, output: &mut [f64]) {
        let mut pcm = [[0.25, -0.25]; 8];
        runtime
            .render_chunk(&mut pcm[..output.len()], None, output, |pcm, id, values| {
                assert_eq!(id, 0);
                assert_eq!(pcm, &[[0.25, -0.25]; 8][..values.len()]);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn exact_foreign_base_and_signed_routes_survive_partitions_and_refusal() {
        let prepared = PreparedForeignModulation::new(0, BASE, sources(), &ROUTES, &[], 8).unwrap();
        let mut whole = prepared.prepare(48_000).unwrap();
        whole.begin();
        let mut values = [0.0; 4];
        read(&mut whole, &mut values);
        whole.commit().unwrap();
        for (actual, expected) in values
            .into_iter()
            .zip([BASE, BASE - 0.375, BASE, BASE + 0.375])
        {
            assert!((actual - expected).abs() <= 1e-15);
        }
        assert_eq!(values[0].to_bits(), BASE.to_bits());

        let mut split = prepared.prepare(48_000).unwrap();
        split.begin();
        let mut output = [0.0; 4];
        read(&mut split, &mut output[..3]);
        let refusal = split.render_chunk(&mut [[0.0; 2]], None, &mut output[3..], |_, _, _| {
            Err(Error::ForeignProcessingUnavailable)
        });
        assert_eq!(refusal, Err(Error::ForeignProcessingUnavailable));
        assert!(split.commit().is_err());

        split.begin();
        read(&mut split, &mut output[..1]);
        read(&mut split, &mut output[1..]);
        split.commit().unwrap();
        assert_eq!(output, values);
        whole.begin();
        split.begin();
        let mut after = [0.0; 1];
        read(&mut whole, &mut after);
        let mut resumed = [0.0; 1];
        read(&mut split, &mut resumed);
        assert_eq!(after, resumed);
    }

    #[test]
    fn clamps_only_after_summing_and_rejects_invalid_preparation() {
        let extreme = [ForeignModulationRoute {
            amount: 1.0,
            ..ROUTES[0]
        }];
        let mut runtime = PreparedForeignModulation::new(0, BASE, sources(), &extreme, &[], 8)
            .unwrap()
            .prepare(48_000)
            .unwrap();
        runtime.begin();
        let mut output = [0.0; 4];
        read(&mut runtime, &mut output);
        assert_eq!(output[1], 1.0);
        assert_eq!(output[3], 0.0);
        assert!(matches!(
            PreparedForeignModulation::new(0, f64::NAN, sources(), &ROUTES, &[], 8),
            Err(Error::InvalidParameterValue)
        ));
        assert!(matches!(
            PreparedForeignModulation::new(
                0,
                BASE,
                sources(),
                &[ForeignModulationRoute {
                    source: 1,
                    ..ROUTES[0]
                }],
                &[],
                8
            ),
            Err(Error::InvalidParameterValue)
        ));
    }
}
