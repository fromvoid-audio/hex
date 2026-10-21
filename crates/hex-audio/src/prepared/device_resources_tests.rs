use super::*;
use hex_engine::native_device_resources::{
    PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
};
use hex_engine::{AudioStorageLedgerReservation, PreparedRevision, SineTable};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::collections::BTreeMap;

fn limits() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 512,
        handle_rows: 256,
        root_rows: 128,
        max_retained_bytes: 20_000_000,
        max_construction_peak_bytes: 20_000_000,
    }
}
fn compile(editor: &Editor, sources: &ResidentDeviceAssets) -> Playback {
    let cancel = AtomicBool::new(false);
    let resources =
        PreparedDeviceResources::prepare_on_control(sources, editor.project(), 1_000_000, &cancel)
            .unwrap();
    let revision = Arc::new(
        PreparedRevision::prepare_with_device_resources(
            editor.project(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            100_000,
            &cancel,
        )
        .unwrap(),
    );
    Playback::try_from_prepared_with_device_resources(revision, &resources).unwrap()
}
fn source(
    editor: &mut Editor,
    sources: &mut ResidentDeviceAssets,
    level: f32,
) -> hex_project::AssetId {
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: format!("source-{level}.wav").into(),
            sample_rate: 8000,
            frames: 1024,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    sources
        .insert_on_control(
            (
                asset,
                hex_engine::OwnedAudioSource::try_resident(8000, vec![[level; 2]; 1024])
                    .ok()
                    .unwrap(),
            ),
            0,
            limits(),
            1024,
            &AtomicBool::new(false),
            || Ok::<_, ResourcePreparationError>(()),
        )
        .unwrap();
    asset
}
fn block(callback: &mut Callback) -> [Frame; 32] {
    let mut output = [[0.0; 2]; 32];
    let mut render = || {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let counted = allocation_counter::measure(&mut render);
        assert_eq!(
            (
                counted.count_total,
                counted.bytes_total,
                counted.count_current,
                counted.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    render();
    output
}

#[test]
fn ordinary_sampler_publication_keeps_migrated_a_and_b_obligations_through_resource_free_c() {
    let mut editor = Editor::new(Project::new(8000).unwrap(), 16).unwrap();
    let mut sources = ResidentDeviceAssets::default();
    #[cfg(feature = "allocation-probe")]
    let (_observer, dropped) = hex_engine::native_device_resources::control_drop_probe::observe(
        std::thread::current().id(),
    );
    let a = source(&mut editor, &mut sources, 0.25);
    let b = source(&mut editor, &mut sources, -0.125);
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "sampler".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    editor
        .execute(Command::InsertInstrument {
            track,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, a, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let device = editor
        .project()
        .track(track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let a_graph = compile(&editor, &sources);
    let (mut control, mut callback) = channel(8000).unwrap();
    let a_graph = match control.publish(a_graph) {
        Err(error) => error.playback,
        Ok(_) => panic!("unconfigured source accepted"),
    };
    assert_eq!(
        control.outstanding_device_resource_leases(),
        0,
        "unconfigured refusal cannot issue an obligation"
    );
    control
        .configure_device_resource_publication_on_control(limits())
        .unwrap();
    let a_revision = control
        .publish_with_policy_and_context_generation(a_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert!(
        block(&mut callback)
            .iter()
            .flatten()
            .any(|value| *value != 0.0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), a_revision);
    assert_eq!(control.outstanding_device_resource_leases(), 1);
    assert!(
        control
            .configure_device_resource_publication_on_control(limits())
            .is_err()
    );
    editor
        .execute(Command::ReplaceDeviceState {
            device,
            state:
                hex_engine::native_device_resources::current_sampler_fixture::classic_loop_state(
                    1024, b, 0.05,
                )
                .unwrap(),
        })
        .unwrap();
    let b_graph = compile(&editor, &sources);
    let b_revision = control
        .publish_with_policy_and_context_generation(b_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), b_revision);
    assert_eq!(control.reclaim(), 1);
    assert_eq!(
        control.outstanding_device_resource_leases(),
        2,
        "returning A cannot close the lease moved into B's tail"
    );
    editor.execute(Command::RemoveInstrument { track }).unwrap();
    let c_graph = compile(&editor, &sources);
    drop(sources);
    #[cfg(feature = "allocation-probe")]
    assert!(dropped.try_recv().is_err());
    let c_revision = control
        .publish_with_policy_and_context_generation(c_graph, PublicationPolicy::InheritRuntime, 7)
        .unwrap();
    let c_output = block(&mut callback);
    assert!(
        c_output.iter().flatten().all(|value| *value > 0.0),
        "source-free C renders positive A's old tail, not negative B"
    );
    assert_eq!(control.status.applied_revision.load(Acquire), c_revision);
    let installed = callback
        .active
        .as_ref()
        .and_then(ActivePublication::legacy)
        .unwrap();
    assert_eq!(
        (installed.control_context_generation, installed.revision),
        (7, c_revision)
    );
    assert_eq!(control.reclaim(), 1);
    assert_eq!(
        control.outstanding_device_resource_leases(),
        1,
        "A physically lives in C; B's untriggered runtime returned on control"
    );
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(dropped.try_recv().unwrap(), std::thread::current().id());
        assert!(
            dropped.try_recv().is_err(),
            "A cannot drop while C owns its tail"
        );
    }
    control.command(PlaybackCommand::Stop).unwrap();
    block(&mut callback);
    let empty = Playback::try_from_prepared_with_session(Arc::new(
        PreparedRevision::prepare(
            &Project::new(8000).unwrap(),
            &BTreeMap::new(),
            Arc::new(SineTable::new()),
            None,
        )
        .unwrap(),
    ))
    .unwrap();
    control.publish(empty).unwrap();
    block(&mut callback);
    control.reclaim();
    assert_eq!(control.outstanding_device_resource_leases(), 0);
    #[cfg(feature = "allocation-probe")]
    {
        assert_eq!(dropped.try_recv().unwrap(), std::thread::current().id());
        assert!(dropped.try_recv().is_err());
    }
    control.request_shutdown();
    block(&mut callback);
    control.reclaim();
}
