//! Headless allocation/retirement proof; this does not measure native deadlines.
use allocation_counter::measure;
use hex_audio::prepared;
use hex_engine::{
    Audio, BorrowedMediaPage, Frame, MediaPages, PreparedRevision, SessionPlayback,
    SessionPlaybackCommand, SineTable, StreamSourceKey,
    playback::{Playback, PlaybackCommand},
};
use hex_project::{Command, Created, Editor, Project};
use std::{collections::BTreeMap, sync::Arc};

struct NoPages;
impl MediaPages for NoPages {
    fn page(&mut self, _: StreamSourceKey, _: u64) -> Option<BorrowedMediaPage<'_>> {
        panic!("resident graph must not request pages")
    }
    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        panic!("resident graph must not request pages")
    }
}
fn assert_zero(count: allocation_counter::AllocationInfo) {
    assert_eq!(
        (
            count.count_total,
            count.bytes_total,
            count.count_current,
            count.bytes_current
        ),
        (0, 0, 0, 0)
    );
}
#[test]
fn authored_feed_render_observation_handoff_and_retirement_have_no_callback_allocation_or_drop() {
    assert_callback(false);
}
#[test]
fn gain_automation_feed_render_observation_handoff_and_retirement_have_no_callback_allocation_or_drop()
 {
    assert_callback(true);
}
fn assert_callback(automation: bool) {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, .. } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "source".into(),
            path: "memory.wav".into(),
            source_sample_rate: 48000,
            source_frames: 2048,
            playback_frames: 2048,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let mut returns = Vec::new();
    for _ in 0..2 {
        let Created::ReturnTrack(id) = editor
            .execute(Command::AddReturnTrack { name: "bus".into() })
            .unwrap()
        else {
            panic!("return")
        };
        returns.push(id);
        let state = hex_engine::patch_native_device_parameter(
            &hex_engine::default_native_device_state(hex_engine::DeviceTypeId::from_static(
                "hex.utility",
            ))
            .unwrap(),
            hex_lattice_contract::ParameterId::new(11).unwrap(),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
        editor
            .execute(Command::InsertReturnDevice {
                return_track: id,
                before: None,
                state,
            })
            .unwrap();
    }
    editor
        .execute(Command::SetReturnSend {
            track,
            return_track: returns[1],
            gain: 0.5,
        })
        .unwrap();
    editor
        .execute(Command::SetReturnFeed {
            source: returns[1],
            destination: returns[0],
            gain: 0.5,
        })
        .unwrap();
    if automation {
        editor.execute(Command::Gain { track, gain: 0.0 }).unwrap();
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ZERO,
                        gain: 0.5,
                        outgoing: hex_project::GainInterpolation::Linear,
                    },
                    hex_project::GainPoint {
                        incoming_gain: None,
                        beat: hex_project::BeatTime::ONE,
                        gain: 1.0,
                        outgoing: hex_project::GainInterpolation::Step,
                    },
                ],
            })
            .unwrap();
    }
    let media = BTreeMap::from([(asset, Audio::new(48000, vec![[0.25, -0.5]; 2048]).unwrap())]);
    let table = Arc::new(SineTable::new());
    let revision = Arc::new(
        PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
    );
    let observation =
        hex_engine::PreparedPlaybackObservation::prepare_on_control(&revision).unwrap();
    observation.set_render_work_enabled(true);
    let mut session = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    session.apply(SessionPlaybackCommand::Play).unwrap();
    let owners = Arc::strong_count(&revision);
    let mut output = [[0.0; 2]; 513];
    let mut pages = NoPages;
    let mut calls = 0;
    assert_zero(measure(|| {
        session
            .render_with_pages_and_post_node_observer(
                &mut output,
                &mut pages,
                Some(&mut |_, _, _: &[Frame]| calls += 1),
            )
            .unwrap()
    }));
    assert!(calls >= 6);
    assert_eq!(Arc::strong_count(&revision), owners);
    let (mut control, mut callback) = prepared::channel(48000).unwrap();
    control
        .publish(Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap())
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    assert_zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    assert_zero(measure(|| {
        for _ in 0..2 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
    }));
    assert!(
        observation
            .render_work_meters()
            .all(|(_, meter)| meter.sample().is_some())
    );
    editor
        .execute(Command::SetReturnFeed {
            source: returns[1],
            destination: returns[0],
            gain: 0.25,
        })
        .unwrap();
    if automation {
        editor
            .execute(Command::UpsertTrackGainPoint {
                track,
                point: hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.25,
                    outgoing: hex_project::GainInterpolation::Linear,
                },
            })
            .unwrap();
    }
    let next = Arc::new(PreparedRevision::prepare(editor.project(), &media, table, None).unwrap());
    let next_observation =
        hex_engine::PreparedPlaybackObservation::prepare_on_control(&next).unwrap();
    next_observation.set_render_work_enabled(true);
    control
        .publish(Playback::try_from_prepared_with_session(next).unwrap())
        .unwrap();
    assert_zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    assert!(control.status.rejected_action().is_none());
    assert!(
        control.reclaim() > 0,
        "old revision ownership must retire to the control side"
    );
    control.request_shutdown();
    assert_zero(measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }));
    control.reclaim();
}
