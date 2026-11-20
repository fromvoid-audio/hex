//! A bound effect must receive the same input on a closed stream as on an
//! independently calculated delayed route, including across loop and handoff cuts.
use super::*;
use crate::{
    BorrowedMediaPage, MediaPages, STREAM_PAGE_FRAMES, StreamResourceExtent, StreamSourceKey,
    StreamedSourceMetadata,
    compile::PreparedAudioSource,
    native_device_resources::{PreparedDeviceResources, ResidentDeviceAssets},
};
use std::collections::BTreeMap;
use std::sync::atomic::AtomicBool;

#[derive(Clone, Copy, Debug)]
enum Chain {
    Track,
    Main,
    ReturnSend,
    ReturnFeed,
}

struct ProtectedPages<'a> {
    source: StreamSourceKey,
    samples: &'a [Frame],
    extents: &'a [StreamResourceExtent],
}

impl MediaPages for ProtectedPages<'_> {
    fn page(&mut self, source: StreamSourceKey, page_index: u64) -> Option<BorrowedMediaPage<'_>> {
        assert_eq!(source, self.source);
        assert!(
            self.extents.iter().any(|extent| {
                extent.source == source.into() && extent.page_index == page_index
            }),
            "unprotected source page {page_index}"
        );
        let start = usize::try_from(page_index).unwrap() * STREAM_PAGE_FRAMES;
        Some(BorrowedMediaPage {
            page_index,
            frames: &self.samples[start..(start + STREAM_PAGE_FRAMES).min(self.samples.len())],
        })
    }

    fn request_page(&mut self, _: StreamSourceKey, _: u64) {
        panic!("the planned window must cover every page read");
    }
}

#[test]
fn bound_effect_demand_and_pcm_cross_loop_and_successor_cuts() {
    for chain in [
        Chain::Track,
        Chain::Main,
        Chain::ReturnSend,
        Chain::ReturnFeed,
    ] {
        check_loop_and_successor(chain);
    }
}

fn check_loop_and_successor(chain: Chain) {
    let loop_start = STREAM_PAGE_FRAMES - 6;
    let loop_end = STREAM_PAGE_FRAMES + 23;
    let length = loop_end + 16;
    let samples: Vec<_> = (0..length)
        .map(|frame| {
            let value = 0.125 + (frame % 32) as f32 / 256.0;
            [value, -0.5 * value]
        })
        .collect();
    let mut f = fixture(&[&[]], length);
    let track = f.tracks[0];
    let state = state(19).unwrap();
    let (command, owner, gain) = match chain {
        Chain::Track => (
            Command::InsertDevice {
                track,
                before: None,
                state,
            },
            ProcessorChainOwner::Track(track.value()),
            0.25,
        ),
        Chain::Main => (
            Command::InsertMainDevice {
                before: None,
                state,
            },
            ProcessorChainOwner::Main,
            0.25,
        ),
        Chain::ReturnSend | Chain::ReturnFeed => {
            let Created::ReturnTrack(bus) = f
                .editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return bus")
            };
            f.editor
                .execute(Command::SetReturnSend {
                    track,
                    return_track: bus,
                    gain: 1.0,
                })
                .unwrap();
            let gain = if matches!(chain, Chain::ReturnFeed) {
                let Created::Track(receiver) = f
                    .editor
                    .execute(Command::AddTrack {
                        name: "receiver".into(),
                    })
                    .unwrap()
                else {
                    panic!("audio receiver")
                };
                f.editor
                    .execute(Command::SetAudioFeed {
                        source: hex_project::RoutingOwner::Return(bus),
                        destination: receiver,
                        gain: 0.5,
                    })
                    .unwrap();
                1.375
            } else {
                1.25
            };
            (
                Command::InsertReturnDevice {
                    return_track: bus,
                    before: None,
                    state,
                },
                ProcessorChainOwner::Return(bus.value()),
                gain,
            )
        }
    };
    let Created::Device(device) = f.editor.execute(command).unwrap() else {
        panic!("effect")
    };
    f.editor
        .execute(Command::SetLoopRegion {
            start: loop_start as u64,
            end: loop_end as u64,
            enabled: true,
        })
        .unwrap();

    let binding = hex_lattice_contract::ForeignProcessorBinding {
        root: 1,
        instance: device.value(),
        graph: 1,
        epoch: 1,
    };
    let definition = BorrowedDefinition {
        binding_epoch: Some(binding.epoch),
        instances: vec![(
            ProcessorInstanceIdentity::new(owner, device.value()).unwrap(),
            3,
        )],
        unbounded_tail: false,
    };
    let definitions: [&dyn NativeDeviceDefinition; 1] = [&definition];
    let registry = NativeDeviceRegistry::new(&definitions).unwrap();
    let key = StreamSourceKey::new(3, *f.media.keys().next().unwrap());
    let sources = BTreeMap::from([(
        key.asset(),
        PreparedAudioSource::Streamed(Arc::new(
            StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                length as u64,
                length as u64,
                hex_project::IDENTITY_CONVERSION,
                [3; 32],
            )
            .unwrap(),
        )),
    )]);
    let sine = Arc::new(SineTable::new());
    let cancel = AtomicBool::new(false);
    let transforms = crate::audio_transform::AudioTransformPreparation::default();
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        f.editor.project(),
        1 << 20,
        &cancel,
    )
    .unwrap();
    let prepare = |project: &hex_project::Project| {
        let revision = PreparedRevision::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
            project, &sources, key.generation(), Arc::clone(&sine), None,
            &transforms, &resources, 64, &cancel, &registry, None,
        ).unwrap();
        SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap()
    };
    let mut runtime = prepare(f.editor.project());
    let seek = runtime
        .prepare_stream_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::PlayFrom(loop_start as u64),
        ))
        .unwrap();
    let mut extents = Vec::new();
    let collect = |extents: &mut Vec<_>, extent| {
        extents.push(extent);
        Ok(())
    };
    let initial = runtime
        .visit_prepared_stream_action(&seek, 11, &mut |extent| collect(&mut extents, extent))
        .unwrap();
    assert_eq!(initial.frames, 11);
    assert_eq!(
        extents.len(),
        2,
        "the first callback crosses a page boundary"
    );
    runtime
        .apply(SessionPlaybackCommand::Seek(loop_start as u64))
        .unwrap();
    runtime.apply(SessionPlaybackCommand::Play).unwrap();

    let mut endpoint = Borrowed::new(owner, device.value());
    let mut output = Vec::new();
    let mut render = |runtime: &mut SessionPlayback, extents: &[StreamResourceExtent], frames| {
        let start = runtime.arrangement_frame() as usize;
        for frame in start..start + frames {
            let page = (frame / STREAM_PAGE_FRAMES) as u64;
            let offset = frame % STREAM_PAGE_FRAMES;
            assert!(
                extents.iter().any(|extent| {
                    extent.source == key.into()
                        && extent.page_index == page
                        && (extent.start..extent.end).contains(&offset)
                }),
                "{chain:?} source frame {frame} is outside the protected ranges"
            );
        }
        let mut block = [[0.0; 2]; 11];
        endpoint.next_offset = 0;
        runtime
            .render_with_foreign_processors(
                &mut block[..frames],
                Some(&mut ProtectedPages {
                    source: key,
                    samples: &samples,
                    extents,
                }),
                None,
                None,
                &mut endpoint,
            )
            .unwrap();
        output.extend_from_slice(&block[..frames]);
    };
    render(&mut runtime, &extents, initial.frames);
    f.editor
        .execute(Command::RenameTrack {
            track,
            name: "successor".into(),
        })
        .unwrap();
    let mut successor = prepare(f.editor.project());
    extents.clear();
    let before = (runtime.performance_frame(), runtime.arrangement_frame());
    let (handoff, report) = successor
        .preview_stream_handoff_with_report(&mut runtime, 11, None, &mut |extent| {
            collect(&mut extents, extent)
        })
        .unwrap();
    assert!(!report.running_audio_restart());
    assert_eq!(
        (runtime.performance_frame(), runtime.arrangement_frame()),
        before
    );
    assert!(
        !successor
            .try_inherit_state_live_from(&mut runtime)
            .unwrap()
            .running_audio_restart()
    );
    assert_eq!(
        (successor.performance_frame(), successor.arrangement_frame()),
        before
    );
    render(&mut successor, &extents, handoff.frames);
    for _ in 0..6 {
        extents.clear();
        let window = successor
            .visit_next_stream_chunk(11, None, &mut |extent| collect(&mut extents, extent))
            .unwrap();
        assert!(window.frames > 0);
        render(&mut successor, &extents, window.frames);
    }

    assert_eq!(endpoint.processed, output.len());
    assert!(output.len() >= 19 + 2 * (loop_end - loop_start));
    for (frame, actual) in output.into_iter().enumerate() {
        let expected = frame.checked_sub(19).map_or([0.0; 2], |input| {
            let position = loop_start + input % (loop_end - loop_start);
            // The 29-frame loop attenuates its last seven input frames before
            // the effect and leaves the first sample at full level.
            let seam_gain = ((loop_end - position - 1) as f32 / 6.0).min(1.0);
            samples[position].map(|sample| sample * seam_gain * gain)
        });
        for (actual, expected) in actual.into_iter().zip(expected) {
            assert!(
                (actual - expected).abs() < 1.0e-7,
                "{chain:?} output frame {frame}: {actual} != {expected}"
            );
        }
    }
}
