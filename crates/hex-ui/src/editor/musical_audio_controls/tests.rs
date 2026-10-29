use super::*;
#[test]
fn point_pages_clamp_to_current_clip_and_modes_dispatch_real_typed_commands() {
    assert_eq!(clamped_page(127, 2), 0);
    assert_eq!(clamped_page(127, 0), 0);
    assert_eq!(clamped_page(127, 9), 1);
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 32).unwrap();
    let hex_project::Created::Imported { clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "audio".into(),
            path: "audio.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(command(editor.project(), clip, Action::Warp).unwrap())
        .unwrap();
    for (index, expected) in modes().into_iter().enumerate() {
        editor
            .execute(command(editor.project(), clip, Action::SelectMode(index as u8)).unwrap())
            .unwrap();
        assert_eq!(
            std::mem::discriminant(
                &editor
                    .project()
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .domain
                    .warp
                    .as_ref()
                    .unwrap()
                    .mode
            ),
            std::mem::discriminant(&expected)
        );
    }
    editor
        .execute(command(editor.project(), clip, Action::MarkerAdd).unwrap())
        .unwrap();
    assert_eq!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .warp
            .as_ref()
            .unwrap()
            .markers
            .len(),
        3
    );
    editor
        .execute(command(editor.project(), clip, Action::EnvelopeAdd).unwrap())
        .unwrap();
    editor
        .execute(command(editor.project(), clip, Action::EnvelopeGain(0, -1)).unwrap())
        .unwrap();
    assert!(
        editor
            .project()
            .clip(clip)
            .unwrap()
            .audio_edit()
            .unwrap()
            .domain
            .source_gain[0]
            .gain_q16
            < 65_536
    );
    editor
        .execute(command(editor.project(), clip, Action::EnvelopeRemove(1)).unwrap())
        .unwrap();
    let retained = editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .source_gain[0];
    editor
        .execute(command(editor.project(), clip, Action::EnvelopeAdd).unwrap())
        .unwrap();
    let points = &editor
        .project()
        .clip(clip)
        .unwrap()
        .audio_edit()
        .unwrap()
        .domain
        .source_gain;
    assert_eq!(points.len(), 2);
    assert!(points.contains(&retained));
}

#[test]
fn numeric_pitch_keeps_absolute_single_and_relative_selection_units() {
    let mut model =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { clip, .. } = model
        .execute(Command::ImportAudioTrack {
            name: "pitch".into(),
            path: "pitch.wav".into(),
            sample_rate: 48_000,
            frames: 48_000,
        })
        .unwrap()
    else {
        panic!("import")
    };
    model
        .execute(Command::SetAudioClipPitch { clip, cents: 137 })
        .unwrap();
    assert!(
        matches!(pitch_command(model.project(),clip,237.,false).unwrap(),Command::SetAudioClipPitch {clip:id,cents:237} if id==clip)
    );
    assert!(
        matches!(pitch_command(model.project(),clip,237.,true).unwrap(),Command::EditClips {clips,edit:hex_project::ClipEdit::PitchBy {cents:100}} if clips==vec![clip])
    );
    for value in [f64::NAN, f64::INFINITY, -4801., 4801., 0.5] {
        assert!(pitch_command(model.project(), clip, value, false).is_err());
    }
    for value in [-4800., 0., 4800.] {
        let command = pitch_command(model.project(), clip, value, false).unwrap();
        let mut oracle = hex_project::Editor::new(model.project().clone(), 8).unwrap();
        oracle.execute(command).unwrap();
        assert_eq!(
            f64::from(
                oracle
                    .project()
                    .clip(clip)
                    .unwrap()
                    .audio_edit()
                    .unwrap()
                    .domain
                    .pitch_cents
            ),
            value
        );
    }
}
