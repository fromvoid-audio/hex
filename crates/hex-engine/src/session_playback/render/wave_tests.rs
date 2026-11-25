use super::*;
use crate::signal_graph::tests::fixture;
use crate::wave_executor::{SerialWaveExecutor, WaveClockWindow, WaveExecutor, WaveStageArrival};
use crate::{FilteredNodeSignalObserver, NodeSignalPhase, PostNodeIdentity, ProcessorChainOwner};
use hex_lattice::NativeRuntimeChain;
use hex_lattice_contract::{DeviceContractError, DeviceTypeId};
use hex_project::{Command, Created, ReturnId, TrackSelectionIds};

struct CountingWaveExecutor {
    inner: SerialWaveExecutor,
    workers: usize,
    waves_started: usize,
    tasks_pushed: usize,
    tasks_reclaimed: usize,
    waves_finished: usize,
}

impl CountingWaveExecutor {
    fn new(workers: usize) -> Self {
        Self {
            inner: SerialWaveExecutor::default(),
            workers,
            waves_started: 0,
            tasks_pushed: 0,
            tasks_reclaimed: 0,
            waves_finished: 0,
        }
    }
}

impl WaveExecutor for CountingWaveExecutor {
    fn worker_count(&self) -> usize {
        self.workers
    }

    fn prepare_clock(&mut self) -> Option<&mut WaveClockWindow> {
        let clock = self.inner.prepare_clock()?;
        self.waves_started += 1;
        Some(clock)
    }

    fn push_task(
        &mut self,
        owner_index: usize,
        chain: NativeRuntimeChain,
        audio: &[Frame],
        frame_offset: usize,
        stage_start: usize,
        stage_end: usize,
        compensated: bool,
        stage_arrivals: &[WaveStageArrival],
    ) -> Result<usize, NativeRuntimeChain> {
        let slot = self.inner.push_task(
            owner_index,
            chain,
            audio,
            frame_offset,
            stage_start,
            stage_end,
            compensated,
            stage_arrivals,
        )?;
        self.tasks_pushed += 1;
        Ok(slot)
    }

    fn execute_staged(&mut self) {
        self.inner.execute_staged();
    }

    fn reclaim_task(
        &mut self,
        slot: usize,
        audio_out: &mut [Frame],
    ) -> (usize, NativeRuntimeChain, Option<DeviceContractError>) {
        self.tasks_reclaimed += 1;
        self.inner.reclaim_task(slot, audio_out)
    }

    fn finish_wave(&mut self) {
        self.waves_finished += 1;
        self.inner.finish_wave();
    }
}

struct FailingTrackCapture {
    fail_track: TrackId,
    failed: bool,
}

impl CaptureSink for FailingTrackCapture {
    fn selects(&self, id: OutputCaptureId) -> bool {
        id == OutputCaptureId::Track(self.fail_track)
    }

    fn add(
        &mut self,
        id: OutputCaptureId,
        _offset: usize,
        _frames: &[Frame],
    ) -> Result<(), SessionPlaybackError> {
        if id == OutputCaptureId::Track(self.fail_track) {
            self.failed = true;
            return Err(SessionPlaybackError::DeviceRuntime);
        }
        Ok(())
    }
}

fn fill_distinct_media(f: &mut crate::signal_graph::tests::Fixture, frames: usize) {
    for (index, audio) in f.media.values_mut().enumerate() {
        let scale = 0.08 * (index + 1) as f32;
        let samples = (0..frames)
            .map(|i| {
                let phase = (i as f32 * 0.07) + (index as f32 * 0.5);
                [phase.sin() * scale, phase.cos() * -scale]
            })
            .collect();
        *audio = crate::Audio::new(48_000, samples).unwrap();
    }
}

fn insert_utility_on_tracks(f: &mut crate::signal_graph::tests::Fixture, tracks: &[TrackId]) {
    for &track in tracks {
        f.editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: crate::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .unwrap(),
            })
            .unwrap();
    }
}

#[test]
fn flat_wave_reclaims_all_chains_and_finishes_wave_when_inline_track_fails() {
    let mut f = fixture(&[&[], &[], &[], &[], &[]], 512);
    fill_distinct_media(&mut f, 512);
    let tracks = f.tracks.clone();
    insert_utility_on_tracks(&mut f, &tracks);

    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut serial = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    serial.apply(SessionPlaybackCommand::Play).unwrap();

    // Mark track 2 observed so tracks 0, 1, 3, 4 stage into the wave executor
    // while track 2 executes inline between tracks 1 and 3 and fails capture.
    let fail_track = tracks[2];
    let mut observer = FilteredNodeSignalObserver::for_owner(
        ProcessorChainOwner::Track(fail_track.value()),
        |_, _, _, _| {},
    );
    let mut capture = FailingTrackCapture {
        fail_track,
        failed: false,
    };
    let mut exec = CountingWaveExecutor::new(2);
    let mut failed_buf = [[0.0; 2]; 128];
    let result = playback.render_internal_borrowed::<dyn MediaPages>(
        &mut failed_buf,
        None,
        Some(&mut observer),
        Some(&mut capture),
        &mut hex_lattice::RefuseForeign,
        RenderBlockInputs {
            sources: None,
            live_audio: None,
            pending_monitor_pcm: false,
        },
        Some(&mut exec),
    );

    assert_eq!(result, Err(SessionPlaybackError::DeviceRuntime));
    assert!(capture.failed);
    assert_eq!(exec.waves_started, 1);
    assert_eq!(exec.tasks_pushed, 4);
    assert_eq!(exec.tasks_reclaimed, 4);
    assert_eq!(exec.waves_finished, 1);
    for (index, chain) in playback.device_chains.iter().enumerate() {
        assert_eq!(chain.len(), 1, "track {index} chain must be reclaimed");
    }

    // Executor must remain usable on the following block.
    assert!(exec.prepare_clock().is_some());
    exec.finish_wave();

    // Follow-up block renders cleanly via parallel wave and matches serial PCM.
    let mut wave_out = [[0.0; 2]; 128];
    let mut serial_out = [[0.0; 2]; 128];
    playback
        .render_with_wave_executor(&mut wave_out, &mut exec)
        .unwrap();
    serial.render(&mut serial_out).unwrap();
    assert!(wave_out.iter().any(|frame| *frame != [0.0; 2]));
    for (actual, expected) in wave_out.iter().zip(&serial_out) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
    }
}

#[test]
fn compensated_wave_reclaims_all_chains_and_finishes_wave_when_inline_track_fails() {
    let mut f = fixture(&[&[], &[], &[], &[], &[]], 1024);
    fill_distinct_media(&mut f, 1024);
    let tracks = f.tracks.clone();
    insert_utility_on_tracks(&mut f, &tracks);

    // Group track 2 so its output routes into a group edge (exercising
    // compensated wave 0 alongside tracks 0, 1, 3, 4 and invoking capture.add
    // inside the wave loop). Add latency on the group bus so compensated block
    // routing activates.
    let Created::Track(group) = f
        .editor
        .execute(Command::GroupTracks {
            tracks: TrackSelectionIds::new(&[tracks[2]]).unwrap(),
            name: "bus".into(),
        })
        .unwrap()
    else {
        panic!("group track")
    };
    f.editor
        .execute(Command::InsertDevice {
            track: group,
            before: None,
            state: crate::default_native_device_state(DeviceTypeId::from_static("hex.saturator"))
                .unwrap(),
        })
        .unwrap();

    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut serial = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    serial.apply(SessionPlaybackCommand::Play).unwrap();

    // Warm up past compensation horizon so `can_block_compensated_route` is active.
    let mut warmup = [[0.0; 2]; 256];
    playback.render(&mut warmup).unwrap();
    serial.render(&mut warmup).unwrap();
    assert!(playback.can_block_compensated_route(128));

    let fail_track = tracks[2];
    let mut observer = FilteredNodeSignalObserver::for_owner(
        ProcessorChainOwner::Track(fail_track.value()),
        |_, _, _, _| {},
    );
    let mut capture = FailingTrackCapture {
        fail_track,
        failed: false,
    };
    let mut exec = CountingWaveExecutor::new(2);
    let mut failed_buf = [[0.0; 2]; 128];
    let result = playback.render_internal_borrowed::<dyn MediaPages>(
        &mut failed_buf,
        None,
        Some(&mut observer),
        Some(&mut capture),
        &mut hex_lattice::RefuseForeign,
        RenderBlockInputs {
            sources: None,
            live_audio: None,
            pending_monitor_pcm: false,
        },
        Some(&mut exec),
    );

    assert_eq!(result, Err(SessionPlaybackError::DeviceRuntime));
    assert!(capture.failed);
    assert_eq!(exec.waves_started, 1);
    assert_eq!(exec.tasks_pushed, 4);
    assert_eq!(exec.tasks_reclaimed, 4);
    assert_eq!(exec.waves_finished, 1);
    for (index, chain) in playback.device_chains.iter().enumerate() {
        assert!(!chain.is_empty(), "track {index} chain must be reclaimed");
    }

    assert!(exec.prepare_clock().is_some());
    exec.finish_wave();

    // Subsequent compensated block executes cleanly and stays bit-exact.
    let mut wave_out = [[0.0; 2]; 128];
    let mut serial_out = [[0.0; 2]; 128];
    playback
        .render_with_wave_executor(&mut wave_out, &mut exec)
        .unwrap();
    serial.render(&mut serial_out).unwrap();
    assert!(wave_out.iter().any(|frame| *frame != [0.0; 2]));
    for (actual, expected) in wave_out.iter().zip(&serial_out) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
    }
}

#[test]
fn filtered_observer_dispatches_unobserved_flat_tracks_and_returns_in_parallel() {
    // 1) Flat tracks: 5 tracks with utility, observe only track 2.
    let mut f = fixture(&[&[], &[], &[], &[], &[]], 256);
    fill_distinct_media(&mut f, 256);
    let tracks = f.tracks.clone();
    insert_utility_on_tracks(&mut f, &tracks);
    let observed_track = ProcessorChainOwner::Track(tracks[2].value());

    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut parallel = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut serial = SessionPlayback::new(revision).unwrap();
    parallel.apply(SessionPlaybackCommand::Play).unwrap();
    serial.apply(SessionPlaybackCommand::Play).unwrap();

    let mut parallel_out = [[0.0; 2]; 128];
    let mut serial_out = [[0.0; 2]; 128];
    let mut seen_parallel = Vec::new();
    let mut seen_serial = Vec::new();
    let mut exec = CountingWaveExecutor::new(2);

    {
        let mut obs = FilteredNodeSignalObserver::for_owner(
            observed_track,
            |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                seen_parallel.push((id, phase, offset, frames.len()));
            },
        );
        parallel
            .render_with_signal_observers_and_wave(
                &mut parallel_out,
                Some(&mut obs),
                None,
                Some(&mut exec),
            )
            .unwrap();
    }
    {
        let mut obs =
            |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                if id.owner == observed_track {
                    seen_serial.push((id, phase, offset, frames.len()));
                }
            };
        serial
            .render_with_node_signal_observer(&mut serial_out, Some(&mut obs))
            .unwrap();
    }

    assert_eq!(exec.waves_started, 1);
    assert_eq!(exec.tasks_pushed, 4);
    assert_eq!(exec.tasks_reclaimed, 4);
    assert_eq!(exec.waves_finished, 1);
    assert_eq!(seen_parallel, seen_serial);
    assert!(
        seen_parallel
            .iter()
            .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Input)
    );
    assert!(
        seen_parallel
            .iter()
            .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Output)
    );
    for (actual, expected) in parallel_out.iter().zip(&serial_out) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
    }

    // 2) Flat returns: 1 track sending to 5 return buses with utility, observe only return 2.
    let mut rf = fixture(&[&[]], 256);
    fill_distinct_media(&mut rf, 256);
    let mut returns: Vec<ReturnId> = Vec::new();
    for i in 0..5 {
        let Created::ReturnTrack(bus) = rf
            .editor
            .execute(Command::AddReturnTrack {
                name: format!("ret {i}"),
            })
            .unwrap()
        else {
            panic!("return track")
        };
        rf.editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: crate::default_native_device_state(DeviceTypeId::from_static("hex.utility"))
                    .unwrap(),
            })
            .unwrap();
        rf.editor
            .execute(Command::SetReturnSend {
                track: rf.tracks[0],
                return_track: bus,
                gain: 0.25,
            })
            .unwrap();
        returns.push(bus);
    }
    let observed_return = ProcessorChainOwner::Return(returns[2].value());

    let rev = Arc::new(
        PreparedRevision::prepare(
            rf.editor.project(),
            &rf.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut par_ret = SessionPlayback::new(Arc::clone(&rev)).unwrap();
    let mut ser_ret = SessionPlayback::new(rev).unwrap();
    par_ret.apply(SessionPlaybackCommand::Play).unwrap();
    ser_ret.apply(SessionPlaybackCommand::Play).unwrap();

    let mut par_ret_out = [[0.0; 2]; 128];
    let mut ser_ret_out = [[0.0; 2]; 128];
    let mut seen_par_ret = Vec::new();
    let mut seen_ser_ret = Vec::new();
    let mut ret_exec = CountingWaveExecutor::new(2);

    {
        let mut obs = FilteredNodeSignalObserver::for_owner(
            observed_return,
            |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                seen_par_ret.push((id, phase, offset, frames.len()));
            },
        );
        par_ret
            .render_with_signal_observers_and_wave(
                &mut par_ret_out,
                Some(&mut obs),
                None,
                Some(&mut ret_exec),
            )
            .unwrap();
    }
    {
        let mut obs =
            |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                if id.owner == observed_return {
                    seen_ser_ret.push((id, phase, offset, frames.len()));
                }
            };
        ser_ret
            .render_with_node_signal_observer(&mut ser_ret_out, Some(&mut obs))
            .unwrap();
    }

    assert_eq!(ret_exec.waves_started, 1);
    assert_eq!(ret_exec.tasks_pushed, 4);
    assert_eq!(ret_exec.tasks_reclaimed, 4);
    assert_eq!(ret_exec.waves_finished, 1);
    assert_eq!(seen_par_ret, seen_ser_ret);
    assert!(
        seen_par_ret
            .iter()
            .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Input)
    );
    assert!(
        seen_par_ret
            .iter()
            .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Output)
    );
    for (actual, expected) in par_ret_out.iter().zip(&ser_ret_out) {
        assert_eq!(actual.map(f32::to_bits), expected.map(f32::to_bits));
    }
}

#[test]
fn filtered_observer_dispatches_unobserved_compensated_tracks_and_returns_in_parallel() {
    let mut f = fixture(&[&[], &[], &[], &[], &[]], 1024);
    fill_distinct_media(&mut f, 1024);
    let tracks = f.tracks.clone();
    insert_utility_on_tracks(&mut f, &tracks);

    let mut returns: Vec<ReturnId> = Vec::new();
    for i in 0..5 {
        let Created::ReturnTrack(bus) = f
            .editor
            .execute(Command::AddReturnTrack {
                name: format!("ret {i}"),
            })
            .unwrap()
        else {
            panic!("return track")
        };
        // Latency on return 0 ensures compensated block routing activates with
        // both a track wave (5 tracks) and a return wave (5 returns).
        let dev = if i == 0 {
            "hex.saturator"
        } else {
            "hex.utility"
        };
        f.editor
            .execute(Command::InsertReturnDevice {
                return_track: bus,
                before: None,
                state: crate::default_native_device_state(DeviceTypeId::from_static(dev)).unwrap(),
            })
            .unwrap();
        f.editor
            .execute(Command::SetReturnSend {
                track: tracks[0],
                return_track: bus,
                gain: 0.2,
            })
            .unwrap();
        returns.push(bus);
    }

    let revision = Arc::new(
        PreparedRevision::prepare(
            f.editor.project(),
            &f.media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );

    for observed_owner in [
        ProcessorChainOwner::Track(tracks[2].value()),
        ProcessorChainOwner::Return(returns[2].value()),
    ] {
        let mut parallel = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        let mut serial = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        parallel.apply(SessionPlaybackCommand::Play).unwrap();
        serial.apply(SessionPlaybackCommand::Play).unwrap();

        let mut warmup = [[0.0; 2]; 256];
        parallel.render(&mut warmup).unwrap();
        serial.render(&mut warmup).unwrap();
        assert!(parallel.can_block_compensated_route(128));

        let mut parallel_out = [[0.0; 2]; 128];
        let mut serial_out = [[0.0; 2]; 128];
        let mut seen_parallel = Vec::new();
        let mut seen_serial = Vec::new();
        let mut exec = CountingWaveExecutor::new(2);

        {
            let mut obs = FilteredNodeSignalObserver::for_owner(
                observed_owner,
                |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                    seen_parallel.push((id, phase, offset, frames.len()));
                },
            );
            parallel
                .render_with_signal_observers_and_wave(
                    &mut parallel_out,
                    Some(&mut obs),
                    None,
                    Some(&mut exec),
                )
                .unwrap();
        }
        {
            let mut obs =
                |id: PostNodeIdentity, phase: NodeSignalPhase, offset: usize, frames: &[Frame]| {
                    if id.owner == observed_owner {
                        seen_serial.push((id, phase, offset, frames.len()));
                    }
                };
            serial
                .render_with_node_signal_observer(&mut serial_out, Some(&mut obs))
                .unwrap();
        }

        // Track wave (5 tracks) + Return wave (5 returns) = 10 owners; 1 observed inline, 9 staged!
        assert_eq!(exec.waves_started, 2, "owner {observed_owner:?}");
        assert_eq!(exec.tasks_pushed, 9, "owner {observed_owner:?}");
        assert_eq!(exec.tasks_reclaimed, 9, "owner {observed_owner:?}");
        assert_eq!(exec.waves_finished, 2, "owner {observed_owner:?}");
        assert_eq!(seen_parallel, seen_serial, "owner {observed_owner:?}");
        assert!(
            seen_parallel
                .iter()
                .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Input),
            "owner {observed_owner:?}"
        );
        assert!(
            seen_parallel
                .iter()
                .any(|(_, phase, _, _)| *phase == NodeSignalPhase::Output),
            "owner {observed_owner:?}"
        );
        for (actual, expected) in parallel_out.iter().zip(&serial_out) {
            assert_eq!(
                actual.map(f32::to_bits),
                expected.map(f32::to_bits),
                "owner {observed_owner:?}"
            );
        }
    }
}
