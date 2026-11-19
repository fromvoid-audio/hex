//! Finite whole-clip placement through actual publication, seek and callback.
use allocation_counter::measure;
use hex_audio::prepared::{self, PublicationPolicy};
use hex_engine::{
    Audio, Frame, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{AudioEventEdit, Command, Created, Editor, Project, TrimEdge};
use std::{
    collections::BTreeMap,
    error::Error,
    sync::{Arc, atomic::Ordering::Acquire},
};

fn value(frame: u64) -> Frame {
    let v = (frame % 251 + 1) as f32 / 2048.0;
    [v, -v * 0.5]
}
fn render(callback: &mut prepared::Callback, output: &mut [Frame]) -> Result<(), Box<dyn Error>> {
    let mut result = None;
    let stats = measure(|| {
        callback.begin_block();
        result = Some(callback.render(output));
    });
    result.ok_or("missing callback")??;
    assert_eq!(
        (
            stats.count_total,
            stats.bytes_total,
            stats.count_current,
            stats.bytes_current
        ),
        (0, 0, 0, 0)
    );
    Ok(())
}
#[test]
fn moved_range_is_silent_outside_clip_then_revealed_with_original_pcm_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    const DESTINATION: u64 = hex_project::MAX_AUDIO_DOMAIN_FRAMES + 4096;
    for block in [1, 7, 64, 512] {
        let mut editor = Editor::new(Project::new(48_000)?, 8)?;
        let Created::Imported { clip, asset, .. } = editor.execute(Command::ImportAudioTrack {
            name: "source".into(),
            path: "original.wav".into(),
            sample_rate: 48_000,
            frames: 8192,
        })?
        else {
            return Err("import".into());
        };
        editor.execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 2048 },
        })?;
        let media = BTreeMap::from([(asset, Audio::new(48_000, (0..8192).map(value).collect())?)]);
        let prepare = |e: &Editor| -> Result<Playback, Box<dyn Error>> {
            Ok(Playback::try_from_prepared_with_session(Arc::new(
                PreparedRevision::prepare(e.project(), &media, Arc::new(SineTable::new()), None)?,
            ))?)
        };
        let (mut control, mut callback) = prepared::channel(48_000)?;
        control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "publication")?;
        control.command(PlaybackCommand::Play).map_err(|_| "play")?;
        let mut output = vec![[0.0; 2]; block];
        render(&mut callback, &mut output)?;
        control.reclaim();
        let running_position = control.status.position.load(Acquire);
        editor.execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::PlaceRange {
                start: 0,
                end: 2048,
                destination: DESTINATION as i64,
            },
        })?;
        let revision = control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "placement publication")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            control.status.position.load(Acquire),
            running_position + (1024_usize.div_ceil(block) * block) as u64,
            "placement must retain the live transport cursor"
        );
        assert!(
            output.iter().all(|f| *f == [0.0; 2]),
            "out-of-window event must remain silent, block{block}"
        );
        control.reclaim();
        control
            .command(PlaybackCommand::PlayFrom(2500))
            .map_err(|_| "play untouched")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        let end = control.status.position.load(Acquire);
        for (i, frame) in output.iter().enumerate() {
            assert_eq!(
                *frame,
                value(end - block as u64 + i as u64),
                "untouched source, block{block}"
            );
        }
        editor.execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Right,
            frame: DESTINATION + 2048,
        })?;
        let revision = control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "widen publication")?;
        render(&mut callback, &mut output)?;
        control.reclaim();
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        control
            .command(PlaybackCommand::PlayFrom(DESTINATION))
            .map_err(|_| "play revealed")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        let end = control.status.position.load(Acquire);
        for (i, frame) in output.iter().enumerate() {
            assert_eq!(
                *frame,
                value(end - block as u64 + i as u64 - DESTINATION),
                "revealed source, block{block}"
            );
        }
        control.reclaim();
    }
    Ok(())
}

#[test]
fn negative_event_retains_live_cursor_and_original_pcm_on_left_extension_without_callback_allocations()
-> Result<(), Box<dyn Error>> {
    for block in [1, 7, 64, 512] {
        let mut editor = Editor::new(Project::new(48_000)?, 8)?;
        let Created::Imported { clip, asset, track } =
            editor.execute(Command::ImportAudioTrack {
                name: "source".into(),
                path: "original.wav".into(),
                sample_rate: 48_000,
                frames: 8192,
            })?
        else {
            return Err("import".into());
        };
        editor.execute(Command::MoveClip {
            clip,
            track,
            start: 8192,
        })?;
        editor.execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Split { at: 2048 },
        })?;
        let media = BTreeMap::from([(asset, Audio::new(48_000, (0..8192).map(value).collect())?)]);
        let prepare = |e: &Editor| -> Result<Playback, Box<dyn Error>> {
            Ok(Playback::try_from_prepared_with_session(Arc::new(
                PreparedRevision::prepare(e.project(), &media, Arc::new(SineTable::new()), None)?,
            ))?)
        };
        let (mut control, mut callback) = prepared::channel(48_000)?;
        control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "publication")?;
        control
            .command(PlaybackCommand::PlayFrom(8192))
            .map_err(|_| "play")?;
        let mut output = vec![[0.0; 2]; block];
        render(&mut callback, &mut output)?;
        control.reclaim();
        let running_position = control.status.position.load(Acquire);
        editor.execute(Command::EditAudioEvents {
            clip,
            operation: AudioEventEdit::Place {
                event: 1,
                start: -4096,
            },
        })?;
        let revision = control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "placement publication")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(
            control.status.position.load(Acquire),
            running_position + (1024_usize.div_ceil(block) * block) as u64,
            "placement must retain the live transport cursor"
        );
        assert!(
            output.iter().all(|f| *f == [0.0; 2]),
            "out-of-window event must remain silent, block{block}"
        );
        control.reclaim();
        control
            .command(PlaybackCommand::PlayFrom(8192 + 2500))
            .map_err(|_| "play untouched")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        let end = control.status.position.load(Acquire);
        for (i, frame) in output.iter().enumerate() {
            assert_eq!(
                *frame,
                value(end - block as u64 + i as u64 - 8192),
                "untouched source, block{block}"
            );
        }
        editor.execute(Command::TrimClipEdge {
            clip,
            edge: TrimEdge::Left,
            frame: 4096,
        })?;
        let revision = control
            .publish_with_policy_and_context_generation(
                prepare(&editor)?,
                PublicationPolicy::InheritRuntime,
                1,
            )
            .map_err(|_| "widen publication")?;
        render(&mut callback, &mut output)?;
        control.reclaim();
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        control
            .command(PlaybackCommand::PlayFrom(4096))
            .map_err(|_| "play revealed")?;
        for _ in 0..1024_usize.div_ceil(block) {
            render(&mut callback, &mut output)?;
        }
        let end = control.status.position.load(Acquire);
        for (i, frame) in output.iter().enumerate() {
            assert_eq!(
                *frame,
                value(end - block as u64 + i as u64 - 4096),
                "revealed source, block{block}"
            );
        }
        control.reclaim();
    }
    Ok(())
}
