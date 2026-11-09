use super::*;
use crate::{
    ForeignModulationRoute, ForeignModulationSamples, ModulationSourceId,
    PreparedForeignModulation, PreparedModulationSource,
};

const BINDING: ForeignProcessorBinding = ForeignProcessorBinding {
    root: 1,
    instance: 17,
    graph: 2,
    epoch: 3,
};
const BASE: f64 = 0.500_000_000_000_000_1;

fn effect(
    raw_id: u32,
    amount: f32,
    bypassed: bool,
    prepared_binding: ForeignProcessorBinding,
) -> PreparedRuntimeInstance {
    let mut instance = declaration(ProcessorChainOwner::Track(5), bypassed);
    instance.definition = Some(PreparedProcessorFactory::Foreign(
        PreparedForeignProcessor::new(8, 3, 16)
            .unwrap()
            .with_binding(prepared_binding)
            .unwrap(),
    ));
    instance.modulation = Some(
        PreparedForeignModulation::new(
            raw_id,
            BASE,
            vec![
                PreparedModulationSource::free(
                    ModulationSourceId::new(1).unwrap(),
                    48_000.0,
                    12_000.0,
                    0.0,
                )
                .unwrap(),
            ],
            &[ForeignModulationRoute {
                source: 0,
                amount,
                range: [-1.0, 1.0],
            }],
            &[],
            2,
        )
        .unwrap()
        .into_effect_modulation(BINDING)
        .unwrap(),
    );
    instance
}

#[derive(Default)]
struct Endpoint {
    values: [f64; 8],
    start: usize,
    received: usize,
    calls: usize,
    bypassed: bool,
    refuse_at: Option<usize>,
    revoked: bool,
}
impl ForeignProcessors for Endpoint {
    fn process_segment(
        &mut self,
        _: ForeignProcessorNode<'_>,
        _: usize,
        _: &mut [StereoFrame],
        _: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        Err(DeviceContractError::ForeignProcessingUnavailable)
    }

    fn process_segment_with_modulation(
        &mut self,
        node: ForeignProcessorNode<'_>,
        frame_offset: usize,
        frames: &mut [StereoFrame],
        lane: ForeignModulationSamples<'_>,
        _: Option<&dyn NativeInstrumentClock>,
    ) -> Result<(), DeviceContractError> {
        assert_eq!(node.prepared.binding(), Some(BINDING));
        assert_eq!(node.bypassed, self.bypassed);
        assert_eq!(frame_offset, self.start + self.received);
        assert_eq!(lane.id, 0);
        assert_eq!(frames.len(), lane.normalized.len());
        if self.refuse_at == Some(frame_offset) {
            return Err(DeviceContractError::ForeignProcessingFailed);
        }
        let end = self.received + frames.len();
        self.values[self.received..end].copy_from_slice(lane.normalized);
        self.received = end;
        self.calls += 1;
        Ok(())
    }

    fn reject_callback(&mut self) {
        self.revoked = true;
    }
}

#[test]
fn graph_stages_exact_samples_at_physical_offsets_and_freezes_committed_tail() {
    let instance = effect(0, 0.25, false, BINDING);
    let quote = NativeRuntimeChain::declared_storage_preflight_bytes_for_prepared(
        std::slice::from_ref(&instance),
    );
    let mut chain = NativeRuntimeChain::new_prepared(vec![instance.clone()]).unwrap();
    assert_eq!(chain.accounted_storage_bytes(), quote);
    assert!(
        chain
            .storage_view()
            .entries()
            .next()
            .unwrap()
            .matches_prepared_runtime_storage(&instance)
    );
    chain.begin_block();
    let mut rejected = Endpoint {
        start: 5,
        refuse_at: Some(7),
        ..Default::default()
    };
    assert_eq!(
        chain.process_range_with_foreign(0..1, &mut [[0.0; 2]; 3], 5, None, None, &mut rejected,),
        Err(DeviceContractError::ForeignProcessingFailed)
    );
    assert!(rejected.revoked);
    chain.rollback_block();

    let mut accepted = Endpoint {
        start: 5,
        ..Default::default()
    };
    chain.begin_block();
    for (offset, length) in [(5, 1), (6, 2)] {
        chain
            .process_range_with_foreign(
                0..1,
                &mut [[0.0; 2]; 2][..length],
                offset,
                None,
                None,
                &mut accepted,
            )
            .unwrap();
    }
    chain.commit_block();
    assert_eq!(accepted.calls, 2);
    assert_eq!(&accepted.values[..3], &[BASE, BASE + 0.25, BASE]);

    let mut next = NativeRuntimeChain::new_prepared(vec![effect(0, 0.5, true, BINDING)]).unwrap();
    assert!(next.can_inherit_from(&chain));
    next.inherit_from(&chain);
    next.begin_block();
    let mut bypass = Endpoint {
        bypassed: true,
        ..Default::default()
    };
    next.process_range_with_foreign(0..1, &mut [[0.0; 2]], 0, None, None, &mut bypass)
        .unwrap();
    next.commit_block();
    assert_eq!(bypass.values[0], BASE - 0.5);
    next.begin_output_tail();
    next.begin_block();
    let mut tail = Endpoint {
        bypassed: true,
        ..Default::default()
    };
    next.process_range_with_foreign(0..1, &mut [[0.0; 2]; 4], 0, None, None, &mut tail)
        .unwrap();
    next.commit_block();
    assert_eq!(&tail.values[..4], &[BASE - 0.5; 4]);

    let different = NativeRuntimeChain::new_prepared(vec![effect(1, 0.5, true, BINDING)]).unwrap();
    assert!(!different.can_inherit_from(&next));
}

#[test]
fn graph_refuses_wrong_effect_binding_or_instrument_and_requires_a_dense_consumer() {
    let other = ForeignProcessorBinding {
        epoch: 4,
        ..BINDING
    };
    assert!(matches!(
        NativeRuntimeChain::new_prepared(vec![effect(0, 0.25, false, other)]),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    ));
    let mut instrument = effect(0, 0.25, false, BINDING);
    let Some(PreparedProcessorFactory::Foreign(prepared)) = instrument.definition.as_mut() else {
        panic!("fixture must be foreign");
    };
    *prepared = prepared.with_instrument_profile();
    assert!(matches!(
        NativeRuntimeChain::new_prepared(vec![instrument]),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    ));
    let instance = effect(0, 0.25, false, BINDING);
    let mut ordinary = endpoint(&instance);
    let mut chain = NativeRuntimeChain::new_prepared(vec![instance]).unwrap();
    chain.begin_block();
    let mut pcm = [[0.25; 2]; 2];
    assert_eq!(
        chain.process_range_with_foreign(0..1, &mut pcm, 0, None, None, &mut ordinary),
        Err(DeviceContractError::ForeignProcessingUnavailable)
    );
    assert_eq!(ordinary.segments, 0);
    assert!(ordinary.revoked);
    assert_eq!(pcm, [[0.0; 2]; 2]);
}
