//! Control-prepared dependency order for routes between device-local sources.
//! The host resolves stable source and output IDs to sorted bank indices.
//! A source with incoming
//! edges renders only after its upstream sources. Authored source values remain
//! independent of the resulting per-sample phase steps or macro output.
use crate::{
    MAX_MODULATION_ROUTES, MAX_MODULATION_SOURCES, PreparedModulationSource,
    modulation::RangeContribution,
};
use hex_lattice_contract::DeviceContractError as Error;

/// A state-aware parameter on the destination source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SourceModulationParameter {
    MacroValue,
    MathA,
    MathB,
    MixA,
    MixB,
    MixBlend,
    /// Signed full depth shifts the authored free rate by four octaves.
    /// Zero rate stays stopped and the effective step saturates at Nyquist.
    FreeLfoRate,
    /// Apply the same signed rate law to the sample's tempo-derived step.
    TempoLfoRate,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceModulationRoute {
    pub source: usize,
    pub target: usize,
    pub parameter: SourceModulationParameter,
    pub amount: f32,
    pub range: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreparedSourceRoute {
    pub(crate) source: usize,
    pub(crate) target: usize,
    pub(crate) parameter: SourceModulationParameter,
    pub(crate) contribution: RangeContribution,
}

pub(crate) fn prepare(
    sources: &[PreparedModulationSource],
    routes: &[SourceModulationRoute],
) -> Result<Vec<PreparedSourceRoute>, Error> {
    if routes.len() > MAX_MODULATION_ROUTES || sources.len() > MAX_MODULATION_SOURCES {
        return Err(Error::RuntimeBudgetExceeded);
    }
    if routes.is_empty() {
        return Ok(Vec::new());
    }
    let mut ordered = Vec::new();
    ordered
        .try_reserve_exact(routes.len())
        .map_err(|_| Error::RuntimeAllocation)?;

    for (index, route) in routes.iter().enumerate() {
        if route.source >= sources.len() {
            return Err(Error::InvalidDescriptor);
        }
        if !sources
            .get(route.target)
            .is_some_and(|source| source.supports_modulation_parameter(route.parameter))
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        ordered.push((
            index,
            PreparedSourceRoute {
                source: route.source,
                target: route.target,
                parameter: route.parameter,
                contribution: RangeContribution::prepare(route.amount, route.range)?,
            },
        ));
    }

    let mut ranks = [None; MAX_MODULATION_SOURCES];
    for rank in 0..sources.len() {
        let next = (0..sources.len()).find(|&candidate| {
            ranks[candidate].is_none()
                && ordered
                    .iter()
                    .all(|(_, route)| route.target != candidate || ranks[route.source].is_some())
        });
        let next = next.ok_or(Error::InvalidDescriptor)?;
        ranks[next] = Some(rank);
    }
    ordered.sort_unstable_by_key(|(index, route)| (ranks[route.target], *index));
    let mut prepared = Vec::new();
    prepared
        .try_reserve_exact(ordered.len())
        .map_err(|_| Error::RuntimeAllocation)?;
    prepared.extend(ordered.into_iter().map(|(_, route)| route));
    Ok(prepared)
}

pub(crate) fn target_mask(routes: &[PreparedSourceRoute]) -> u32 {
    routes
        .iter()
        .fold(0, |mask, route| mask | (1 << route.target))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ModulationSourceBank, ModulationSourceId};

    fn sources() -> [PreparedModulationSource; 4] {
        let id = |n| ModulationSourceId::new(n).unwrap();
        [
            PreparedModulationSource::macro_value(id(1), 8, 0.25).unwrap(),
            PreparedModulationSource::macro_value(id(2), 8, 0.25).unwrap(),
            PreparedModulationSource::free(id(3), 8.0, 2.0, 0.0).unwrap(),
            PreparedModulationSource::macro_value(id(4), 8, 0.5).unwrap(),
        ]
    }

    fn route(source: usize, target: usize, amount: f32) -> SourceModulationRoute {
        SourceModulationRoute {
            source,
            target,
            parameter: SourceModulationParameter::MacroValue,
            amount,
            range: [-1.0, 1.0],
        }
    }

    #[test]
    fn source_edges_resolve_dependencies_and_fan_in_before_device_consumption() {
        let sources = sources();
        let routes = prepare(
            &sources,
            &[route(1, 0, 1.0), route(2, 1, 0.25), route(3, 1, 0.25)],
        )
        .unwrap();
        let mut bank = ModulationSourceBank::new(&sources, 4).unwrap();
        bank.begin();
        bank.render_chunk_with_routes(4, None, None, &routes, target_mask(&routes))
            .unwrap();
        assert_eq!(
            bank.samples(sources[0].id).unwrap(),
            &[0.625, 0.875, 0.625, 0.375]
        );
        assert_eq!(
            bank.samples(sources[1].id).unwrap(),
            &[0.375, 0.625, 0.375, 0.125]
        );
        bank.rollback();
        bank.begin();
        bank.render_chunk_with_routes(4, None, None, &routes, target_mask(&routes))
            .unwrap();
        assert_eq!(
            bank.samples(sources[0].id).unwrap(),
            &[0.625, 0.875, 0.625, 0.375]
        );
        bank.commit().unwrap();
        bank.begin();
        bank.render_chunk(4).unwrap();
        assert_eq!(bank.samples(sources[0].id).unwrap(), &[0.25; 4]);
    }

    #[test]
    fn free_rate_fan_in_drives_phase_before_the_downstream_macro_and_rolls_back() {
        use crate::LfoWaveform;
        let id = |n| ModulationSourceId::new(n).unwrap();
        let sources = [
            PreparedModulationSource::free_with_waveform(
                id(1),
                8.0,
                1.0,
                0.0,
                LfoWaveform::RisingRamp,
            )
            .unwrap(),
            PreparedModulationSource::macro_value(id(2), 8, 0.5).unwrap(),
            PreparedModulationSource::free_with_waveform(id(3), 8.0, 2.0, 0.0, LfoWaveform::Square)
                .unwrap(),
            PreparedModulationSource::macro_value(id(4), 8, 1.0).unwrap(),
        ];
        let rate = |source, amount| SourceModulationRoute {
            parameter: SourceModulationParameter::FreeLfoRate,
            ..route(source, 0, amount)
        };
        let routes = prepare(
            &sources,
            &[route(0, 1, 0.25), rate(2, 0.25), rate(3, -0.125)],
        )
        .unwrap();
        let mask = target_mask(&routes);
        let mut phase = 0.0;
        let expected: [f32; 13] = std::array::from_fn(|frame| {
            let sample = (2.0 * phase - 1.0) as f32;
            let octaves = if frame % 4 < 2 { 0.5_f64 } else { -1.5 };
            phase = (phase + 0.125 * octaves.exp2()) % 1.0;
            sample
        });
        let mut whole = ModulationSourceBank::new(&sources, 13).unwrap();
        whole.begin();
        whole
            .render_chunk_with_routes(13, None, None, &routes, mask)
            .unwrap();
        assert_eq!(whole.samples(id(1)).unwrap(), expected);
        whole.rollback();

        whole.begin();
        let mut start = 0;
        for count in [1, 7, 5] {
            whole
                .render_chunk_with_routes(count, None, None, &routes, mask)
                .unwrap();
            let samples = &expected[start..start + count];
            assert_eq!(whole.samples(id(1)).unwrap(), samples);
            for (&actual, &sample) in whole.samples(id(2)).unwrap().iter().zip(samples) {
                assert_eq!(actual, (0.5 + 0.25 * f64::from(sample)) as f32);
            }
            start += count;
        }
        whole.commit().unwrap();

        let tempo = PreparedModulationSource::tempo(id(1), 8.0, 120.0, 1.0, 0.0).unwrap();
        let mut incompatible = sources;
        incompatible[0] = tempo;
        assert!(prepare(&incompatible, &[rate(2, 0.25)]).is_err());
        assert!(prepare(&sources, &[rate(1, 0.25), route(0, 1, 0.25)]).is_err());
    }

    #[test]
    fn source_edges_refuse_cycles_invalid_values_and_unsupported_destinations() {
        let sources = sources();
        for routes in [
            vec![route(0, 0, 1.0)],
            vec![route(0, 1, 0.5), route(1, 0, 0.5)],
            vec![route(0, 2, 1.0)],
            vec![route(4, 1, 1.0)],
            vec![route(0, 4, 1.0)],
            vec![route(1, 0, f32::NAN)],
        ] {
            assert!(prepare(&sources, &routes).is_err());
        }
    }
}
