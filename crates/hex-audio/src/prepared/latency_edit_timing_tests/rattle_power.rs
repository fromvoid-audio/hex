//! Exercise the real Rattle processor and the host's separate dry delay.
use super::*;
use hex_devices::rattle::{LATENCY_FRAMES, RATTLE_DEFINITION, RattleDefinition};
use hex_lattice::{EMPTY_DEVICE_RESOURCES, PreparedNativeProcessor};

#[test]
fn rattle_power_retains_aligned_dry_and_restarts_its_dsp_fresh() {
    const LATENCY: usize = LATENCY_FRAMES as usize;
    const FADE: usize = 240;
    let state = RATTLE_DEFINITION.default_state().unwrap();
    let input: Vec<_> = (0..8192)
        .map(|frame| {
            let sample = (frame as f32 * 0.037).sin() * 0.3;
            [sample, -sample * 0.8]
        })
        .collect();
    let factory = RattleDefinition::prepare_owned(&state, RATE, &[]).unwrap();
    let wet = |input: &[Frame]| {
        let mut rendered = input.to_vec();
        factory
            .create_runtime()
            .process_block(&mut rendered, &EMPTY_DEVICE_RESOURCES);
        rendered
    };
    let uninterrupted = wet(&input[..3 * BLOCK]);
    let (mut editor, _, prepare) = resident_input(input.clone());
    let Created::Device(rattle) = editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap()
    else {
        panic!("Rattle insertion");
    };
    let on = prepare(editor.project());
    editor
        .execute(Command::SetDeviceBypass {
            device: rattle,
            bypassed: true,
        })
        .unwrap();
    let off = prepare(editor.project());
    assert_eq!(on.arrangement().main_latency_frames(), LATENCY_FRAMES);
    assert_eq!(off.arrangement().main_latency_frames(), LATENCY_FRAMES);

    for off_frames in [64, BLOCK] {
        let reenable = BLOCK + off_frames;
        let fresh = wet(&input[reenable..reenable + BLOCK]);
        let settled = off_frames >= FADE;
        let (mut control, mut callback) = channel(RATE).unwrap();
        let mut cursor = 0;
        let output_identity = control.output_identity().unwrap();
        for (phase, (revision, frames)) in [(&on, BLOCK), (&off, off_frames), (&on, BLOCK)]
            .into_iter()
            .enumerate()
        {
            let context = hex_engine::AudioSourceContext {
                project_generation: 5,
                editor_revision: phase as u64 + 1,
            };
            let policy = if phase == 0 {
                PublicationPolicy::InstallFreshRuntime
            } else {
                PublicationPolicy::InheritRuntime
            };
            let audio_revision = control
                .publish_unaccounted_pcm_obligation(
                    Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap(),
                    policy,
                    context.project_generation,
                    true,
                    false,
                    Some(context),
                )
                .unwrap();
            if phase == 0 {
                control.command(PlaybackCommand::Play).unwrap();
            }
            let mut output = [[0.0; 2]; BLOCK];
            let mut render = || {
                callback.begin_block();
                callback.render(&mut output[..frames]).unwrap();
            };
            #[cfg(feature = "allocation-probe")]
            {
                let counts = allocation_counter::measure(&mut render);
                assert_eq!(
                    (
                        counts.count_total,
                        counts.bytes_total,
                        counts.count_current,
                        counts.bytes_current
                    ),
                    (0, 0, 0, 0)
                );
            }
            #[cfg(not(feature = "allocation-probe"))]
            render();

            assert_eq!(
                control.status.recording_source(),
                Some(AppliedAudioSource {
                    context,
                    audio_revision,
                    output_identity
                })
            );
            assert_eq!(
                control.status.position.load(Acquire),
                (cursor + frames) as u64
            );
            assert_eq!(control.reclaim(), usize::from(phase > 0));

            for (offset, actual) in output[..frames].iter().enumerate() {
                let frame = cursor + offset;
                if frame < LATENCY {
                    assert_eq!(*actual, [0.0; 2]);
                    continue;
                }
                let dry = input[frame - LATENCY];
                let (wet, mix) = match phase {
                    0 => (uninterrupted[frame], 1.0),
                    1 => (
                        uninterrupted[frame],
                        1.0 - (offset + 1).min(FADE) as f32 / FADE as f32,
                    ),
                    _ if settled => (
                        fresh[offset],
                        (offset + 1).saturating_sub(LATENCY).min(FADE) as f32 / FADE as f32,
                    ),
                    _ => {
                        let from = 1.0 - off_frames as f32 / FADE as f32;
                        let progress = (offset + 1).min(FADE) as f32 / FADE as f32;
                        (uninterrupted[frame], from + (1.0 - from) * progress)
                    }
                };
                if mix == 0.0 {
                    assert_eq!(
                        *actual, dry,
                        "Rattle must retain exactly aligned dry, phase {phase}, frame {frame}"
                    );
                } else {
                    for ((actual, dry), wet) in actual.iter().zip(dry).zip(wet) {
                        let expected = dry + (wet - dry) * mix;
                        assert!(
                            (actual - expected).abs() < 1e-6,
                            "Rattle power PCM, settled={settled}, phase={phase}, frame={frame}: {actual} vs {expected}"
                        );
                    }
                }
            }
            cursor += frames;
        }
    }
}
