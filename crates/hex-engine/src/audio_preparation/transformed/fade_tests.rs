use super::*;
use hex_project::{ClipFade, Command, Editor};

fn phased_frame_fixture() -> (hex_project::Project, OwnedAudioInputs, [crate::Frame; 8]) {
    let (project, bank, _) = super::super::arrangement_clip_tests::fixture(false, false, 1);
    let source: [crate::Frame; 8] =
        std::array::from_fn(|index| [index as f32 / 32.0, -(index as f32) / 64.0]);
    let mut bank = OwnedAudioInputs::try_new(
        vec![(
            project.assets().next().unwrap().id(),
            OwnedAudioSource::try_resident(project.sample_rate(), source.to_vec())
                .ok()
                .unwrap(),
        )],
        bank.reservation,
    )
    .ok()
    .unwrap();
    bank.reservation.handle_obligations = 64;
    let clip_id = project.tracks()[0].clips()[0].id();
    let mut editor = Editor::new(project, 8).unwrap();
    for cents in [1, 0] {
        editor
            .execute(Command::SetAudioClipPitch {
                clip: clip_id,
                cents,
            })
            .unwrap();
    }
    let mut snapshot = editor.project().snapshot();
    let row = &mut snapshot.tracks[0].audio_clips_mut().unwrap()[0];
    let edit = row.edit.as_mut().unwrap();
    edit.view_start = 1;
    edit.view_end = 5;
    row.duration = 12;
    row.fade = ClipFade::neutral(12).unwrap();
    edit.placement = Some(hex_project::AudioViewPlacement {
        mode: hex_project::AudioViewMode::Repeat,
        outer_length: hex_project::AudioDomainClock::Frames(12),
        cycle_phase: 2,
    });
    let project = hex_project::Project::from_snapshot(snapshot).unwrap();
    (project, bank, source)
}

#[test]
fn reserved_compiled_fade_survives_native_slot_and_metadata_installation() {
    if !super::super::session_source_tests::supported(
        "reserved_compiled_fade_survives_native_slot_and_metadata_installation",
    ) {
        return;
    }
    let (project, bank, source) = phased_frame_fixture();
    let clip = project.tracks()[0].clips()[0].id();
    let mut snapshot = project.snapshot();
    let edit = snapshot.tracks[0].audio_clips_mut().unwrap()[0]
        .edit
        .as_mut()
        .unwrap();
    std::sync::Arc::make_mut(&mut edit.domain).fade = ClipFade::new(8, 0, 2, 2).unwrap();
    let project = &hex_project::Project::from_snapshot(snapshot).unwrap();
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
    assert_eq!(slot.reserved_phase.map(std::num::NonZeroU64::get), Some(2));
    let fade = ClipFade::new(slot.end - slot.start, 0, 2, 2).unwrap();
    let audio = crate::Audio::new(project.sample_rate(), source.to_vec()).unwrap();
    let compiled = crate::Clip::region(audio, slot.start, slot.end - slot.start, 1, 4, true)
        .unwrap()
        .with_source_phase(2)
        .unwrap()
        .with_fade(fade)
        .unwrap();
    let wrong_phase = compiled.clone().with_source_phase(1).unwrap();
    assert_eq!(
        install_compiled_origin(
            project,
            project.clip(clip).unwrap(),
            slot,
            Some(wrong_phase)
        ),
        Err(AudioStorageError::SourceMismatch)
    );
    assert!(slot.prepared_origin.is_none());
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
    let metadata = compiled.metadata.as_ref().unwrap();
    assert_eq!(metadata.fade, Some(fade));
    assert_eq!(metadata.source_phase, 2);
    let graph = crate::Project::new(
        48_000,
        vec![crate::Track::new(vec![compiled.clone()], 1.0, false).unwrap()],
    )
    .unwrap();
    let mut renderer = crate::Renderer::new(&graph);
    renderer.play();
    let mut actual = [[0.0; 2]; 12];
    renderer.render(&mut actual).unwrap();
    for (frame, sample) in actual.into_iter().enumerate() {
        assert_eq!(
            sample,
            source[1 + (2 + frame) % 4].map(|value| value * fade.gain_at(frame as u64))
        );
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

#[test]
fn native_repeat_preparation_preserves_midcycle_source_phase_and_seek() {
    if !super::super::session_source_tests::supported("native_frame_repeat_phase") {
        return;
    }
    let (project, bank, source) = phased_frame_fixture();
    let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
    let token = custody
        .begin_input_on_control(
            AudioArrangementInput::new(&project, 11, bank).with_audio_prepared_revision(),
        )
        .ok()
        .unwrap();
    assert_eq!(
        custody.phase(),
        AudioPreparationPhase::ArrangementAndSessionSources,
        "preparation refusal: {:?}",
        custody.reason()
    );
    let revision = custody.revision.owner_for_test().unwrap().as_ref();
    let clip = &revision.arrangement().tracks()[0].clips[0];
    assert_eq!(clip.metadata.as_ref().unwrap().source_phase, 2);
    assert_eq!(
        custody
            .capacity_report()
            .arrangement_clips
            .metadata_pointee_bytes,
        mem::size_of::<crate::ClipMetadata>()
    );
    let mut renderer = crate::Renderer::new(revision.arrangement());
    renderer.play();
    let mut actual = [[0.0; 2]; 12];
    for block in actual.chunks_mut(3) {
        renderer.render(block).unwrap();
    }
    let expected: [_; 12] = std::array::from_fn(|n| source[1 + (2 + n) % 4]);
    assert_eq!(actual, expected);
    renderer.seek(5);
    let mut seek = [[0.0; 2]; 5];
    renderer.render(&mut seek).unwrap();
    assert_eq!(seek, expected[5..10]);
    assert!(custody.cancel(&token));
    custody.cleanup_on_control();
    assert!(custody.recover_on_control(&token).is_some());
}
