use super::*;
use hex_project::{ClipFade, Command, Editor};

#[test]
fn reserved_compiled_fade_survives_native_slot_and_metadata_installation() {
    if !super::super::session_source_tests::supported(
        "reserved_compiled_fade_survives_native_slot_and_metadata_installation",
    ) {
        return;
    }
    let (project, bank, _) = super::super::arrangement_clip_tests::fixture(false, false, 1);
    let clip = project.tracks()[0].clips()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 2,
            fade_out: 2,
        })
        .unwrap();
    let project = editor.project();
    let input = AudioArrangementInput::new(project, 11, bank).with_audio_clip_inventory();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let mut reserved = custody
        .reserve_stages_with_resources(
            project,
            11,
            &input.inputs,
            input.stages,
            ConstructorResources {
                sampler: None,
                effects: None,
                native_input: None,
            },
        )
        .ok()
        .unwrap();
    let slot = &mut reserved.slots[0];
    assert!(slot.reserve_fade);
    let fade = ClipFade::new(slot.end - slot.start, 0, 2, 2).unwrap();
    let audio = crate::Audio::new(project.sample_rate(), vec![[0.25; 2]; 8]).unwrap();
    let compiled = crate::Clip::new(audio, slot.start, slot.end - slot.start, false)
        .unwrap()
        .with_fade(fade)
        .unwrap();
    install_compiled_origin(project, project.clip(clip).unwrap(), slot, Some(compiled)).unwrap();
    assert_eq!(slot.fade, fade);
    let SourceKind::Resident(audio) = &slot.source_owner(&input.inputs).unwrap().kind else {
        panic!("resident")
    };
    slot.owner = Some(OwnedAudioSource {
        kind: SourceKind::Resident(audio.clone()),
        handle: slot.handle,
    });
    let before = custody.arrangement_clips.capacity(false).unwrap();
    assert_eq!(
        before.metadata_pointee_bytes,
        mem::size_of::<crate::ClipMetadata>()
    );
    custody
        .arrangement_clips
        .construct(&mut reserved.slots, &input.inputs, false, false, false)
        .unwrap();
    let compiled = custody.arrangement_clips.clips().next().unwrap();
    assert_eq!(compiled.metadata.as_ref().unwrap().fade, Some(fade));
    let graph = crate::Project::new(
        48_000,
        vec![crate::Track::new(vec![compiled.clone()], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = crate::Renderer::new(&graph);
    renderer.play();
    let mut actual = [[0.0; 2]; 8];
    renderer.render(&mut actual).unwrap();
    for (frame, sample) in actual.into_iter().enumerate() {
        assert_eq!(sample, [0.25 * fade.gain_at(frame as u64); 2]);
    }
}

#[test]
fn unreserved_compiled_fade_and_nonzero_phase_refuse_before_native_pcm() {
    if !super::super::session_source_tests::supported(
        "unreserved_compiled_fade_and_nonzero_phase_refuse_before_native_pcm",
    ) {
        return;
    }
    let (project, bank, _) = super::super::arrangement_clip_tests::fixture(false, false, 1);
    let clip = project.tracks()[0].clips()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    editor
        .execute(Command::SetAudioClipPitch { clip, cents: 100 })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let project = editor.project();
    let input = AudioArrangementInput::new(project, 11, bank).with_audio_clip_inventory();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let mut reserved = custody
        .reserve_stages_with_resources(
            project,
            11,
            &input.inputs,
            input.stages,
            ConstructorResources {
                sampler: None,
                effects: None,
                native_input: None,
            },
        )
        .ok()
        .unwrap();
    assert_eq!(
        custody
            .arrangement_clips
            .capacity(false)
            .unwrap()
            .metadata_pointee_bytes,
        0
    );
    let slot = &mut reserved.slots[0];
    assert!(!slot.reserve_fade);
    for case in 0..3 {
        let audio = crate::Audio::new(48_000, vec![[0.25; 2]; 8]).unwrap();
        let mut compiled = if case == 1 {
            crate::Clip::new(audio, slot.start, slot.end - slot.start, true)
                .unwrap()
                .with_source_phase(1)
                .unwrap()
        } else {
            crate::Clip::new(audio, slot.start, slot.end - slot.start, false)
                .unwrap()
                .with_fade(ClipFade::new(slot.end - slot.start, 0, 2, 2).unwrap())
                .unwrap()
        };
        if case == 2 {
            slot.reserve_fade = true;
            compiled.metadata.as_mut().unwrap().fade = Some(ClipFade::neutral(4).unwrap());
        }
        assert_eq!(
            install_compiled_origin(project, project.clip(clip).unwrap(), slot, Some(compiled)),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(slot.prepared_origin.is_none());
    }
}
