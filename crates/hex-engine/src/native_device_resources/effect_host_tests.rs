//! Actual prepared Track/Renderer scope retention. Unregistered ABI fixture;
//! not a resource-registry or native graph admission permit.
use super::*;
use crate::{Audio, Clip, PreparedEffectInstance, Renderer, Track};
use hex_lattice::{DeviceResources, HostDeviceResourceScope};
use std::sync::Arc;
fn prepared(owner: &DeviceResourceBank, asset: AssetId) -> PreparedEffectInstance {
    let scoped = super::effect_scope_tests::scoped(owner, asset, false);
    PreparedEffectInstance {
        instance: scoped.instance,
        resources: Some(scoped.resources),
    }
}
#[test]
fn actual_prepared_effect_source_and_arrangement_renderer_keep_the_original_root_until_control_drop()
 {
    let Some((owner, ids)) = super::effect_scope_tests::owner() else {
        return;
    };
    let retired = super::retained::RETIRED_RESOURCE_ROOTS.with(|counter| counter.get());
    let mut track = Track::new(
        vec![Clip::new(Audio::new(44_100, vec![[1.0; 2]; 4]).unwrap(), 0, 4, false).unwrap()],
        1.0,
        false,
    )
    .unwrap();
    track.device_chain = Arc::from(vec![prepared(&owner, ids[0])]);
    let project = crate::Project::new(44_100, vec![track]).unwrap();
    drop(owner);
    let mut renderer = Renderer::new(&project);
    renderer.play();
    let mut output = [[0.0; 2]; 4];
    renderer.render(&mut output).unwrap();
    assert_eq!(output, tests::PCM);
    renderer.seek(0);
    let mut first = [[0.0; 2]; 1];
    let mut rest = [[0.0; 2]; 3];
    renderer.render(&mut first).unwrap();
    renderer.render(&mut rest).unwrap();
    assert_eq!(first, tests::PCM[..1]);
    assert_eq!(rest, tests::PCM[1..]);
    drop(renderer);
    assert_eq!(
        super::retained::RETIRED_RESOURCE_ROOTS.with(|counter| counter.get()),
        retired
    );
    assert_eq!(
        project.tracks()[0].device_chain[0]
            .resources
            .get(0)
            .unwrap()
            .pcm
            .frame(2),
        Some(tests::PCM[2])
    );
    drop(project);
    assert_eq!(
        super::retained::RETIRED_RESOURCE_ROOTS.with(|counter| counter.get()),
        retired + 1
    );
}
#[test]
fn optional_scope_never_collapses_owned_empty_into_absent_or_asset_free_storage_matching() {
    let Some((owner, ids)) = super::effect_scope_tests::owner() else {
        return;
    };
    let empty = hex_lattice_contract::NativeDeviceState::new(
        hex_lattice_contract::DeviceTypeId::from_static("test.empty-scope"),
        1,
        vec![],
    )
    .unwrap();
    let resources = owner
        .prepare_configuration_on_control(&empty, 4096)
        .unwrap();
    let some = Some(resources);
    let none: Option<DeviceResourceConfiguration> = None;
    assert!(some.is_empty());
    assert!(!some.same_configuration(&none));
    let instance = hex_lattice::PreparedRuntimeInstance {
        meter: Default::default(),
        owner: hex_lattice::ProcessorChainOwner::Track(1),
        device_id: 1,
        device_type: empty.type_id(),
        matrix: hex_lattice::StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 44_100,
        configuration: Some(Arc::new(empty)),
        definition: None,
    };
    let plain = instance.clone();
    let chain = crate::EffectRuntimeChain::new_prepared(vec![PreparedEffectInstance {
        instance,
        resources: some,
    }])
    .unwrap();
    assert!(
        !chain
            .storage_view()
            .entries()
            .next()
            .unwrap()
            .matches_prepared_runtime_storage(&plain)
    );
    let absent = crate::EffectRuntimeChain::new_prepared(vec![PreparedEffectInstance {
        instance: plain.clone(),
        resources: none,
    }])
    .unwrap();
    assert!(!chain.configuration_matches_range(&absent, 0..1));
    assert!(
        absent
            .storage_view()
            .entries()
            .next()
            .unwrap()
            .matches_prepared_runtime_storage(&plain)
    );
    // A referenced configuration can never enter through an absent scope.
    let scoped = super::effect_scope_tests::scoped(&owner, ids[0], false);
    assert!(
        crate::EffectRuntimeChain::new_prepared(vec![PreparedEffectInstance {
            instance: scoped.instance,
            resources: None
        }])
        .is_err()
    );
}
