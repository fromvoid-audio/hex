//! Original 100-track PCM regression; headless correctness, not a device deadline test.
#[path = "../examples/support/session_audio_fixture.rs"]
mod fixture;

use hex_engine::{
    LaunchQuantization, PlaybackAction, PlaybackCommand, PreparedRevision, SessionPlaybackCommand,
    SessionPlaybackOwner, SineTable, playback::Playback,
};
use hex_project::{BeatTime, Command, Editor, Tempo};
use std::{error::Error, sync::Arc};

fn launch(playback: &mut Playback, scene: hex_project::SceneId) -> Result<(), Box<dyn Error>> {
    playback
        .try_apply_action(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchScene {
                scene,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .map_err(|error| error.message())?;
    Ok(())
}

fn verify(distinct: bool) -> Result<(), Box<dyn Error>> {
    let fixture = fixture::fixture(distinct)?;
    let assets = if distinct { 100 } else { 1 };
    assert_eq!(
        fixture::unique_pcm_summary(&fixture.media),
        (
            assets,
            assets * fixture::SOURCE_FRAMES,
            assets * fixture::SOURCE_FRAMES * std::mem::size_of::<hex_engine::Frame>(),
        )
    );
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(PreparedRevision::prepare(
        &fixture.project,
        &fixture.media,
        Arc::clone(&table),
        None,
    )?);
    assert_eq!(revision.session().audio_clips().len(), 200);
    for clip in fixture.scene_a_clips.iter().chain(&fixture.scene_b_clips) {
        let canonical = fixture
            .project
            .session_audio_clip(*clip)
            .ok_or("canonical source")?;
        let prepared = revision
            .session()
            .audio_clip(*clip)
            .ok_or("prepared source")?;
        assert_eq!(
            prepared
                .resident_audio()
                .ok_or("resident source")?
                .frames()
                .as_ptr(),
            fixture.media[&canonical.asset()].frames().as_ptr()
        );
    }
    let mut playback = Playback::try_from_prepared_with_session(Arc::clone(&revision))?;
    launch(&mut playback, fixture.scene_a)?;
    let partitions = [1, 127, 31, 911, 3033, 5];
    let actual = fixture::render_partitions(&mut playback, &partitions)?;
    let expected = fixture::finite_session_oracle(
        &fixture.project,
        &fixture.media,
        fixture.scene_a,
        0,
        actual.len(),
    )?;
    assert_eq!(actual, expected);
    assert!(actual[4096..].iter().all(|frame| *frame == [0.0; 2]));
    assert_eq!(playback.session_route_count(), 100);
    for (index, track) in fixture.tracks.iter().enumerate() {
        let route = playback.session_route_status(index).ok_or("route")?;
        assert_eq!(route.track, *track);
        assert_eq!(
            route.owner,
            SessionPlaybackOwner::Session {
                clip: fixture.scene_a_clips[index]
            }
        );
    }

    // Relaunch at a nonzero performance frame: the PCM oracle is relative to
    // the new launch, not to the global transport clock.
    let launch_frame = playback.performance_frame();
    assert!(launch_frame > 4096);
    launch(&mut playback, fixture.scene_b)?;
    let actual = fixture::render_partitions(&mut playback, &[3, 29, 97, 5])?;
    assert_eq!(
        actual,
        fixture::finite_session_oracle(
            &fixture.project,
            &fixture.media,
            fixture.scene_b,
            0,
            actual.len()
        )?
    );
    assert_eq!(playback.performance_frame(), launch_frame + 134);

    let mut editor = Editor::new(fixture.project.clone(), 8)?;
    editor.execute(Command::SetTempoPoint {
        beat: BeatTime::ZERO,
        tempo: Tempo::new(60_000_000)?,
    })?;
    let candidate = Arc::new(PreparedRevision::prepare(
        editor.project(),
        &fixture.media,
        table,
        Some(revision.session()),
    )?);
    let mut next = Playback::try_from_prepared_with_session(candidate)?;
    next.try_inherit_runtime_from(&mut playback)
        .map_err(|error| format!("handoff failed: {error:?}"))?;
    let actual = fixture::render_partitions(&mut next, &[1, 31, 2])?;
    assert_eq!(
        actual,
        fixture::finite_session_oracle(
            editor.project(),
            &fixture.media,
            fixture.scene_b,
            134,
            actual.len()
        )?
    );
    for index in 0..100 {
        assert_eq!(
            next.session_route_status(index).ok_or("route")?.owner,
            SessionPlaybackOwner::Session {
                clip: fixture.scene_b_clips[index]
            }
        );
    }
    next.try_apply_action(PlaybackAction::Session(
        SessionPlaybackCommand::LaunchScene {
            scene: fixture.stop_scene,
            quantization: LaunchQuantization::Immediate,
        },
    ))
    .map_err(|error| error.message())?;
    // Every stopped clip fades out over the shared 128-frame declick from its
    // current source position (134 + 34 frames after launch).
    let released = fixture::render_partitions(&mut next, &[1, 7, 13])?;
    let sounding = fixture::finite_session_oracle(
        editor.project(),
        &fixture.media,
        fixture.scene_b,
        168,
        released.len(),
    )?;
    assert!(sounding.iter().any(|frame| *frame != [0.0; 2]));
    for (offset, (actual, full)) in released.iter().zip(&sounding).enumerate() {
        let gain = (128 - offset) as f32 / 128.0;
        for channel in 0..2 {
            let expected = full[channel] * gain;
            assert!(
                (actual[channel] - expected).abs() <= 1e-5 * expected.abs().max(1.0),
                "release frame {offset}: {actual:?} != {full:?} * {gain}"
            );
        }
    }
    for index in 0..100 {
        assert_eq!(
            next.session_route_status(index).ok_or("route")?.owner,
            SessionPlaybackOwner::Silence
        );
    }
    next.try_apply_action(PlaybackAction::Session(
        SessionPlaybackCommand::BackToArrangement {
            quantization: LaunchQuantization::Immediate,
        },
    ))
    .map_err(|error| error.message())?;
    next.try_apply_action(PlaybackAction::Transport(PlaybackCommand::Seek(0)))
        .map_err(|error| error.message())?;
    let actual = fixture::render_partitions(&mut next, &[1, 127, 43])?;
    assert_eq!(
        actual,
        fixture::finite_arrangement_oracle(editor.project(), &fixture.media, 0, actual.len())?
    );
    Ok(())
}

#[test]
fn hundred_tracks_shared_pcm_preserve_exact_routes_and_source_frames() -> Result<(), Box<dyn Error>>
{
    verify(false)
}

#[test]
fn hundred_tracks_distinct_pcm_preserve_exact_routes_and_source_frames()
-> Result<(), Box<dyn Error>> {
    verify(true)
}
