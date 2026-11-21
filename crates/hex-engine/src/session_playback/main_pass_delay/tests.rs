use super::*;
use crate::session_playback::SessionPlaybackCommand;
use std::{
    collections::BTreeMap,
    mem::{align_of, size_of},
    sync::Arc,
};

fn runtime() -> SessionPlayback {
    let project = hex_project::Project::new(48_000).unwrap();
    let revision = crate::PreparedRevision::prepare(
        &project,
        &BTreeMap::new(),
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap();
    SessionPlayback::new(Arc::new(revision)).unwrap()
}

#[test]
fn inline_return_prefix_leaves_the_rest_of_the_render_untouched_and_ends_on_seek() {
    const FRAMES: usize = HOSTED_RETURN_BRIDGE_FRAMES as usize;
    let old_pcm = std::array::from_fn::<_, FRAMES, _>(|frame| {
        let value = frame as f32 / FRAMES as f32;
        [value, -value]
    });
    let prefix = MainPass::ReturnOnPrefix {
        ring: MainPassDelay::copy_committed_ring(&old_pcm, 0).unwrap(),
        remaining: FRAMES,
    };
    let mut playback = runtime();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    let unprefixed = [0.25, 0.5];
    let mut output = [unprefixed; 200];
    playback.apply_main_pass(&mut output);
    assert_eq!(output, [unprefixed; 200]);
    playback.main_pass_delay = Some(prefix);
    assert_eq!(playback.main_output_latency_frames(), 0);
    for section in 0..3 {
        output.fill(unprefixed);
        playback.apply_main_pass(&mut output);
        for (offset, actual) in output.iter().enumerate() {
            let frame = section * output.len() + offset;
            assert_eq!(*actual, old_pcm.get(frame).copied().unwrap_or(unprefixed));
        }
    }
    assert!(playback.main_pass_delay.is_none());

    playback.main_pass_delay = Some(prefix);
    playback.apply_main_pass(&mut output);
    playback.apply(SessionPlaybackCommand::Seek(24)).unwrap();
    assert!(playback.main_pass_delay.is_none());
    output.fill(unprefixed);
    playback.apply_main_pass(&mut output);
    assert_eq!(output, [unprefixed; 200]);
}

#[test]
fn main_pass_inline_storage_can_hold_at_most_one_pcm_ring() {
    let previous = size_of::<Option<MainPassDelay>>();
    let current = size_of::<Option<MainPass>>();
    let metadata = size_of::<HostedReturnPowerOn>() + align_of::<MainPass>();
    assert!(current <= previous + metadata);
    println!(
        "main pass field bytes before={previous} after={current} session_box={}",
        size_of::<SessionPlayback>()
    );
}
