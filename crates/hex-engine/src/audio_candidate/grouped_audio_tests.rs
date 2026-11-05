//! Native constructor closure and PCM for finite retained-source clips.
use super::*;
use crate::{Audio, AudioSourceContext, PlaybackAction, PlaybackCommand, PreparedRevision};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

fn fixture() -> (
    Editor,
    Vec<hex_project::ClipId>,
    BTreeMap<hex_project::AssetId, Audio>,
) {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let mut ids = Vec::new();
    let mut media = BTreeMap::new();
    for (index, frames) in [64, 48, 32].into_iter().enumerate() {
        let Created::Imported { clip, asset, .. } = editor
            .execute(Command::ImportAudioTrack {
                name: format!("source {index}"),
                path: format!("{index}.wav").into(),
                sample_rate: 48_000,
                frames,
            })
            .unwrap()
        else {
            panic!("import")
        };
        ids.push(clip);
        media.insert(
            asset,
            Audio::new(
                48_000,
                (0..frames)
                    .map(|n| [(n as f32 + 1.0) / 256.0, -(index as f32 + 1.0) / 8.0])
                    .collect(),
            )
            .unwrap(),
        );
    }
    let mut snapshot = editor.project().snapshot();
    let mut second = snapshot.tracks[1].audio_clips_mut().unwrap().remove(0);
    second.start = 96;
    second.duration = 80;
    second.source_offset = 3;
    second.source_length = 23;
    second.looping = true;
    second.source_phase = 7;
    second.gain = hex_project::ClipGain::new(-2_321).unwrap();
    second.fade = hex_project::ClipFade::new(80, 0, 13, 11).unwrap();
    let mut third = snapshot.tracks[2].audio_clips_mut().unwrap().remove(0);
    third.start = 224;
    snapshot.tracks[0]
        .audio_clips_mut()
        .unwrap()
        .extend([second, third]);
    snapshot.tracks.truncate(1);
    let mut editor = Editor::new(Project::from_snapshot(snapshot).unwrap(), 16).unwrap();
    editor
        .execute(Command::EditAudioEvents {
            clip: ids[0],
            operation: hex_project::AudioEventEdit::Delete { start: 11, end: 23 },
        })
        .unwrap();
    editor
        .execute(Command::SetAudioClipGain {
            clip: ids[0],
            millidb: -1_777,
        })
        .unwrap();
    (editor, ids, media)
}
fn semantic(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
    start: u64,
) -> Vec<crate::Frame> {
    let revision = PreparedRevision::prepare_with_audio_transforms(
        project,
        media,
        Arc::new(crate::SineTable::new()),
        None,
        &crate::audio_transform::AudioTransformPreparation::default(),
    )
    .unwrap();
    let mut renderer = crate::Renderer::new(revision.arrangement());
    renderer.seek(start);
    renderer.play();
    let mut pcm = vec![[0.0; 2]; 256];
    for block in pcm.chunks_mut(17) {
        renderer.render(block).unwrap();
    }
    pcm
}
fn candidate(
    project: &Project,
    media: &BTreeMap<hex_project::AssetId, Audio>,
) -> (
    AudioCandidatePreparation,
    AudioCandidateReservationQuote,
    crate::AudioStorageReservation,
) {
    let quote = AudioCandidateReservationQuote::for_project(project, media.len()).unwrap();
    let bytes = media
        .values()
        .map(|a| std::mem::size_of_val(a.frames()))
        .sum();
    let limits = quote.storage_reservation(bytes).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(media.len(), limits).unwrap();
    for (asset, audio) in media {
        cell.copy_resident_source_on_control(*asset, audio).unwrap();
    }
    cell.prepare_on_control(
        project,
        11,
        AudioSourceContext {
            project_generation: 3,
            editor_revision: 7,
        },
    )
    .unwrap();
    cell.owner_on_control()
        .unwrap()
        .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
            data_rows: quote.data_rows,
            handle_rows: quote.handle_rows,
            root_rows: 8,
            max_retained_bytes: limits.max_prepared_data_bytes,
            max_construction_peak_bytes: limits.max_construction_data_bytes,
        })
        .unwrap();
    let originals: u64 = media
        .values()
        .map(|audio| audio.frames().len() as u64)
        .sum();
    assert_eq!(
        cell.resident_pcm_capacity_frames().unwrap(),
        originals + quote.prepared_pcm_frames
    );
    (cell, quote, limits)
}
#[test]
fn native_group_constructor_seals_exact_pcm_and_control_cleanup_across_history() {
    assert!(crate::audio_storage_constructor_layout_supported());
    let (mut editor, ids, media) = fixture();
    let mut original = editor.project().snapshot();
    let expected = semantic(editor.project(), &media, 0);
    let untouched = editor.project().clip(ids[2]).unwrap().clone();
    let Created::Clip(group) = editor
        .execute(Command::GroupClips(ids[..2].to_vec()))
        .unwrap()
    else {
        panic!("group")
    };
    assert_eq!(editor.project().clip(ids[2]), Some(&untouched));
    for start in [0, 19, 82, 127] {
        let (mut cell, _, _) = candidate(editor.project(), &media);
        let owner = cell.owner_on_control().unwrap();
        owner
            .test_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(start)))
            .unwrap();
        let mut actual = vec![[0.0; 2]; 256];
        for block in actual.chunks_mut(31) {
            owner.test_render(block).unwrap();
        }
        assert_eq!(actual, semantic(editor.project(), &media, start));
        if start == 0 {
            assert_eq!(actual, expected);
        }
        cell.cleanup_on_control().unwrap();
        assert!(!cell.failed_incomplete());
    }
    assert!(editor.undo());
    original.next_id = editor.project().snapshot().next_id;
    assert_eq!(editor.project().snapshot(), original);
    let (mut undo, _, _) = candidate(editor.project(), &media);
    undo.cleanup_on_control().unwrap();
    assert!(editor.redo());
    assert!(editor.project().clip(group).is_some());
    let (mut redo, _, _) = candidate(editor.project(), &media);
    redo.cleanup_on_control().unwrap();
}
#[test]
fn native_group_missing_child_refuses_and_preserves_collected_bank_for_control_cleanup() {
    let (mut editor, ids, media) = fixture();
    editor
        .execute(Command::GroupClips(ids[..2].to_vec()))
        .unwrap();
    let quote = AudioCandidateReservationQuote::for_project(editor.project(), media.len()).unwrap();
    let limits = quote.storage_reservation(8_192).unwrap();
    let mut cell = AudioCandidatePreparation::try_new_on_control(media.len(), limits).unwrap();
    let (asset, audio) = media.first_key_value().unwrap();
    cell.copy_resident_source_on_control(*asset, audio).unwrap();
    assert!(
        cell.prepare_on_control(
            editor.project(),
            11,
            AudioSourceContext {
                project_generation: 0,
                editor_revision: 1
            }
        )
        .is_err()
    );
    cell.cleanup_on_control().unwrap();
    assert!(!cell.failed_incomplete());
    // No source owner or project mutation was accepted from this refused copy.
    assert_eq!(audio.frames().len(), 64);
}

#[test]
fn directed_finite_and_grouped_native_origins_quote_and_render_the_actual_output() {
    for grouped in [false, true] {
        let (mut editor, ids, media) = fixture();
        let clip = if grouped {
            let Created::Clip(group) = editor
                .execute(Command::GroupClips(ids[..2].to_vec()))
                .unwrap()
            else {
                panic!("group");
            };
            group
        } else {
            ids[0]
        };
        let Created::Clip(tail) = editor
            .execute(Command::SplitClip { clip, frame: 31 })
            .unwrap()
        else {
            panic!("split");
        };
        editor.execute(Command::RemoveClip(tail)).unwrap();
        editor.execute(Command::ReverseClips(vec![clip])).unwrap();
        let expected_frames: u64 = editor
            .project()
            .tracks()
            .iter()
            .flat_map(|t| t.clips())
            .filter(|c| c.audio_edit().is_some())
            .map(|c| c.duration())
            .sum();
        assert_eq!(
            crate::audio_preparation::transformed::prepared_pcm_frames(editor.project()).unwrap(),
            expected_frames
        );
        for start in [0, 7, 30] {
            let (mut cell, _, _) = candidate(editor.project(), &media);
            let owner = cell.owner_on_control().unwrap();
            owner
                .test_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(start)))
                .unwrap();
            let mut actual = vec![[0.0; 2]; 256];
            for block in actual.chunks_mut(17) {
                owner.test_render(block).unwrap();
            }
            assert_eq!(actual, semantic(editor.project(), &media, start));
            cell.cleanup_on_control().unwrap();
            assert!(!cell.failed_incomplete());
        }
    }
}

#[test]
fn native_constructor_group_track_routes_resident_pcm_and_releases_custody() {
    let (mut editor, _, media) = fixture();
    let track = editor.project().tracks()[0].id();
    let Created::Track(group) = editor
        .execute(Command::GroupTracks {
            tracks: hex_project::TrackSelectionIds::new(&[track]).unwrap(),
            name: "group".into(),
        })
        .unwrap()
    else {
        panic!("group")
    };
    editor
        .execute(Command::Gain {
            track: group,
            gain: 0.5,
        })
        .unwrap();
    let expected = semantic(editor.project(), &media, 0);
    let (mut cell, _, _) = candidate(editor.project(), &media);
    let owner = cell.owner_on_control().unwrap();
    owner
        .test_apply_action(PlaybackAction::Transport(PlaybackCommand::PlayFrom(0)))
        .unwrap();
    let mut actual = vec![[0.0; 2]; 256];
    for block in actual.chunks_mut(31) {
        owner.test_render(block).unwrap();
    }
    assert_eq!(actual, expected);
    cell.cleanup_on_control().unwrap();
    assert!(!cell.failed_incomplete());
}
