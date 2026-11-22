use hex_project::{
    BeatTime, Command, Created, Editor, Project, WarpMarker, WarpMode, WarpSettings,
};

#[test]
fn converted_warped_aliases_round_trip_in_the_existing_storage_format() {
    let mut editor = Editor::new(Project::new(44_100).unwrap(), 8).unwrap();
    let Created::Imported { asset, clip, .. } = editor
        .execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "Untouched.wav".into(),
            sample_rate: 44_100,
            frames: 88_200,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    editor
        .execute(Command::SetAudioClipWarp {
            clip,
            settings: Some(WarpSettings {
                mode: WarpMode::RePitch,
                markers: vec![
                    WarpMarker {
                        source_frame: 0,
                        beat: BeatTime::ZERO,
                    },
                    WarpMarker {
                        source_frame: 22_050,
                        beat: BeatTime::ONE,
                    },
                    WarpMarker {
                        source_frame: 88_200,
                        beat: BeatTime::from_quarters(4).unwrap(),
                    },
                ],
            }),
        })
        .unwrap();
    editor
        .execute(Command::SetClipFades {
            clip,
            fade_in: 441,
            fade_out: 882,
        })
        .unwrap();
    editor
        .execute(Command::DuplicateAliasClips(vec![clip]))
        .unwrap();
    let original = editor.project();
    let converted = original.at_sample_rate(48_000).unwrap();
    assert_eq!(converted.clip_alias(clip), original.clip_alias(clip));
    let edit = converted.clip(clip).unwrap().audio_edit().unwrap();
    assert_eq!(
        edit.domain.warp.as_ref().unwrap().markers[1].source_frame,
        24_000
    );
    assert_eq!(edit.domain.fade.fade_in(), 480);
    assert_eq!(
        converted.asset(asset).unwrap().path(),
        original.asset(asset).unwrap().path()
    );
    let reopened = hex_storage::decode(&hex_storage::encode(&converted).unwrap()).unwrap();
    assert_eq!(reopened, converted);
    assert_eq!(reopened.at_sample_rate(44_100).unwrap(), *original);
}
