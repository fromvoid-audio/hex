//! A scope resize during a streamed handoff with a conflicting hosted effect.
use super::*;

#[test]
fn paused_streamed_scope_refusal_preserves_playback_and_allows_the_next_resize() {
    use crate::native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets};
    use crate::{BorrowedMediaPage, MediaPages, StreamSourceKey, StreamedSourceMetadata};
    use std::collections::BTreeMap;
    use std::sync::atomic::AtomicBool;

    struct Pages<'a> {
        key: StreamSourceKey,
        samples: &'a [Frame],
        reads: usize,
    }
    impl MediaPages for Pages<'_> {
        fn page(&mut self, key: StreamSourceKey, index: u64) -> Option<BorrowedMediaPage<'_>> {
            assert_eq!((key, index), (self.key, 0));
            self.reads += 1;
            Some(BorrowedMediaPage {
                page_index: index,
                frames: self.samples,
            })
        }
        fn request_page(&mut self, _: StreamSourceKey, _: u64) {
            panic!("the exact requested page is present");
        }
    }
    #[derive(Default)]
    struct Effect {
        delay: [Frame; 16],
        cursor: usize,
    }
    impl ForeignProcessors for Effect {
        fn process_segment(
            &mut self,
            node: ForeignProcessorNode<'_>,
            _: usize,
            frames: &mut [Frame],
            _: Option<&dyn hex_lattice::NativeInstrumentClock>,
        ) -> Result<(), DeviceContractError> {
            assert_eq!(
                node.prepared.total_latency_frames() as usize,
                self.delay.len()
            );
            for frame in frames {
                let input = frame.map(|sample| sample * 0.5);
                *frame = std::mem::replace(&mut self.delay[self.cursor], input);
                self.cursor = (self.cursor + 1) % self.delay.len();
            }
            Ok(())
        }
        fn committed_tail_is_drained(&self, _: ForeignProcessorNode<'_>) -> bool {
            false
        }
        fn reject_callback(&mut self) {
            panic!("valid fixture must render");
        }
    }

    let mut f = fixture(&[&[]], 128);
    let Created::Device(device) = f
        .editor
        .execute(Command::InsertMainDevice {
            before: None,
            state: state(19).unwrap(),
        })
        .unwrap()
    else {
        panic!("native effect");
    };
    let owner = ProcessorChainOwner::Main;
    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let definition = BorrowedDefinition {
        binding_epoch: Some(binding.epoch),
        unbounded_tail: false,
        instances: vec![(
            ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
            0,
        )],
    };
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let effects = NativeDeviceRegistry::new(&definitions).unwrap();
    let key = StreamSourceKey::new(3, *f.media.keys().next().unwrap());
    let sources = BTreeMap::from([(
        key.asset(),
        crate::compile::PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                128,
                128,
                hex_project::IDENTITY_CONVERSION,
                [3; 32],
            )
            .unwrap(),
        )),
    )]);
    let cancel = AtomicBool::new(false);
    let sine = Arc::new(SineTable::new());
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        f.editor.project(),
        1 << 20,
        &cancel,
    )
    .unwrap();
    let prepare = |project: &hex_project::Project| {
        let revision =
            PreparedRevision::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
                project,
                &sources,
                key.generation(),
                Arc::clone(&sine),
                None,
                &crate::audio_transform::AudioTransformPreparation::default(),
                &resources,
                64,
                &cancel,
                &effects,
                None,
            )
            .unwrap();
        assert!(revision.contains_streamed_sources());
        SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
    };
    let mut previous = prepare(f.editor.project());
    f.editor
        .execute(Command::SetLoopRegion {
            start: 8,
            end: 112,
            enabled: true,
        })
        .unwrap();
    let mut first_resize = prepare(f.editor.project());
    // Model a successor whose prepared configuration cannot inherit from the
    // old tail, but which requires the very same physical endpoint.
    first_resize.main_chain =
        foreign_chain_with_configuration(owner, binding, Arc::new(state(20).unwrap()));

    let samples = [[0.25, -0.5]; 128];
    let mut pages = Pages {
        key,
        samples: &samples,
        reads: 0,
    };
    let mut effect = Effect::default();
    let mut block = [[0.0; 2]; 8];
    previous.apply(SessionPlaybackCommand::Seek(24)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    for _ in 0..5 {
        previous
            .render_with_foreign_processors(&mut block, Some(&mut pages), None, None, &mut effect)
            .unwrap();
    }
    assert!(pages.reads > 0);
    assert_eq!(block, [[0.125, -0.25]; 8]);
    let preview = |next: &mut SessionPlayback, previous: &mut SessionPlayback| {
        next.preview_stream_handoff_with_continuation_and_report(
            previous,
            8,
            None,
            &mut |_| Ok(()),
            true,
            false,
            None,
        )
    };
    assert_eq!(
        preview(&mut first_resize, &mut previous).unwrap_err(),
        SessionPlaybackError::ForeignTailSourceBusy
    );

    previous.apply(SessionPlaybackCommand::Pause).unwrap();
    let paused_at = previous.arrangement_frame();
    let reads = pages.reads;
    previous
        .render_with_foreign_processors(&mut block, Some(&mut pages), None, None, &mut effect)
        .unwrap();
    assert_eq!(block, [[0.0; 2]; 8]);
    assert_eq!(pages.reads, reads);
    assert_eq!(
        preview(&mut first_resize, &mut previous).unwrap_err(),
        SessionPlaybackError::ForeignTailSourcePaused
    );
    assert_eq!(previous.arrangement_frame(), paused_at);
    assert_eq!(previous.arrangement_loop_state().active_region, None);

    f.editor
        .execute(Command::SetLoopRegion {
            start: 16,
            end: 72,
            enabled: true,
        })
        .unwrap();
    let mut next_resize = prepare(f.editor.project());
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    preview(&mut next_resize, &mut previous).unwrap();
    next_resize
        .try_inherit_state_live_from(&mut previous)
        .unwrap();
    assert_eq!(next_resize.arrangement_frame(), paused_at);
    for _ in 0..2 {
        next_resize
            .render_with_foreign_processors(&mut block, Some(&mut pages), None, None, &mut effect)
            .unwrap();
    }
    assert!(pages.reads > reads);
    assert!(block.iter().any(|sample| sample[0] > 0.0));
    assert_eq!(
        next_resize.arrangement_loop_state().active_region,
        Some((16, 72))
    );
    assert_eq!(next_resize.arrangement_frame(), 24);
}
