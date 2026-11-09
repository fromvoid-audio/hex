use super::*;
use crate::{Audio, Renderer};
use hex_lattice::{
    DeviceResources, NativeProcessorRuntime, PreparedNativeProcessor, PreparedProcessorFactory,
    PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
};
use hex_lattice_contract::DeviceTypeId;
use hex_project::{BeatTime, Command, Created, Editor};
use std::{any::Any, collections::BTreeMap, sync::Arc};

const WIDTH: usize = 300;
const CLIP: Frame = [0.125, -0.25];

// Nonlinear, zero-latency processing with one previous sample of state. It
// distinguishes processing the shared sum once from processing each note or
// the receiver's clip, and makes a missed seek reset observable.
#[derive(Default)]
struct MemorySquare(Frame);
impl NativeProcessorRuntime for MemorySquare {
    fn as_any(&self) -> &dyn Any {
        self
    }
    fn process_block(&mut self, frames: &mut [Frame], _: &dyn DeviceResources) {
        for frame in frames {
            for (sample, history) in frame.iter_mut().zip(&mut self.0) {
                let input = *sample;
                *sample = input * input + *history;
                *history = input * 0.125;
            }
        }
    }
    fn reset(&mut self) {
        self.0 = [0.0; 2];
    }
    fn can_copy_history_from(&self, previous: &dyn NativeProcessorRuntime) -> bool {
        previous.as_any().is::<Self>()
    }
    fn copy_history_from(&mut self, previous: &dyn NativeProcessorRuntime) {
        if let Some(previous) = previous.as_any().downcast_ref::<Self>() {
            self.0 = previous.0;
        }
    }
}
struct SquareRecipe(Option<u32>);
impl PreparedNativeProcessor for SquareRecipe {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<MemorySquare>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(MemorySquare::default())
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.0
    }
}

fn recipe(source: hex_project::SourceId, latency: Option<u32>) -> PreparedRuntimeInstance {
    PreparedRuntimeInstance {
        modulation: None,
        automation: Default::default(),
        meter: Default::default(),
        owner: ProcessorChainOwner::Source(source.value()),
        device_id: 99,
        device_type: DeviceTypeId::from_static("test.source.square"),
        matrix: StereoMatrix::identity(),
        bypassed: false,
        sample_rate: 48_000,
        configuration: None,
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(SquareRecipe(
            latency,
        )))),
    }
}

fn fixture(route: usize, move_receiver: bool) -> Project {
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(receiver) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "receiver".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("receiver")
    };
    let source = editor
        .project()
        .track(receiver)
        .unwrap()
        .source_id()
        .unwrap();
    let Created::Track(contributor) = editor
        .execute(Command::AddMidiTrack {
            name: "contributor".into(),
        })
        .unwrap()
    else {
        panic!("contributor")
    };
    editor
        .execute(Command::BindTrackSource {
            track: contributor,
            source: Some(source),
        })
        .unwrap();
    for (track, pitch) in [(receiver, 60), (contributor, 67)] {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip")
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let Created::Asset(asset) = editor
        .execute(Command::AddAsset {
            path: "receiver.wav".into(),
            sample_rate: 48_000,
            frames: WIDTH as u64,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    editor
        .execute(Command::PlaceClip {
            track: receiver,
            asset,
            start: 0,
            duration: WIDTH as u64,
            source_offset: 0,
            source_length: WIDTH as u64,
            looping: false,
        })
        .unwrap();
    if move_receiver {
        editor
            .execute(Command::SetNativeSourceReceiver {
                source,
                track: Some(contributor),
            })
            .unwrap();
    }
    match route {
        1 => {
            let Created::ReturnTrack(bus) = editor
                .execute(Command::AddReturnTrack {
                    name: "return".into(),
                })
                .unwrap()
            else {
                panic!("return")
            };
            editor
                .execute(Command::SetReturnSend {
                    track: receiver,
                    return_track: bus,
                    gain: 0.5,
                })
                .unwrap();
        }
        2 => {
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(contributor),
                    destination: receiver,
                    gain: 1.0,
                })
                .unwrap();
        }
        _ => {}
    }
    crate::compile::project(
        editor.project(),
        &BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]),
    )
    .unwrap()
}

fn with_recipe<'a>(
    project: &'a Project,
    receiver: usize,
    instances: &[PreparedRuntimeInstance],
) -> Renderer<'a> {
    let recipes = [PreparedSourceEffects {
        source: project.tracks[0].source_id().unwrap(),
        receiver,
        instances,
    }];
    Renderer {
        project,
        position: 0,
        playing: false,
        runtime: crate::RendererRuntime::with_prepared_source_effects(project, &recipes),
    }
}

#[test]
fn source_suffix_processes_shared_sum_without_touching_receiver_clips() {
    for route in 0..3 {
        let project = fixture(route, false);
        assert_eq!(project.tracks[0].source_id(), project.tracks[1].source_id());
        assert!(project.tracks[0].receives_source_audio());
        assert!(!project.tracks[1].receives_source_audio());
        let mut generated = [[0.0; 2]; WIDTH];
        crate::render_source_generators(&project, 0, 0, WIDTH as u64, &mut generated, None)
            .unwrap();
        assert!(generated.iter().any(|frame| frame[0].abs() > 0.001));
        let mut renderer = with_recipe(
            &project,
            0,
            &[recipe(project.tracks[0].source_id().unwrap(), Some(0))],
        );
        assert_eq!(renderer.runtime.compensation.is_some(), route == 2);
        renderer.play();
        let mut whole = [[0.0; 2]; WIDTH];
        renderer.render(&mut whole).unwrap();
        let mut previous = [0.0; 2];
        for (sample, raw) in whole.iter().zip(generated) {
            for channel in 0..2 {
                let dry_plus_source =
                    CLIP[channel] + raw[channel] * raw[channel] + previous[channel] * 0.125;
                let expected = if route == 1 {
                    dry_plus_source + dry_plus_source * 0.5
                } else {
                    dry_plus_source
                };
                assert!(
                    (sample[channel] - expected).abs() < 1e-6,
                    "route {route}, sample {sample:?}, expected {expected}"
                );
            }
            previous = raw;
        }
        renderer.seek(0);
        let mut split = [[0.0; 2]; WIDTH];
        renderer.render(&mut split[..17]).unwrap();
        renderer.render(&mut split[17..256]).unwrap();
        renderer.render(&mut split[256..]).unwrap();
        assert_eq!(split, whole);
    }
}

#[test]
fn invalid_prepared_source_recipes_refuse_before_rendering() {
    for (index, latency, expected) in [
        (1, Some(0), "source effects require the explicit receiver"),
        (
            0,
            Some(1),
            "source effects require native zero-latency processors at the project rate",
        ),
        (
            0,
            None,
            "source effects require native zero-latency processors at the project rate",
        ),
    ] {
        let project = fixture(0, false);
        let mut renderer = with_recipe(
            &project,
            index,
            &[recipe(project.tracks[0].source_id().unwrap(), latency)],
        );
        renderer.play();
        assert_eq!(renderer.render(&mut [[0.0; 2]; 1]).unwrap_err().0, expected);
    }
    let project = fixture(0, false);
    let source = project.tracks[0].source_id().unwrap();
    let instances = [recipe(project.tracks[0].source_id().unwrap(), Some(0))];
    let duplicates = [0, 0].map(|receiver| PreparedSourceEffects {
        source,
        receiver,
        instances: &instances,
    });
    assert!(matches!(
        SourceEffectStages::prepare(&project, &duplicates),
        Err(Invalid("source effects already have a receiver stage"))
    ));
    for wrong_owner in [
        ProcessorChainOwner::Track(source.value()),
        ProcessorChainOwner::Source(source.value() + 1),
    ] {
        let mut wrong = recipe(source, Some(0));
        wrong.owner = wrong_owner;
        let mut renderer = with_recipe(&project, 0, &[wrong]);
        renderer.play();
        assert_eq!(
            renderer.render(&mut [[0.0; 2]; 1]).unwrap_err().0,
            "source effect processor belongs to another owner"
        );
    }
}

#[test]
fn the_same_source_effect_recipe_follows_the_explicit_receiver() {
    let first = fixture(0, false);
    let moved = fixture(0, true);
    let source = first.tracks[0].source_id().unwrap();
    assert_eq!(moved.tracks[1].source_id(), Some(source));
    assert_ne!(
        first.tracks[0].canonical_track_id(),
        moved.tracks[1].canonical_track_id()
    );
    assert!(first.tracks[0].receives_source_audio());
    assert!(moved.tracks[1].receives_source_audio());
    let prepared = [recipe(source, Some(0))];
    let mut previous = with_recipe(&first, 0, &prepared);
    let mut current = with_recipe(&moved, 1, &prepared);
    previous.play();
    current.play();
    let mut old = [[0.0; 2]; WIDTH];
    let mut new = [[0.0; 2]; WIDTH];
    previous.render(&mut old).unwrap();
    current.render(&mut new).unwrap();
    assert_eq!(old, new);
}
