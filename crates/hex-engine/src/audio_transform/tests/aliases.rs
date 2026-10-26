use super::*;

#[test]
fn aliases_preserve_pcm_local_fades_and_cached_content_after_make_unique() {
    let (mut editor, clip, media) = fixture(true);
    let context = AudioTransformPreparation::default();
    let original = prepare(&editor, &media, &context);
    let expected = render(&original, 0, 8_000, 127);
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 0.02));
    let Created::Clips { first: alias, last } = editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap()
    else {
        panic!("alias")
    };
    assert_eq!(alias, last);
    let duplicated = prepare(&editor, &media, &context);
    for block in [1, 17, 257] {
        let actual = render(&duplicated, 0, 16_000, block);
        assert_eq!(&actual[..8_000], expected);
        assert_eq!(&actual[8_000..], expected);
    }
    // Shared content edits affect both placements, while destination fades
    // remain independent. The original placement must stay byte-for-byte stable.
    editor
        .execute(Command::SetAudioClipPitch {
            clip: alias,
            cents: 137,
        })
        .unwrap();
    let pitched = prepare(&editor, &media, &context);
    let pitched_pcm = render(&pitched, 0, 16_000, 127);
    assert_ne!(&pitched_pcm[..8_000], expected);
    assert_eq!(&pitched_pcm[..8_000], &pitched_pcm[8_000..]);
    editor
        .execute(Command::SetClipFades {
            clip: alias,
            fade_in: 0,
            fade_out: 0,
        })
        .unwrap();
    let faded = prepare(&editor, &media, &context);
    let faded_pcm = render(&faded, 0, 16_000, 17);
    assert_eq!(&faded_pcm[..8_000], &pitched_pcm[..8_000]);
    assert_ne!(&faded_pcm[8_000..], &pitched_pcm[8_000..]);
    editor
        .execute(Command::MakeClipsUnique(vec![alias]))
        .unwrap();
    let unique = prepare(&editor, &media, &context);
    for block in [1, 17, 257] {
        assert_eq!(render(&unique, 0, 16_000, block), faded_pcm);
    }
    for (before, after) in faded.arrangement().tracks()[0]
        .clips
        .iter()
        .zip(&unique.arrangement().tracks()[0].clips)
    {
        assert!(
            before
                .audio
                .as_ref()
                .unwrap()
                .shares_pcm_with(after.audio.as_ref().unwrap())
        );
    }
    assert!(editor.undo());
    assert_eq!(
        render(&prepare(&editor, &media, &context), 0, 16_000, 257),
        faded_pcm
    );
}
