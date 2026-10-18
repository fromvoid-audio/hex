use super::*;
use crate::native_device_resources::DeviceResourcePreparation;
use crate::{LaunchQuantization, SessionPlayback, SessionPlaybackCommand};
use hex_lattice::DeviceResources;
use hex_project::{BeatTime, Command, Created};
use std::collections::BTreeMap;

fn musical_clips(editor: &mut hex_project::Editor, track: TrackId) -> hex_project::ClipId {
    let Created::Clip(arrangement) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("arrangement")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: arrangement,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    let Created::Scene(scene) = editor
        .execute(Command::AddScene {
            name: "sample".into(),
        })
        .unwrap()
    else {
        panic!("scene")
    };
    let Created::SessionClip(session) = editor
        .execute(Command::CreateSessionMidiClip {
            track,
            scene,
            name: "sample".into(),
            source_length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("session")
    };
    editor
        .execute(Command::AddMidiNote {
            clip: session,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 127,
        })
        .unwrap();
    session
}
#[inline(never)]
fn playback(revision: Arc<PreparedRevision>) -> Box<SessionPlayback> {
    Box::new(SessionPlayback::new(revision).unwrap())
}
fn pcm_address(revision: &PreparedRevision, track: TrackId) -> usize {
    let contexts = revision.session().shared_contexts();
    let owner = contexts
        .owner(contexts.index_for_track(track).unwrap())
        .unwrap();
    let pcm = owner.resource_configuration().unwrap().get(0).unwrap().pcm;
    source_address(pcm)
}
fn source_address(pcm: hex_lattice::DeviceResourcePcm<'_>) -> usize {
    match pcm {
        hex_lattice::DeviceResourcePcm::Mono(v) => v.as_ptr() as usize,
        hex_lattice::DeviceResourcePcm::Stereo(v) => v.as_ptr() as usize,
    }
}
fn source_fixture(
    mono: bool,
) -> Option<(
    crate::native_device_resources::DeviceResourceBank,
    hex_project::Editor,
)> {
    let (source, identity, editor) = crate::shared_analytic::sampler_tests::canonical_fixture()?;
    drop(identity);
    if !mono {
        return Some((source, editor));
    }
    drop(source);
    use crate::native_device_resources::{
        DeviceResourceBank, DeviceResourceCatalog, OriginalResourceLayout,
    };
    use crate::{AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource};
    use std::sync::atomic::AtomicBool;
    let ids = crate::native_device_resources::tests::ids();
    let inputs = OwnedAudioInputs::try_new(
        vec![
            (
                ids[0],
                OwnedAudioSource::try_resident_mono(44_100, vec![0.25, -0.5, 0.75, -1.0])
                    .ok()
                    .unwrap(),
            ),
            (
                ids[1],
                OwnedAudioSource::try_resident(44_100, vec![[0.5; 2]; 4])
                    .ok()
                    .unwrap(),
            ),
        ],
        AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: 65_536,
            max_construction_data_bytes: 65_536,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        },
    )
    .ok()
    .unwrap()
    .prepare_input_storage_on_control()
    .ok()
    .unwrap();
    let mut specs = crate::native_device_resources::tests::specs(&ids);
    specs[0].layout = OriginalResourceLayout::Mono;
    let catalog = DeviceResourceCatalog::prepare_on_control(
        &inputs,
        &specs,
        crate::native_device_resources::tests::limits(),
        &AtomicBool::new(false),
    )
    .unwrap();
    let source = DeviceResourceBank::try_new_on_control(
        inputs,
        catalog,
        65_536,
        65_536,
        &AtomicBool::new(false),
    )
    .ok()
    .unwrap();
    Some((source, editor))
}
#[test]
fn sampler_resource_revision_reaches_actual_arrangement_session_and_checkpoint_without_pcm_copy() {
    for mono in [false, true] {
        verify_musical_revision(mono);
    }
}
fn verify_musical_revision(mono: bool) {
    let Some((source, mut editor)) = source_fixture(mono) else {
        return;
    };
    let track = editor.project().tracks()[0].id();
    let session_clip = musical_clips(&mut editor, track);
    let media = BTreeMap::<hex_project::AssetId, crate::Audio>::new();
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    // The old no-resource path still refuses loaded state instead of guessing PCM.
    assert!(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            crate::SharedSineTable::new_on_control(),
            None
        )
        .is_err()
    );
    let original = source
        .prepare_lease_on_control(
            editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .state(),
            4096,
        )
        .unwrap();
    let original_address = source_address(original.get(0).unwrap().pcm);
    drop(original);
    let revision = Arc::new(
        PreparedRevision::prepare_with_audio_transforms_and_device_resources(
            editor.project(),
            &media,
            crate::SharedSineTable::new_on_control(),
            None,
            &transforms,
            DeviceResourcePreparation {
                bank: &source,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap(),
    );
    eprintln!(
        "SUPPORTED actual original-resource PreparedRevision mono={mono}, Arrangement/Session sampler PCM, checkpoint and owner retirement"
    );
    let pointer = pcm_address(&revision, track);
    assert_eq!(pointer, original_address);
    let successor = Arc::new(
        PreparedRevision::prepare_with_audio_transforms_and_device_resources(
            editor.project(),
            &media,
            crate::SharedSineTable::new_on_control(),
            Some(revision.session()),
            &transforms,
            DeviceResourcePreparation {
                bank: &source,
                max_scope_metadata_bytes: 65_536,
            },
        )
        .unwrap(),
    );
    assert_eq!(pcm_address(&successor, track), pointer);
    let first_context = revision.session().shared_contexts();
    let next_context = successor.session().shared_contexts();
    assert_eq!(
        first_context
            .owner(first_context.index_for_track(track).unwrap())
            .unwrap()
            .id(),
        next_context
            .owner(next_context.index_for_track(track).unwrap())
            .unwrap()
            .id()
    );
    drop(source);
    drop(editor); // actual prepared contexts keep the same original PCM.
    let expected = [
        crate::native_device_resources::tests::PCM[0],
        crate::native_device_resources::tests::PCM[1],
        crate::native_device_resources::tests::PCM[2],
        crate::native_device_resources::tests::PCM[3],
        [0.0; 2],
        [0.0; 2],
    ];
    let expected = if mono {
        expected.map(|v| [v[0]; 2])
    } else {
        expected
    };
    for session in [false, true] {
        let mut whole = playback(Arc::clone(&revision));
        if session {
            whole
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: session_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        whole.apply(SessionPlaybackCommand::Play).unwrap();
        let mut out = [[0.0; 2]; 6];
        whole.render(&mut out).unwrap();
        assert_eq!(out, expected);
        let mut partitioned = playback(Arc::clone(&successor));
        if session {
            partitioned
                .apply(SessionPlaybackCommand::LaunchClip {
                    clip: session_clip,
                    quantization: LaunchQuantization::Immediate,
                })
                .unwrap();
        }
        partitioned.apply(SessionPlaybackCommand::Play).unwrap();
        let mut split = [[0.0; 2]; 6];
        partitioned.render(&mut split[..2]).unwrap();
        let mut checkpoints = crate::session_playback::MutableRuntimeCheckpointStore::new();
        checkpoints.capture_track(&partitioned, track).unwrap();
        partitioned.render(&mut split[2..]).unwrap();
        assert_eq!(split, expected);
        assert_eq!(pcm_address(&successor, track), pointer);
        assert_eq!(checkpoints.checkpoint_count(), 1);
    }
    // Explicit source compilation is not registration or callback publication.
    let registry = hex_devices::native_instrument_registry().unwrap();
    assert!(
        registry
            .definition(hex_devices::sampler::state::TYPE_ID)
            .is_none()
    );
}
#[test]
fn sampler_resource_revision_refuses_metadata_budget_and_preserves_accepted_revision() {
    let Some((source, identity, editor)) =
        crate::shared_analytic::sampler_tests::canonical_fixture()
    else {
        return;
    };
    let track = editor.project().tracks()[0].id();
    drop(identity);
    let media = BTreeMap::<hex_project::AssetId, crate::Audio>::new();
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let accepted = PreparedRevision::prepare_with_audio_transforms_and_device_resources(
        editor.project(),
        &media,
        crate::SharedSineTable::new_on_control(),
        None,
        &transforms,
        DeviceResourcePreparation {
            bank: &source,
            max_scope_metadata_bytes: 65_536,
        },
    )
    .unwrap();
    let pointer = pcm_address(&accepted, track);
    assert!(
        PreparedRevision::prepare_with_audio_transforms_and_device_resources(
            editor.project(),
            &media,
            crate::SharedSineTable::new_on_control(),
            None,
            &transforms,
            DeviceResourcePreparation {
                bank: &source,
                max_scope_metadata_bytes: 0
            }
        )
        .is_err()
    );
    assert_eq!(pcm_address(&accepted, track), pointer);
    assert!(
        accepted
            .session()
            .shared_contexts()
            .owners()
            .next()
            .is_some()
    );
}

#[test]
fn sampler_source_replacement_handoff_preserves_old_trigger_and_plays_new_session_source() {
    for mono in [false, true] {
        let Some((source, mut editor)) = source_fixture(mono) else {
            return;
        };
        let track = editor.project().tracks()[0].id();
        let session_clip = musical_clips(&mut editor, track);
        let media = BTreeMap::<hex_project::AssetId, crate::Audio>::new();
        let transforms = crate::audio_transform::AudioTransformPreparation::default();
        let first = Arc::new(
            PreparedRevision::prepare_with_audio_transforms_and_device_resources(
                editor.project(),
                &media,
                crate::SharedSineTable::new_on_control(),
                None,
                &transforms,
                DeviceResourcePreparation {
                    bank: &source,
                    max_scope_metadata_bytes: 65_536,
                },
            )
            .unwrap(),
        );
        let device = editor.project().track(track).unwrap().instrument().unwrap();
        let replacement = device
            .state()
            .clone()
            .with_asset_references(
                crate::native_device_resources::tests::state(
                    crate::native_device_resources::tests::ids()[1],
                )
                .asset_references()
                .to_vec(),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: device.id(),
                state: replacement,
            })
            .unwrap();
        // A genuinely independent bank prevents the new configuration from
        // accidentally keeping the predecessor's original PCM alive itself.
        let (replacement_source, replacement_editor) = source_fixture(mono).unwrap();
        drop(replacement_editor);
        let next = Arc::new(
            PreparedRevision::prepare_with_audio_transforms_and_device_resources(
                editor.project(),
                &media,
                crate::SharedSineTable::new_on_control(),
                Some(first.session()),
                &transforms,
                DeviceResourcePreparation {
                    bank: &replacement_source,
                    max_scope_metadata_bytes: 65_536,
                },
            )
            .unwrap(),
        );
        assert_ne!(pcm_address(&first, track), pcm_address(&next, track));
        let mut old = playback(first);
        old.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; 2];
        old.render(&mut prefix).unwrap();
        let original = crate::native_device_resources::tests::PCM;
        let original = if mono {
            original.map(|v| [v[0]; 2])
        } else {
            original
        };
        assert_eq!(prefix, original[..2]);
        let mut current = playback(Arc::clone(&next));
        let mut successor = playback(next);
        current.try_inherit_state_from(&mut old).unwrap();
        drop(old);
        drop(editor);
        drop(source);
        drop(replacement_source);
        current
            .apply(SessionPlaybackCommand::LaunchClip {
                clip: session_clip,
                quantization: LaunchQuantization::Immediate,
            })
            .unwrap();
        let mut output = [[0.0; 2]; 6];
        current.render(&mut output[..1]).unwrap();
        // Carry the still-sounding predecessor once more. Its old law/source
        // must survive both real moves, not merely a single graph replacement.
        successor.try_inherit_state_from(&mut current).unwrap();
        drop(current);
        successor.render(&mut output[1..]).unwrap();
        let add_new = |frame: [f32; 2]| frame.map(|sample| sample + 0.5);
        assert_eq!(
            output,
            [
                add_new(original[2]),
                add_new(original[3]),
                [0.5; 2],
                [0.5; 2],
                [0.0; 2],
                [0.0; 2]
            ]
        );
        eprintln!(
            "SUPPORTED actual Sampler replacement mono={mono}, old trigger PCM survives predecessor/bank retirement beside new Session source"
        );
    }
}
