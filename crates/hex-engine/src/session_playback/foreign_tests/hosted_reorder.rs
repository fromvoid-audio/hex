//! The compiled signal graph owns the dry sibling's delay and transport stamps.
use super::*;
use crate::{
    Audio,
    compensation::TransportStamp,
    playback::{HostedMoveRequest, HostedTrackEffectReorder},
    signal_graph::{FreshHistory, SignalRuntime},
};

#[test]
fn staged_hosted_reorder_fades_only_its_track_and_switches_to_the_fresh_binding() {
    use crate::session_playback::NativeRemovalProgress;
    use hex_lattice_contract::ForeignProcessorBinding;

    struct SilentEffect(ForeignProcessorBinding);
    impl ForeignProcessors for SilentEffect {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            assert_eq!(node.prepared.binding(), Some(self.0));
            frames.fill([0.0; 2]);
            Ok(())
        }

        fn reject_callback(&mut self) {
            panic!("unexpected callback refusal");
        }
    }

    const FRAMES: usize = 4096;
    const BRIDGE: usize = 512;
    let mut project = fixture(&[&[], &[]], FRAMES);
    let [track, dry] = *project.tracks.as_slice() else {
        panic!("two resident rows");
    };
    let bed = |frame: usize| [0.0, 0.125 + (frame % 31) as f32 / 1024.0];
    let asset = project.editor.project().track(dry).unwrap().clips()[0].asset();
    project.media.insert(
        asset,
        Audio::new(48_000, (0..FRAMES).map(bed).collect()).unwrap(),
    );
    let delay = &hex_devices::delay::DEFINITION;
    let Created::Device(native) = project
        .editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: delay.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("native Delay");
    };
    let Created::Device(device) = project
        .editor
        .execute(Command::InsertDevice {
            track,
            before: None,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![19])
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("hosted effect");
    };
    let owner = ProcessorChainOwner::Track(track.value());
    let identity = ProcessorInstanceIdentity::new(owner, device.value()).unwrap();
    let prepare = |project: &hex_project::Project, media, epoch| {
        let effect = instruments::paired_effect(identity, epoch);
        let definitions: [&dyn NativeDeviceDefinition; 2] = [&effect, delay];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = PreparedRevision::prepare_with_effect_registry(
            project,
            media,
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap();
        let binding = prepared.arrangement().tracks()[0]
            .device_chain
            .iter()
            .find_map(|node| node.foreign_processor()?.binding())
            .unwrap();
        (SessionPlayback::new(Arc::new(prepared)).unwrap(), binding)
    };
    let (mut before, previous_binding) = prepare(project.editor.project(), &project.media, 1);
    project
        .editor
        .execute(Command::ReorderDevices {
            owner: hex_project::DeviceChainOwner::Track(track),
            selected: hex_project::DeviceSelectionIds::new(&[native]).unwrap(),
            before: None,
        })
        .unwrap();
    let (mut after, binding) = prepare(project.editor.project(), &project.media, 2);
    let request = HostedTrackEffectReorder {
        track,
        device,
        previous_binding,
        binding,
    };
    let verify_bed =
        |runtime: &mut SessionPlayback, binding: ForeignProcessorBinding, pcm: &mut [Frame]| {
            let start = runtime.performance_frame() as usize;
            runtime
                .render_with_foreign_processors(pcm, None, None, None, &mut SilentEffect(binding))
                .unwrap();
            for (offset, sample) in pcm.iter().enumerate() {
                let frame = start + offset;
                let expected = frame.checked_sub(BRIDGE).map_or([0.0; 2], bed);
                assert_eq!(sample[1], expected[1], "dry output at frame {frame}");
            }
        };
    before.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 256];
    for _ in 0..3 {
        verify_bed(&mut before, previous_binding, &mut block);
    }
    after.require_continuous_insert = true;
    after.hosted_move = Some(HostedMoveRequest::Reorder(HostedTrackEffectReorder {
        previous_binding: binding,
        ..request
    }));
    assert!(after.advance_native_removal_from(&mut before).is_err());
    assert!(!before.track_faders[0].active());
    after.hosted_move = Some(HostedMoveRequest::Reorder(request));
    assert_eq!(
        after.advance_native_removal_from(&mut before),
        Ok(NativeRemovalProgress::Fading)
    );
    let before_fade = before.performance_frame();
    assert!(after.try_inherit_state_live_from(&mut before).is_err());
    assert_eq!(before.performance_frame(), before_fade);
    verify_bed(&mut before, previous_binding, &mut block[..31]);
    assert!(before.track_faders[0].active());
    assert!(!before.track_faders[1].active());
    after.cancel_native_removal_from(&mut before);
    verify_bed(&mut before, previous_binding, &mut block);
    assert!(!before.track_faders[0].active());
    assert_eq!(
        after.advance_native_removal_from(&mut before),
        Ok(NativeRemovalProgress::Fading)
    );
    verify_bed(&mut before, previous_binding, &mut block);
    assert_eq!(
        after.advance_native_removal_from(&mut before),
        Ok(NativeRemovalProgress::Ready)
    );
    let cut = before.performance_frame();
    after.try_inherit_state_live_from(&mut before).unwrap();
    assert_eq!(after.performance_frame(), cut);
    assert!(after.track_faders[0].active());
    assert!(!after.track_faders[1].active());
    verify_bed(&mut after, binding, &mut block);
}

#[test]
fn same_track_hosted_reorder_keeps_the_unchanged_dry_delay_and_stamps() {
    const DELAY: u32 = 512;
    let mut project = fixture(&[&[], &[]], 2048);
    let affected = project.tracks[0];
    let delay = &hex_devices::delay::DEFINITION;
    let Created::Device(first) = project
        .editor
        .execute(Command::InsertDevice {
            track: affected,
            before: None,
            state: delay.default_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("native effect");
    };
    let Created::Device(hosted) = project
        .editor
        .execute(Command::InsertDevice {
            track: affected,
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("hosted effect");
    };

    // The borrowed fixture contributes a 16-frame bridge. Only the
    // compiled compensation geometry is exercised here.
    let definition = BorrowedDefinition {
        binding_epoch: Some(1),
        instances: vec![(
            ProcessorInstanceIdentity::new(
                ProcessorChainOwner::Track(affected.value()),
                hosted.value(),
            )
            .unwrap(),
            DELAY - 16,
        )],
        unbounded_tail: false,
    };
    let definitions: [&dyn NativeDeviceDefinition; 2] = [&definition, delay];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let project_media = &project.media;
    let compile = |authored: &hex_project::Project, order: [hex_project::DeviceId; 2]| {
        let revision = PreparedRevision::prepare_with_effect_registry(
            authored,
            project_media,
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap();
        let arrangement = revision.arrangement();
        assert!(
            arrangement.tracks()[0]
                .device_chain
                .iter()
                .map(|device| device.device_id)
                .eq(order.map(hex_project::DeviceId::value))
        );
        let graph = arrangement.signal_graph.as_ref().unwrap();
        assert_eq!(graph.main_input_arrival(), DELAY);
        assert_eq!(
            graph
                .processor_latency(hosted.value(), None)
                .unwrap()
                .upstream_frames,
            0
        );
        SignalRuntime::new(Arc::clone(graph)).unwrap()
    };

    let mut before = compile(project.editor.project(), [first, hosted]);
    project
        .editor
        .execute(Command::ReorderDevices {
            owner: hex_project::DeviceChainOwner::Track(affected),
            selected: hex_project::DeviceSelectionIds::new(&[first]).unwrap(),
            before: None,
        })
        .unwrap();
    let mut after = compile(project.editor.project(), [hosted, first]);

    let dry_input = |frame: u32| [frame as f32 + 1.0, -0.25];
    let render = |runtime: &mut SignalRuntime, frame| {
        runtime.delays.begin_transaction().unwrap();
        runtime
            .delays
            .push_transport_stamp(TransportStamp {
                authored_clock_epoch: std::num::NonZeroU64::new(1),
                tempo_micro_bpm: None,
                performance_frame: u64::from(frame),
                transport_is_running: true,
                arrangement_frame: u64::from(frame) + 2048,
                loop_pass: 0,
                performance_beat_raw: 0,
                arrangement_beat_raw: None,
                meter: None,
                cycle_beats_raw: None,
            })
            .unwrap();
        let out = runtime.route_track_output(1, dry_input(frame)).unwrap();
        runtime.delays.commit_transaction().unwrap();
        out
    };
    for frame in 0..DELAY {
        assert_eq!(render(&mut before, frame), [0.0; 2]);
    }
    assert!(!after.can_copy_from(&before, None));
    let changed = |second_device| FreshHistory::ChangedTrack {
        track: affected,
        device: hosted.value(),
        second_device,
        cut_predecessor_pcm: false,
    };
    assert!(!after.can_copy_from(&before, Some(changed(None))));
    let reordered = changed(Some(first.value()));
    assert!(before.owner_history_is_silent(reordered));
    assert!(after.owner_history_is_silent(reordered));
    after.copy_from(&before, Some(reordered)).unwrap();
    for frame in DELAY..DELAY * 2 {
        assert_eq!(render(&mut after, frame), dry_input(frame - DELAY));
        assert_eq!(
            after
                .delays
                .stamp_at_delay(DELAY)
                .map(|stamp| stamp.arrangement_frame),
            Some(u64::from(frame - DELAY) + 2048)
        );
    }
}

#[test]
fn hosted_track_effect_move_with_empty_third_track_cuts_and_undoes_live() {
    use crate::playback::HostedTrackEffectMove;
    use crate::session_playback::NativeRemovalProgress;
    use hex_lattice_contract::ForeignProcessorBinding;

    struct ScaleEffect {
        binding: ForeignProcessorBinding,
        gain: f32,
    }
    impl ForeignProcessors for ScaleEffect {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            assert_eq!(node.prepared.binding(), Some(self.binding));
            for frame in frames {
                frame[0] *= self.gain;
                frame[1] *= self.gain;
            }
            Ok(())
        }

        fn reject_callback(&mut self) {
            panic!("unexpected callback refusal");
        }
    }

    const FRAMES: usize = 8192;
    let mut project = fixture(&[&[], &[]], FRAMES);
    let [donor, receiver] = *project.tracks.as_slice() else {
        panic!("two resident audio rows");
    };
    let Created::Track(_empty_third) = project
        .editor
        .execute(Command::AddTrack {
            name: "empty third".into(),
        })
        .unwrap()
    else {
        panic!("empty third track");
    };
    let donor_asset = project.editor.project().track(donor).unwrap().clips()[0].asset();
    let receiver_asset = project.editor.project().track(receiver).unwrap().clips()[0].asset();
    project.media.insert(
        donor_asset,
        Audio::new(48_000, vec![[0.25, 0.0]; FRAMES]).unwrap(),
    );
    project.media.insert(
        receiver_asset,
        Audio::new(48_000, vec![[0.0, 0.25]; FRAMES]).unwrap(),
    );
    let Created::Device(device) = project
        .editor
        .execute(Command::InsertDevice {
            track: donor,
            before: None,
            state: NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![19])
                .unwrap(),
        })
        .unwrap()
    else {
        panic!("hosted effect");
    };

    let prepare = |proj: &hex_project::Project,
                   media: &std::collections::BTreeMap<hex_project::AssetId, Audio>,
                   track_id: hex_project::TrackId,
                   track_idx: usize,
                   epoch: u64| {
        let owner = ProcessorChainOwner::Track(track_id.value());
        let identity = ProcessorInstanceIdentity::new(owner, device.value()).unwrap();
        let effect = instruments::paired_effect(identity, epoch);
        let definitions: [&dyn NativeDeviceDefinition; 1] = [&effect];
        let registry = NativeDeviceRegistry::new(&definitions).unwrap();
        let prepared = PreparedRevision::prepare_with_effect_registry(
            proj,
            media,
            Arc::new(SineTable::new()),
            None,
            &registry,
        )
        .unwrap();
        let binding = prepared.arrangement().tracks()[track_idx]
            .device_chain
            .iter()
            .find_map(|node| node.foreign_processor()?.binding())
            .unwrap();
        (SessionPlayback::new(Arc::new(prepared)).unwrap(), binding)
    };

    let (mut before, binding1) = prepare(project.editor.project(), &project.media, donor, 0, 1);
    before.apply(SessionPlaybackCommand::Play).unwrap();
    let mut block = [[0.0; 2]; 256];
    for _ in 0..3 {
        before
            .render_with_foreign_processors(
                &mut block,
                None,
                None,
                None,
                &mut ScaleEffect {
                    binding: binding1,
                    gain: 2.0,
                },
            )
            .unwrap();
    }

    // Move donor -> receiver with the empty third track present.
    project
        .editor
        .execute(Command::MoveTrackDevices {
            source: donor,
            selected: hex_project::DeviceSelectionIds::new(&[device]).unwrap(),
            destination: receiver,
        })
        .unwrap();
    let (mut after_move, binding2) =
        prepare(project.editor.project(), &project.media, receiver, 1, 2);
    after_move.require_continuous_insert = true;
    after_move.hosted_move = Some(HostedMoveRequest::Effect(HostedTrackEffectMove {
        donor,
        receiver,
        device,
        binding: binding2,
    }));

    assert_eq!(
        after_move.advance_native_removal_from(&mut before),
        Ok(NativeRemovalProgress::Fading)
    );
    // Render through the fade and 512-frame dry-alignment drain.
    for _ in 0..4 {
        before
            .render_with_foreign_processors(
                &mut block,
                None,
                None,
                None,
                &mut ScaleEffect {
                    binding: binding1,
                    gain: 2.0,
                },
            )
            .unwrap();
    }
    assert_eq!(
        after_move.advance_native_removal_from(&mut before),
        Ok(NativeRemovalProgress::Ready)
    );
    let cut_frame = before.performance_frame();
    after_move.try_inherit_state_live_from(&mut before).unwrap();
    assert_eq!(after_move.performance_frame(), cut_frame);
    after_move
        .render_with_foreign_processors(
            &mut block,
            None,
            None,
            None,
            &mut ScaleEffect {
                binding: binding2,
                gain: 2.0,
            },
        )
        .unwrap();

    // Undo back to donor while playing with the empty third track still present.
    assert!(project.editor.try_undo().unwrap());
    let (mut after_undo, binding3) = prepare(project.editor.project(), &project.media, donor, 0, 3);
    after_undo.require_continuous_insert = true;
    after_undo.hosted_move = Some(HostedMoveRequest::Effect(HostedTrackEffectMove {
        donor: receiver,
        receiver: donor,
        device,
        binding: binding3,
    }));
    assert_eq!(
        after_undo.advance_native_removal_from(&mut after_move),
        Ok(NativeRemovalProgress::Fading)
    );
    for _ in 0..4 {
        after_move
            .render_with_foreign_processors(
                &mut block,
                None,
                None,
                None,
                &mut ScaleEffect {
                    binding: binding2,
                    gain: 2.0,
                },
            )
            .unwrap();
    }
    assert_eq!(
        after_undo.advance_native_removal_from(&mut after_move),
        Ok(NativeRemovalProgress::Ready)
    );
    let undo_frame = after_move.performance_frame();
    after_undo
        .try_inherit_state_live_from(&mut after_move)
        .unwrap();
    assert_eq!(after_undo.performance_frame(), undo_frame);
}
