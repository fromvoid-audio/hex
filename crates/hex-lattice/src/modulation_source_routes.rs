//! Control-prepared dependency order for routes between device-local sources.
//! The host resolves stable IDs to sorted bank indices. A macro value is the
//! normalized destination admitted here. A route changes only the current
//! source sample, leaving authored state and source history in their owners.
use crate::{
    MAX_MODULATION_ROUTES, MAX_MODULATION_SOURCES, PreparedModulationSource,
    modulation::RangeContribution,
};
use hex_lattice_contract::DeviceContractError as Error;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SourceModulationRoute {
    pub source: usize,
    pub target: usize,
    pub amount: f32,
    pub range: [f32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreparedSourceRoute {
    pub(crate) source: usize,
    pub(crate) target: usize,
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
            .is_some_and(PreparedModulationSource::is_macro)
        {
            return Err(Error::UnsupportedProcessingClass);
        }
        ordered.push((
            index,
            PreparedSourceRoute {
                source: route.source,
                target: route.target,
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
        bank.render_chunk(4).unwrap();
        bank.apply_source_routes(&routes).unwrap();
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
        bank.render_chunk(4).unwrap();
        bank.apply_source_routes(&routes).unwrap();
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
