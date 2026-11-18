//! PCM and route-clock oracles for a prepared latency edit that straddles
//! an already admitted quantized action. Each runtime starts independently;
//! the shifted candidate is a counterfactual, never an installed graph.
use super::*;
use hex_engine::{LaunchQuantization, SessionPlaybackCommand, SessionRouteTarget};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

const RATE: u32 = 48_000;
const DELAY: usize = 72;
const BLOCK: usize = 257;
const SIGNAL: Frame = [0.25; 2];

fn revisions() -> ([Arc<hex_engine::PreparedRevision>; 2], hex_project::TrackId) {
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "constant input".into(),
            path: "memory/constant.wav".into(),
            source_sample_rate: RATE,
            source_frames: 8192,
            playback_frames: 8192,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("audio import");
    };
    let media = BTreeMap::from([(
        asset,
        hex_engine::Audio::new(RATE, vec![SIGNAL; 8192]).unwrap(),
    )]);
    let table = Arc::new(hex_engine::SineTable::new());
    let prepare = |project: &Project| {
        Arc::new(
            hex_engine::PreparedRevision::prepare(project, &media, Arc::clone(&table), None)
                .unwrap(),
        )
    };
    let dry = prepare(editor.project());
    let limiter = &hex_devices::limiter::DEFINITION;
    let state = limiter
        .patch_parameter(
            &limiter.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(0),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let delayed = prepare(editor.project());
    assert_eq!(dry.arrangement().main_latency_frames(), 0);
    assert_eq!(delayed.arrangement().main_latency_frames() as usize, DELAY);
    ([dry, delayed], track)
}

fn at_cut(revision: &Arc<hex_engine::PreparedRevision>, frame: usize) -> Playback {
    let mut runtime = Playback::try_from_prepared_with_session(Arc::clone(revision)).unwrap();
    runtime.apply(PlaybackCommand::Play);
    runtime.render(&mut vec![[0.0; 2]; frame]).unwrap();
    assert_eq!(runtime.position(), frame as u64);
    runtime
}

#[test]
fn latency_edit_must_keep_the_admitted_stop_track_interval() {
    let (revisions, track) = revisions();
    // At 120 bpm the 1/32 boundaries are 3,000 frames apart. The wider
    // interval after each command contains both the causal and audible cut.
    for (old, next, host, old_cut, next_cut, difference) in [
        (0, 1, 2900, 3000, 3000, None),
        (0, 1, 2990, 3000, 6000, Some(10)),
        (1, 0, 2900, 3000, 3000, None),
        (1, 0, 3010, 6000, 3000, Some(62)),
    ] {
        for partition in [1, BLOCK] {
            let candidate_frame = (host + next * DELAY) - old * DELAY;
            let command = PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
                track,
                quantization: LaunchQuantization::ThirtySecond,
            });
            let mut previous = at_cut(&revisions[old], host);
            let mut candidate = at_cut(&revisions[next], candidate_frame);
            let render = |runtime: &mut Playback, cut| {
                runtime.try_apply_action(command).unwrap();
                let pending = runtime.session_route_status(0).unwrap().pending.unwrap();
                assert_eq!(pending.target, SessionRouteTarget::Silence);
                assert_eq!(pending.performance_frame, cut);
                let mut output = [[0.0; 2]; BLOCK];
                for block in output.chunks_mut(partition) {
                    runtime.render(block).unwrap();
                }
                output
            };
            let expected = render(&mut previous, old_cut);
            let shifted = render(&mut candidate, next_cut);
            assert_eq!(
                expected.iter().zip(shifted).position(|(a, b)| *a != b),
                difference,
                "old={old}, next={next}, host={host}, partition={partition}",
            );
            let predecessor_signal = |offset| {
                if (host + offset) < old_cut as usize + old * DELAY {
                    SIGNAL
                } else {
                    [0.0; 2]
                }
            };
            assert_eq!(expected, std::array::from_fn(predecessor_signal));
        }
    }
}

#[test]
fn rejecting_an_expired_cut_keeps_the_original_audio_and_action() {
    let ([old, next], track) = revisions();
    let (mut control, mut callback) = channel(RATE).unwrap();
    let installed = control
        .publish(Playback::try_from_prepared_with_session(old).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 2990]).unwrap();
    let cut = control.capture_fresh_runtime_fence().unwrap();

    let ticket = control
        .action_with_ticket(PlaybackAction::Session(SessionPlaybackCommand::StopTrack {
            track,
            quantization: LaunchQuantization::ThirtySecond,
        }))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 2990);

    let revision = control
        .publish_with_policy(
            Playback::try_from_prepared_with_session(next).unwrap(),
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(cut),
        )
        .unwrap();
    callback.begin_block();
    let mut actual = [[0.0; 2]; BLOCK];
    callback.render(&mut actual).unwrap();
    assert_eq!(control.status.applied_revision.load(Acquire), installed);
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged)),
    );
    assert_eq!(actual[..10], [SIGNAL; 10]);
    assert_eq!(actual[10..], [[0.0; 2]; BLOCK - 10]);
    assert_eq!(control.status.applied_revision.load(Acquire), installed);
    assert!(control.status.playing.load(Acquire));
}
