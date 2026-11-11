//! Published audible playhead with a latent Main chain, through the real
//! prepared callback; no native device or OS audio.
use hex_audio::prepared;
use hex_engine::{
    Audio, PreparedRevision, SineTable,
    playback::{Playback, PlaybackCommand},
};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::sync::atomic::Ordering::Relaxed;
use std::{collections::BTreeMap, sync::Arc};

const LATENCY: u64 = 288;
/// The queued Pause/Stop declick at 48 kHz (#4151).
const FADE: u64 = 128;

/// 48 kHz project with a 288-frame Main limiter and an optional loop.
fn revision(loop_region: Option<(u64, u64)>) -> Arc<PreparedRevision> {
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 16).unwrap();
    let Created::Imported { asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "audible".into(),
            path: "memory/audible.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 256,
            playback_frames: 256,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let definition = &hex_devices::limiter::DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    if let Some((start, end)) = loop_region {
        editor
            .execute(Command::SetLoopRegion {
                start,
                end,
                enabled: true,
            })
            .unwrap();
    }
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![[0.0; 2]; 256]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::new(SineTable::new()), None)
            .unwrap(),
    );
    assert_eq!(
        u64::from(revision.arrangement().main_latency_frames()),
        LATENCY
    );
    revision
}

fn start(revision: &Arc<PreparedRevision>) -> (prepared::Control, prepared::Callback) {
    let (mut control, callback) = prepared::channel(48_000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap())
        .unwrap();
    (control, callback)
}

/// Render `frames` in `block`-sized callbacks.
fn render(callback: &mut prepared::Callback, frames: usize, block: usize) {
    let mut output = vec![[0.0; 2]; block];
    let mut left = frames;
    while left > 0 {
        let count = block.min(left);
        callback.begin_block();
        callback.render(&mut output[..count]).unwrap();
        left -= count;
    }
}

fn positions(control: &prepared::Control) -> (u64, u64) {
    let status = &control.status;
    (
        status.position.load(Relaxed),
        status.audible_position.load(Relaxed),
    )
}

#[test]
fn audible_playhead_trails_main_latency_and_follows_a_loop_wrap() {
    // Loop [1,000, 2,000): the scheduled cursor wraps at 2,000.
    let looped = revision(Some((1_000, 2_000)));
    for block in [1, 63, 256, 512] {
        let (mut control, mut callback) = start(&looped);
        control.command(PlaybackCommand::Seek(1_500)).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        // Fewer than L frames since the seek: Main output is still the reset
        // silence, so the playhead holds the frame playback started from.
        render(&mut callback, 100, block);
        assert_eq!(positions(&control), (1_600, 1_500), "block {block}");
        // Steady state: scheduled minus L.
        render(&mut callback, 300, block);
        assert_eq!(
            positions(&control),
            (1_900, 1_900 - LATENCY),
            "block {block}"
        );
        // Scheduled has wrapped to 1,050; what you hear is still before the
        // loop end: 2,000 - (288 - 50).
        render(&mut callback, 150, block);
        assert_eq!(positions(&control), (1_050, 1_762), "block {block}");
        // Once the wrap itself is L frames old, audible is past it too.
        render(&mut callback, LATENCY as usize, block);
        assert_eq!(positions(&control), (1_338, 1_050), "block {block}");
    }
}

#[test]
fn seek_stop_and_pause_publish_defined_audible_positions() {
    let linear = revision(None);
    for block in [64, 480] {
        let (mut control, mut callback) = start(&linear);
        control.command(PlaybackCommand::Play).unwrap();
        render(&mut callback, 960, block);
        assert_eq!(positions(&control), (960, 960 - LATENCY), "block {block}");
        // Pause fades over FADE frames, then freezes the audible frame; it
        // does not jump to scheduled.
        control.command(PlaybackCommand::Pause).unwrap();
        render(&mut callback, 960, block);
        assert_eq!(
            positions(&control),
            (960 + FADE, 960 + FADE - LATENCY),
            "block {block}"
        );
        // A seek while paused shows the new position at once.
        control.command(PlaybackCommand::Seek(48_000)).unwrap();
        render(&mut callback, block, block);
        assert_eq!(positions(&control), (48_000, 48_000), "block {block}");
        control.command(PlaybackCommand::Play).unwrap();
        render(&mut callback, 960, block);
        assert_eq!(
            positions(&control),
            (48_960, 48_960 - LATENCY),
            "block {block}"
        );
        // Stop rewinds both at the end of its fade.
        control.command(PlaybackCommand::Stop).unwrap();
        render(&mut callback, FADE as usize, block);
        assert_eq!(positions(&control), (0, 0), "block {block}");
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn audible_position_publication_allocates_and_drops_nothing() {
    use allocation_counter::measure;
    let looped = revision(Some((1_000, 2_000)));
    let (mut control, mut callback) = start(&looped);
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 256];
    // Includes a wrap, a seek and a stop.
    for command in [
        None,
        Some(PlaybackCommand::Seek(1_900)),
        None,
        None,
        Some(PlaybackCommand::Stop),
    ] {
        if let Some(command) = command {
            control.command(command).unwrap();
        }
        for _ in 0..4 {
            let info = measure(|| {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            });
            assert_eq!(
                (
                    info.count_total,
                    info.bytes_total,
                    info.count_current,
                    info.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
    }
    assert_eq!(positions(&control), (0, 0));
    control.request_shutdown();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    control.reclaim();
}
