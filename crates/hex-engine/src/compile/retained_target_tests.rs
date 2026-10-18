use super::*;
use crate::{audio_transform::AudioTransformPreparation, owned_shared::OwnedShared};

fn fixture() -> (hex_project::Editor, BTreeMap<AssetId, Audio>) {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let mut media = BTreeMap::new();
    for (index, value) in [0.25, 0.5].into_iter().enumerate() {
        let hex_project::Created::Imported { asset, .. } = editor
            .execute(hex_project::Command::ImportAudioTrack {
                name: format!("track {index}"),
                path: format!("source-{index}.wav").into(),
                sample_rate: 48_000,
                frames: 4,
            })
            .unwrap()
        else {
            panic!("import");
        };
        media.insert(asset, Audio::new(48_000, vec![[value, -value]; 4]).unwrap());
    }
    (editor, media)
}

#[test]
fn retained_compiler_target_keeps_completed_fields_after_later_resolution_error() {
    let (editor, media) = fixture();
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let transforms = AudioTransformPreparation::default();
    let mut contexts = Some(OwnedShared::new_owned(
        crate::shared_analytic::SharedAnalyticContextTable::default(),
    ));
    let context_pointer = Arc::as_ptr(contexts.as_ref().unwrap().arc());
    let mut destination = None;
    let mut resolutions = 0;
    let result = transforms.compile_transaction(|| {
        compile_project_into(
            editor.project(),
            |asset, rate| {
                resolutions += 1;
                if resolutions == 2 {
                    return Err(Invalid("injected later source refusal"));
                }
                resolve_resident(asset, rate, &media)
            },
            None,
            &effects,
            &instruments,
            &mut contexts,
            &transforms,
            &mut destination,
        )
    });
    assert!(matches!(
        result,
        Err(Invalid("injected later source refusal"))
    ));
    assert_eq!(resolutions, 2);
    assert!(contexts.is_none());
    let retained = destination.as_ref().unwrap();
    assert_eq!(Arc::as_ptr(retained.shared_contexts.arc()), context_pointer);
    assert_eq!(retained.tracks.len(), 1);
    let first = media.first_key_value().unwrap().1.frames().as_ptr();
    assert_eq!(
        retained.tracks[0].clips[0]
            .audio
            .as_ref()
            .unwrap()
            .frames()
            .as_ptr(),
        first
    );
    drop(media);
    assert_eq!(
        retained.tracks[0].clips[0].audio.as_ref().unwrap().frames(),
        &[[0.25, -0.25]; 4]
    );
    assert_eq!(editor.project().tracks().len(), 2);
    // This partial value is inspected and destroyed on control, never rendered.
}

#[test]
fn retained_compiler_target_success_matches_public_pcm_and_occupied_refusal_preserves_inputs() {
    let (editor, media) = fixture();
    let effects = hex_devices::native_registry().unwrap();
    let instruments = hex_devices::native_instrument_registry().unwrap();
    let transforms = AudioTransformPreparation::default();
    let mut contexts = None;
    let mut destination = None;
    transforms
        .compile_transaction(|| {
            compile_project_into(
                editor.project(),
                |asset, rate| resolve_resident(asset, rate, &media),
                None,
                &effects,
                &instruments,
                &mut contexts,
                &transforms,
                &mut destination,
            )
        })
        .unwrap();
    let public = project(editor.project(), &media).unwrap();
    let render = |project: &Project, block: usize| {
        let mut renderer = crate::Renderer::new(project);
        renderer.play();
        let mut pcm = [[0.0; 2]; 7];
        for chunk in pcm.chunks_mut(block) {
            renderer.render(chunk).unwrap();
        }
        pcm
    };
    let expected = [
        [0.75, -0.75],
        [0.75, -0.75],
        [0.75, -0.75],
        [0.75, -0.75],
        [0.0; 2],
        [0.0; 2],
        [0.0; 2],
    ];
    assert_eq!(render(&public, 7), expected);
    assert_eq!(render(destination.as_ref().unwrap(), 3), expected);
    let retained_pointer = destination.as_ref().unwrap().tracks.as_ptr();
    let retained_capacity = destination.as_ref().unwrap().tracks.capacity();
    contexts = Some(OwnedShared::new_owned(
        crate::shared_analytic::SharedAnalyticContextTable::default(),
    ));
    let input_pointer = Arc::as_ptr(contexts.as_ref().unwrap().arc());
    let result = transforms.compile_transaction(|| {
        compile_project_into(
            editor.project(),
            |_, _| panic!("occupied destination must refuse before resolver"),
            None,
            &effects,
            &instruments,
            &mut contexts,
            &transforms,
            &mut destination,
        )
    });
    assert!(matches!(
        result,
        Err(Invalid("compiler destination is not vacant"))
    ));
    assert_eq!(Arc::as_ptr(contexts.as_ref().unwrap().arc()), input_pointer);
    assert_eq!(
        destination.as_ref().unwrap().tracks.as_ptr(),
        retained_pointer
    );
    assert_eq!(
        destination.as_ref().unwrap().tracks.capacity(),
        retained_capacity
    );
    assert_eq!(render(destination.as_ref().unwrap(), 1), expected);
}
