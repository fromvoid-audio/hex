//! Actual callback sparse-envelope PCM and allocator regression. Construction,
//! publication, source ownership and final reclamation stay outside measurement.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Frame, compiled_fade_conformance as fixture,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{ClipFade, ClipGain};
use std::{error::Error, sync::atomic::Ordering::Acquire};

#[test]
fn sparse_fade_callback_uses_outer_clock_without_allocating_or_freeing()
-> Result<(), Box<dyn Error>> {
    let fade = ClipFade::new(fixture::DURATION + 9, 9, 16, 4)?;
    let gain = ClipGain::new(3_000)?;
    for block in [1, 7, 64, 257] {
        let graph = Playback::new(fixture::sparse_project(fade, gain)?);
        let (mut control, mut callback) = prepared::channel(48_000)?;
        control.publish(graph).map_err(|_| "publication")?;
        let first = fixture::START + fixture::DESTINATION - 512;
        control
            .command(PlaybackCommand::PlayFrom(first))
            .map_err(|_| "play")?;
        let mut output = vec![[0.0; 2]; block];
        let mut witnessed = 0;
        for _ in 0..1024_usize.div_ceil(block) {
            let mut result = Ok(());
            let stats = measure(|| {
                callback.begin_block();
                result = callback.render(&mut output);
            });
            result?;
            assert_eq!(
                (
                    stats.count_total,
                    stats.bytes_total,
                    stats.count_current,
                    stats.bytes_current
                ),
                (0, 0, 0, 0)
            );
            let end = control.status.position.load(Acquire);
            let start = end - block as u64;
            for (i, sample) in output.iter().enumerate() {
                let position = start + i as u64;
                // Initial transport smoothing has settled during the silent gap.
                if position < first + 256 {
                    continue;
                }
                let elapsed = position - fixture::START;
                let audible = (fixture::DESTINATION..fixture::DURATION).contains(&elapsed);
                let expected: Frame = if audible {
                    witnessed += 1;
                    fixture::SOURCE.map(|value| value * fade.gain_at(elapsed) * gain.linear())
                } else {
                    [0.0; 2]
                };
                assert_eq!(*sample, expected, "position{position} block{block}");
            }
            control.reclaim();
        }
        assert_eq!(witnessed, 4);
        // Callback/control drops and all graph reclamation remain unmeasured.
    }
    Ok(())
}
