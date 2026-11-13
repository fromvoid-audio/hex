use super::*;
use crate::{Audio, Renderer};
use hex_lattice::{
    DeviceResources, NativeInstrumentClock, NativeProcessorRuntime, PreparedNativeProcessor,
    PreparedProcessorFactory, PreparedRuntimeInstance, ProcessorChainOwner, StereoMatrix,
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
struct MemorySquare(Frame, bool);
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
    fn process_block_with_clock(
        &mut self,
        frames: &mut [Frame],
        clock: Option<&dyn NativeInstrumentClock>,
        resources: &dyn DeviceResources,
    ) {
        self.process_block(frames, resources);
        if self.1 {
            for (offset, frame) in frames.iter_mut().enumerate() {
                let stamp = clock
                    .and_then(|clock| match clock.transport_is_running()? {
                        true => clock.performance_frame_at_offset(offset),
                        false => clock.arrangement_frame_at_offset(offset),
                    })
                    .map_or(-1.0, |frame| frame as f32)
                    * (1.0 / 65_536.0);
                for sample in frame {
                    *sample += stamp;
                }
            }
        }
    }
    fn reset(&mut self) {
        self.0 = [0.0; 2];
    }
    fn is_quiescent(&self) -> bool {
        !self.1 && self.0 == [0.0; 2]
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
struct SquareRecipe {
    latency: Option<u32>,
    stamp_clock: bool,
    tail: hex_lattice::OutputTail,
}
impl PreparedNativeProcessor for SquareRecipe {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_runtime_bytes(&self) -> usize {
        std::mem::size_of::<MemorySquare>()
    }
    fn create_runtime(&self) -> Box<dyn NativeProcessorRuntime> {
        Box::new(MemorySquare([0.0; 2], self.stamp_clock))
    }
    fn output_tail(&self) -> hex_lattice::OutputTail {
        self.tail
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        self.latency
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
        definition: Some(PreparedProcessorFactory::Legacy(Arc::new(SquareRecipe {
            latency,
            stamp_clock: false,
            tail: hex_lattice::OutputTail::Finite { max_tail_frames: 1 },
        }))),
    }
}

fn authored_fixture(route: usize, move_receiver: bool) -> (Editor, hex_project::AssetId) {
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
    (editor, asset)
}

fn fixture(route: usize, move_receiver: bool) -> Project {
    let (editor, asset) = authored_fixture(route, move_receiver);
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
        factory_bytes: 0,
        source: project.tracks[0].source_id().unwrap(),
        receiver,
        instances: Arc::from(instances),
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
        factory_bytes: 0,
        source,
        receiver,
        instances: Arc::from(instances.as_slice()),
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

fn prepare_suffix(
    project: &hex_project::Project,
    source: hex_project::SourceId,
) -> Result<Vec<PreparedRuntimeInstance>, Invalid> {
    crate::compile::prepare_source_effect_suffix(
        project,
        source,
        &hex_devices::native_registry().unwrap(),
        &hex_lattice::DevicePreparationContext {
            sample_rate: project.sample_rate(),
            tuning: &hex_lattice::TuningTable::default(),
            resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
        },
    )
}

#[test]
fn registry_suffix_respects_order_and_power_without_processing_receiver_clips() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let (mut editor, asset) = authored_fixture(0, false);
    let model = editor.project();
    let receiver = model.tracks()[0].id();
    let contributor = model.tracks()[1].id();
    let source = model.tracks()[0].source_id().unwrap();
    let instrument = model.native_source(source).unwrap().instrument().id();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    use crate::native_device_resources::{
        DeviceResourceAdmission, DeviceResourcePhase, PreparedDeviceResources,
        ResidentDeviceAssets, inspect_device_resource_union,
    };
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let resources = PreparedDeviceResources::prepare_on_control(
        &ResidentDeviceAssets::default(),
        model,
        1 << 20,
        &cancel,
    )
    .unwrap();
    let mut revision = crate::PreparedRevision::prepare_with_device_resources(
        model,
        &media,
        Arc::new(crate::SineTable::new()),
        None,
        &crate::audio_transform::AudioTransformPreparation::default(),
        &resources,
        0,
        &cancel,
    )
    .unwrap();
    let measure = |revision: &crate::PreparedRevision| {
        let registered = [DeviceResourceAdmission::prepared_on_control(
            &resources, revision,
        )];
        let phase = DeviceResourcePhase {
            stores: &[],
            registered: &registered,
            selections: &[],
            outbound: &[],
            legacy: &[],
            checked_runtime_bytes: 0,
            construction_bytes: 0,
        };
        let budget = crate::AudioStorageLedgerReservation {
            data_rows: 64,
            handle_rows: 64,
            root_rows: 16,
            max_retained_bytes: 1 << 20,
            max_construction_peak_bytes: 1 << 20,
        };
        inspect_device_resource_union(&phase, &phase, budget)
            .unwrap()
            .retained
            .retained_bytes
    };
    let plain_retained = measure(&revision);
    let mut compiled = revision.arrangement().as_ref().clone();
    assert!(compiled.source_effects.is_empty());
    assert!(prepare_suffix(model, source).unwrap().is_empty());

    // Prepare the bound receiver separately, then author the suffix on the
    // detached copy to verify that dormant definitions have no retained charge.
    for track in [receiver, contributor] {
        editor
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    let utility = UTILITY_DEFINITION.default_state().unwrap();
    let invert = UTILITY_DEFINITION
        .patch_parameter(&utility, parameter::INVERT_LEFT, ParameterValue::Bool(true))
        .unwrap();
    let swap = UTILITY_DEFINITION
        .patch_parameter(&utility, parameter::CHANNEL, ParameterValue::Choice(3))
        .unwrap();
    let Created::Device(invert_id) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state: invert,
        })
        .unwrap()
    else {
        panic!("inverting child")
    };
    let Created::Device(swap_id) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state: swap,
        })
        .unwrap()
    else {
        panic!("swapping sibling")
    };
    editor
        .execute(Command::ReparentDevice {
            device: swap_id,
            parent: None,
        })
        .unwrap();

    let native = hex_devices::native_registry().unwrap();
    let context = hex_lattice::DevicePreparationContext {
        sample_rate: 48_000,
        tuning: &hex_lattice::TuningTable::default(),
        resources: &hex_lattice::EMPTY_DEVICE_RESOURCES,
    };
    let prepare = |compiled: &Project| {
        crate::compile::prepare_source_effect_recipes(
            compiled,
            editor.project(),
            &native,
            &context,
            None,
        )
        .unwrap()
    };
    compiled.source_effects = prepare(&compiled);
    let mut receiver_only = compiled.clone();
    receiver_only.tracks.pop().unwrap();
    let retained = metadata::retained_bytes(&compiled.source_effects).unwrap();
    assert!(retained > 0);
    assert_eq!(
        retained,
        metadata::retained_bytes(&prepare(&receiver_only)).unwrap()
    );
    assert!(metadata::construction_for([2].into_iter()).unwrap() >= retained);
    assert_eq!(
        metadata::construction_bytes(editor.project(), None),
        Some(0)
    );
    assert_eq!(compiled.source_effects.len(), 1);
    let plan = &compiled.source_effects[0];
    assert_eq!(plan.source, source);
    assert_eq!(plan.receiver, 0);
    assert_eq!(plan.instances.len(), 2);
    for (entry, id) in plan.instances.iter().zip([invert_id, swap_id]) {
        assert_eq!(entry.owner, ProcessorChainOwner::Source(source.value()));
        assert_eq!(entry.device_id, id.value());
        assert!(!entry.bypassed);
    }
    let mut renderer = Renderer::new(&compiled);
    renderer.play();
    let mut rendered = [[0.0; 2]; WIDTH];
    renderer.render(&mut rendered).unwrap();
    revision.test_install_source_effect_recipes(prepare(&compiled));
    assert_eq!(measure(&revision) - plain_retained, retained);
    let mut session =
        crate::SessionPlayback::new_with_device_resources(Arc::new(revision), &resources).unwrap();
    session.apply(crate::SessionPlaybackCommand::Play).unwrap();
    let mut session_pcm = [[0.0; 2]; WIDTH];
    session.render(&mut session_pcm).unwrap();
    for (session, offline) in session_pcm.iter().zip(rendered) {
        assert!((session[0] - offline[0]).abs() < 1e-6);
        assert!((session[1] - offline[1]).abs() < 1e-6);
    }
    let mut source_audio = [[0.0; 2]; WIDTH];
    crate::render_source_generators(&compiled, 0, 0, WIDTH as u64, &mut source_audio, None)
        .unwrap();
    assert!(source_audio.iter().any(|frame| frame[0].abs() > 0.001));
    for (wet, [left, right]) in rendered.iter().zip(source_audio) {
        assert!((wet[0] - (CLIP[0] + right)).abs() < 1e-6);
        assert!((wet[1] - (CLIP[1] - left)).abs() < 1e-6);
    }

    editor
        .execute(Command::SetDeviceBypass {
            device: instrument,
            bypassed: true,
        })
        .unwrap();
    let bypassed = prepare_suffix(editor.project(), source).unwrap();
    assert!(bypassed[0].bypassed);
    assert!(!bypassed[1].bypassed);
    assert!(!editor.project().device(invert_id).unwrap().bypassed());
}

#[test]
fn registry_suffix_refuses_prefix_latency_foreign_and_unknown_effects() {
    use hex_lattice_contract::{NativeDeviceMetadata, NativeDeviceState};

    let utility = hex_devices::utility::UTILITY_DEFINITION
        .default_state()
        .unwrap();
    let limiter = hex_devices::limiter::DEFINITION.default_state().unwrap();
    let foreign = NativeDeviceState::new(DeviceTypeId::from_static("hex.vst3"), 1, vec![]).unwrap();
    let unknown =
        NativeDeviceState::new(DeviceTypeId::from_static("test.unknown"), 1, vec![]).unwrap();
    let instrument = hex_project::default_instrument_state().unwrap();
    let mut editor = Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    for (states, instrument_index, expected) in [
        (
            vec![utility, instrument.clone()],
            1,
            "source effects before the instrument are not supported",
        ),
        (
            vec![instrument.clone(), limiter],
            0,
            "source effects require native zero-latency processors",
        ),
        (
            vec![instrument.clone(), foreign],
            0,
            "foreign source effects are not supported",
        ),
        (
            vec![instrument, unknown],
            0,
            "native device state is unsupported or malformed",
        ),
    ] {
        let Created::NativeSource(source) = editor
            .execute(Command::CreateNativeSource {
                name: "retained".into(),
                devices: states,
                instrument_index,
            })
            .unwrap()
        else {
            panic!("retained source")
        };
        assert_eq!(
            prepare_suffix(editor.project(), source).err(),
            Some(Invalid(expected))
        );
    }
    // Retaining definitions alone must not prepare or admit any of these devices.
    assert!(crate::compile::project(editor.project(), &BTreeMap::new()).is_ok());
}

#[test]
fn session_playing_paths_run_source_effects_once_before_mixing_receiver_clips() {
    use crate::{
        LaunchQuantization, PreparedRevision, SessionPlayback, SessionPlaybackCommand,
        SessionPlaybackError,
    };

    for route in [0, 2] {
        for console in [false, true] {
            let (mut editor, asset) = authored_fixture(route, false);
            let contributor = editor.project().tracks()[1].id();
            let source = editor.project().tracks()[0].source_id().unwrap();
            let session_clip = if console {
                let Created::Scene(scene) = editor
                    .execute(Command::AddScene {
                        name: "scene".into(),
                    })
                    .unwrap()
                else {
                    panic!("scene")
                };
                let Created::SessionClip(clip) = editor
                    .execute(Command::CreateSessionMidiClip {
                        track: contributor,
                        scene,
                        name: "console note".into(),
                        source_length: BeatTime::ONE,
                    })
                    .unwrap()
                else {
                    panic!("session clip")
                };
                editor
                    .execute(Command::AddMidiNote {
                        clip,
                        start: BeatTime::ZERO,
                        duration: BeatTime::ONE,
                        pitch: 76,
                        channel: 0,
                        velocity: 100,
                    })
                    .unwrap();
                Some(clip)
            } else {
                None
            };
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let revision = Arc::new(
                PreparedRevision::prepare(
                    editor.project(),
                    &media,
                    Arc::new(crate::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            assert_eq!(
                revision
                    .arrangement()
                    .signal_graph
                    .as_ref()
                    .is_some_and(|graph| graph.needs_upstream_frame_routing()),
                route == 2,
            );
            let mut clocked = recipe(source, Some(0));
            clocked.definition = Some(PreparedProcessorFactory::Legacy(Arc::new(SquareRecipe {
                latency: Some(0),
                stamp_clock: true,
                tail: hex_lattice::OutputTail::Finite { max_tail_frames: 1 },
            })));
            let instances = [clocked];
            let recipes = [PreparedSourceEffects {
                factory_bytes: 0,
                source,
                receiver: 0,
                instances: Arc::from(instances.as_slice()),
            }];
            let start = |playback: &mut SessionPlayback| {
                playback.apply(SessionPlaybackCommand::Play).unwrap();
                if let Some(clip) = session_clip {
                    playback
                        .apply(SessionPlaybackCommand::LaunchClip {
                            clip,
                            quantization: LaunchQuantization::Immediate,
                        })
                        .unwrap();
                }
            };
            let mut dry = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            let mut wet =
                SessionPlayback::test_with_source_effects(Arc::clone(&revision), &recipes).unwrap();
            start(&mut dry);
            start(&mut wet);
            let mut baseline = [[0.0; 2]; WIDTH];
            let mut actual = [[0.0; 2]; WIDTH];
            dry.render(&mut baseline).unwrap();
            wet.render(&mut actual[..17]).unwrap();
            wet.render(&mut actual[17..]).unwrap();
            assert!(baseline.iter().any(|f| (f[0] - CLIP[0]).abs() > 0.001));

            let mut previous = [0.0; 2];
            for (frame, (sample, dry)) in actual.iter().zip(baseline).enumerate() {
                let source = [dry[0] - CLIP[0], dry[1] - CLIP[1]];
                let stamp = frame as f32 * (1.0 / 65_536.0);
                for ch in 0..2 {
                    let expected =
                        CLIP[ch] + source[ch] * source[ch] + previous[ch] * 0.125 + stamp;
                    assert!(
                        (sample[ch] - expected).abs() < 1e-6,
                        "route {route}, console {console}: {sample:?} vs {expected}"
                    );
                }
                previous = source;
            }
            if !console {
                wet.apply(SessionPlaybackCommand::Seek(0)).unwrap();
                let mut restarted = [[0.0; 2]; WIDTH];
                wet.render(&mut restarted).unwrap();
                assert_eq!(restarted, actual);
            }
            dry.apply(SessionPlaybackCommand::Pause).unwrap();
            wet.apply(SessionPlaybackCommand::Pause).unwrap();
            let paused_at = wet.performance_frame();
            let mut paused_dry = [[0.0; 2]; 31];
            let mut paused_wet = [[0.0; 2]; 31];
            dry.render(&mut paused_dry).unwrap();
            wet.render(&mut paused_wet).unwrap();
            let stamp = paused_at as f32 * (1.0 / 65_536.0);
            for (wet, dry) in paused_wet.iter().zip(paused_dry) {
                for ch in 0..2 {
                    let expected = dry[ch] * dry[ch] + previous[ch] * 0.125 + stamp;
                    assert!((wet[ch] - expected).abs() < 1e-6);
                }
                previous = dry;
            }
            assert_eq!(wet.performance_frame(), paused_at);
            wet.apply(SessionPlaybackCommand::Stop).unwrap();
            let mut stopped = [[1.0; 2]; 31];
            wet.render(&mut stopped).unwrap();
            assert_eq!(stopped, [[0.0; 2]; 31]);
            let mut candidate = SessionPlayback::new(Arc::clone(&revision)).unwrap();
            assert!(matches!(
                candidate.try_inherit_state_from(&mut wet),
                Err(SessionPlaybackError::Arrangement(
                    "source effect handoff requires the same prepared source configuration"
                ))
            ));
        }
    }
}

/// The quote tests must refuse before asking even one valid factory to build.
struct BudgetFactory(usize);

impl hex_lattice::PreparedOwnedProcessorRuntime for BudgetFactory {
    fn runtime_version(&self) -> u32 {
        1
    }
    fn declared_pair_bytes(&self) -> usize {
        self.0
    }
    fn create_runtime(
        &self,
    ) -> Result<
        Box<dyn hex_lattice::OwnedProcessorRuntime>,
        hex_lattice_contract::DeviceContractError,
    > {
        panic!("over-budget source runtime must not be constructed")
    }
    fn output_tail(&self) -> hex_lattice::OutputTail {
        hex_lattice::OutputTail::Finite { max_tail_frames: 0 }
    }
    fn algorithmic_latency_frames(&self) -> Option<u32> {
        Some(0)
    }
}

#[test]
fn aggregate_track_return_main_and_source_budgets_refuse_before_runtime_creation() {
    let mut project = fixture(1, false);
    let source = project.tracks[0].source_id().unwrap();
    let instance = |owner, id, bytes| {
        let mut instance = recipe(source, Some(0));
        instance.owner = owner;
        instance.device_id = id;
        instance.definition = Some(PreparedProcessorFactory::Owned(Arc::new(BudgetFactory(
            bytes,
        ))));
        instance
    };
    let devices = |owner, count, bytes| {
        Arc::from(
            (1..=count as u64)
                .map(|id| instance(owner, id, bytes))
                .collect::<Vec<_>>(),
        )
    };
    let source_instances = [instance(
        ProcessorChainOwner::Source(source.value()),
        999,
        1,
    )];
    let recipes = [PreparedSourceEffects {
        factory_bytes: 0,
        source,
        receiver: 0,
        instances: Arc::from(source_instances.as_slice()),
    }];
    let slots = hex_project::MAX_DEVICES_PER_TRACK;
    assert_eq!(slots * 4, hex_lattice::MAX_STATEFUL_PROCESSORS);
    for track in &mut project.tracks {
        let owner = ProcessorChainOwner::Track(track.canonical_track_id().unwrap().value());
        track.device_chain = devices(owner, slots, 1);
    }
    project.return_buses[0].device_chain = devices(
        ProcessorChainOwner::Return(project.return_buses[0].id.value()),
        slots,
        1,
    );
    project.main_device_chain = devices(ProcessorChainOwner::Main, slots, 1);
    assert!(matches!(
        SourceEffectStages::prepare(&project, &recipes),
        Err(Invalid(
            "source effects exceed the aggregate graph runtime budget"
        ))
    ));

    let mut project = fixture(0, false);
    let bytes = hex_lattice::MAX_OWNED_PROCESSOR_PAIR_BYTES;
    project.main_device_chain = devices(ProcessorChainOwner::Main, 2, bytes);
    // A refused offline graph must not instantiate Main either.
    let mut renderer = with_recipe(&project, 0, &source_instances);
    renderer.play();
    assert_eq!(
        renderer.render(&mut [[0.0; 2]]).unwrap_err(),
        Invalid("source effects exceed the aggregate graph runtime budget")
    );
}

#[test]
fn moving_a_loop_behind_the_cursor_keeps_source_effect_history() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};

    let (mut editor, asset) = authored_fixture(0, false);
    let source = editor.project().tracks()[0].source_id().unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let table = Arc::new(crate::SineTable::new());
    let mut instance = recipe(source, Some(0));
    instance.configuration = Some(Arc::new(
        hex_lattice_contract::NativeDeviceState::new(instance.device_type, 1, vec![1]).unwrap(),
    ));
    let recipe = PreparedSourceEffects {
        factory_bytes: 0,
        source,
        receiver: 0,
        instances: Arc::from([instance]),
    };
    let mut make = |end| {
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end,
                enabled: true,
            })
            .unwrap();
        let revision =
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap();
        SessionPlayback::test_with_source_effects(Arc::new(revision), std::slice::from_ref(&recipe))
            .unwrap()
    };
    let mut playing = make(1024);
    let mut oracle = make(1024);
    for runtime in [&mut playing, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 1120]).unwrap();
        assert_eq!(runtime.arrangement_frame(), 96);
    }
    let mut edited = make(64);
    let (_, preview) = edited
        .preview_stream_handoff_with_policy_and_report(
            &mut playing,
            32,
            None,
            &mut |_| Ok(()),
            true,
            false,
        )
        .unwrap();
    assert!(!preview.running_audio_restart());
    let mut donor_after_preview = [[0.0; 2]; 32];
    let mut oracle_after_preview = donor_after_preview;
    playing.render(&mut donor_after_preview).unwrap();
    oracle.render(&mut oracle_after_preview).unwrap();
    assert_eq!(
        donor_after_preview, oracle_after_preview,
        "preview must not transfer source-effect history"
    );
    edited.try_inherit_state_live_from(&mut playing).unwrap();
    assert_eq!(
        (edited.arrangement_frame(), edited.performance_frame()),
        (128, 1152)
    );
    // Neither runtime reaches a wrap during this block. Matching PCM proves
    // the retained effect (with a one-sample memory) did not restart at the edit.
    let mut actual = [[0.0; 2]; 32];
    let mut expected = actual;
    edited.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        (edited.arrangement_frame(), edited.performance_frame()),
        (160, 1184)
    );
}

#[test]
fn receiver_move_continues_the_source_effect_and_incompatible_recipe_leaves_donor_intact() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand, SessionPlaybackError};
    use hex_lattice_contract::NativeDeviceState;

    let table = Arc::new(crate::SineTable::new());
    let build = |moved, previous| {
        let (editor, asset) = authored_fixture(0, moved);
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), previous)
                .unwrap(),
        )
    };
    let before = build(false, None);
    let after = build(true, Some(before.session()));
    let source = before.arrangement().tracks[0].source_id().unwrap();
    assert_eq!(after.arrangement().tracks[1].source_id(), Some(source));
    assert!(after.arrangement().tracks[1].receives_source_audio());

    let mut instance = recipe(source, Some(0));
    instance.configuration = Some(Arc::new(
        NativeDeviceState::new(instance.device_type, 1, vec![1]).unwrap(),
    ));
    let make = |revision, receiver, instances: &[PreparedRuntimeInstance]| {
        SessionPlayback::test_with_source_effects(
            revision,
            &[PreparedSourceEffects {
                factory_bytes: 0,
                source,
                receiver,
                instances: Arc::from(instances),
            }],
        )
        .unwrap()
    };
    let mut donor = make(Arc::clone(&before), 0, std::slice::from_ref(&instance));
    let mut reference = make(Arc::clone(&before), 0, std::slice::from_ref(&instance));
    for playback in [&mut donor, &mut reference] {
        playback.apply(SessionPlaybackCommand::Play).unwrap();
        playback.render(&mut [[0.0; 2]; 97]).unwrap();
    }
    let mut changed = instance.clone();
    changed.device_id += 1;
    let mut wrong = make(Arc::clone(&after), 1, &[changed]);
    assert!(matches!(
        wrong.try_inherit_state_from(&mut donor),
        Err(SessionPlaybackError::Arrangement(
            "source effect handoff requires the same prepared source configuration"
        ))
    ));
    assert_eq!(donor.performance_frame(), 97);
    assert_eq!(wrong.performance_frame(), 0);
    assert!(wrong.try_inherit_state_live_from(&mut donor).is_err());
    assert_eq!(wrong.performance_frame(), 0);
    let mut restart = make(Arc::clone(&after), 1, std::slice::from_ref(&instance));
    restart.request_live_restart();
    assert!(restart.try_inherit_state_live_from(&mut donor).is_err());
    assert_eq!(restart.performance_frame(), 0);
    assert_eq!(donor.performance_frame(), 97);

    let mut moved = make(after, 1, &[instance]);
    moved.try_inherit_state_live_from(&mut donor).unwrap();
    assert_eq!(moved.performance_frame(), 97);
    let mut actual = [[0.0; 2]; 173];
    let mut expected = [[0.0; 2]; 173];
    moved.render(&mut actual).unwrap();
    reference.render(&mut expected).unwrap();
    for (i, (actual, expected)) in actual.iter().zip(expected).enumerate() {
        assert!(
            (actual[0] - expected[0]).abs() < 1e-6 && (actual[1] - expected[1]).abs() < 1e-6,
            "at {i}: moved {actual:?} vs uninterrupted {expected:?}"
        );
    }
}

#[test]
fn ordinary_runtime_quote_covers_the_constructed_source_effect_stage() {
    use crate::{PreparedRevision, SessionPlayback};

    let (editor, asset) = authored_fixture(0, false);
    let source = editor.project().tracks()[0].source_id().unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let plain_quote = SessionPlayback::interactive_runtime_allowance(&revision).unwrap();
    let plain_actual = {
        let playback = SessionPlayback::new(Arc::clone(&revision)).unwrap();
        playback.accounted_runtime_storage_bytes().unwrap()
    };

    let mut revision = Arc::try_unwrap(revision).unwrap();
    revision.test_install_source_effect_recipes(vec![PreparedSourceEffects {
        factory_bytes: 0,
        source,
        receiver: 0,
        instances: Arc::from([recipe(source, Some(0))]),
    }]);
    let effect_quote = SessionPlayback::interactive_runtime_allowance(&revision).unwrap();
    let playback = SessionPlayback::new(Arc::new(revision)).unwrap();
    let effect_actual = playback.accounted_runtime_storage_bytes().unwrap();
    assert!(effect_actual > plain_actual);
    assert_eq!(effect_quote - plain_quote, effect_actual - plain_actual);
    assert!(effect_quote >= effect_actual);
}

#[test]
fn linear_export_uses_source_effect_pcm_and_declared_tail_only_on_its_path() {
    use crate::{OutputCaptureId, PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    let (editor, asset) = authored_fixture(1, false);
    let source = editor.project().tracks()[0].source_id().unwrap();
    let receiving = [OutputCaptureId::Track(editor.project().tracks()[0].id())];
    let sent = [OutputCaptureId::Return(
        editor.project().return_tracks()[0].id(),
    )];
    let unrelated = [OutputCaptureId::Track(editor.project().tracks()[1].id())];
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let mut revision = PreparedRevision::prepare(
        editor.project(),
        &media,
        Arc::new(crate::SineTable::new()),
        None,
    )
    .unwrap();
    let main_end = revision
        .arrangement()
        .untrimmed_main_end_frame()
        .unwrap()
        .unwrap();
    let receiving_end = revision
        .arrangement()
        .untrimmed_capture_end_frame(&receiving)
        .unwrap()
        .unwrap();
    let sent_end = revision
        .arrangement()
        .untrimmed_capture_end_frame(&sent)
        .unwrap()
        .unwrap();
    let unrelated_end = revision
        .arrangement()
        .untrimmed_capture_end_frame(&unrelated)
        .unwrap()
        .unwrap();

    let mut dry_pcm = [[0.0; 2]; WIDTH];
    {
        let mut dry = Renderer::new(revision.arrangement());
        dry.play();
        dry.render(&mut dry_pcm).unwrap();
    }

    let instance = recipe(source, Some(0));
    revision.test_install_source_effect_recipes(vec![PreparedSourceEffects {
        factory_bytes: 0,
        source,
        receiver: 0,
        instances: Arc::from([instance.clone()]),
    }]);
    let revision = Arc::new(revision);
    let compiled = revision.arrangement();
    assert_eq!(
        compiled.untrimmed_main_end_frame().unwrap(),
        Some(main_end + 1)
    );
    assert_eq!(
        compiled.untrimmed_capture_end_frame(&receiving).unwrap(),
        Some(receiving_end + 1)
    );
    assert_eq!(
        compiled.untrimmed_capture_end_frame(&unrelated).unwrap(),
        Some(unrelated_end)
    );

    assert_eq!(
        compiled.untrimmed_capture_end_frame(&sent).unwrap(),
        Some(sent_end + 1)
    );

    let mut ordinary = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut export = SessionPlayback::new_linear_arrangement_export(Arc::clone(&revision)).unwrap();
    let mut ordinary_pcm = [[0.0; 2]; WIDTH];
    let mut export_pcm = [[0.0; 2]; WIDTH];
    ordinary.apply(SessionPlaybackCommand::Play).unwrap();
    export.apply(SessionPlaybackCommand::Play).unwrap();
    ordinary.render(&mut ordinary_pcm).unwrap();
    export.render(&mut export_pcm).unwrap();
    assert_eq!(export_pcm, ordinary_pcm);
    assert_ne!(export_pcm, dry_pcm);

    let mut with_tail = compiled.as_ref().clone();
    let mut bypassed = instance.clone();
    bypassed.bypassed = true;
    with_tail.source_effects[0].instances = Arc::from([bypassed]);
    assert_eq!(
        with_tail.untrimmed_main_end_frame().unwrap(),
        Some(main_end)
    );

    let mut unbounded = instance;
    unbounded.definition = Some(PreparedProcessorFactory::Legacy(Arc::new(SquareRecipe {
        latency: Some(0),
        stamp_clock: false,
        tail: hex_lattice::OutputTail::Unbounded,
    })));
    with_tail.source_effects[0].instances = Arc::from([unbounded]);
    assert!(with_tail.untrimmed_main_end_frame().is_err());
    assert!(with_tail.untrimmed_capture_end_frame(&receiving).is_err());
    assert!(with_tail.untrimmed_capture_end_frame(&sent).is_err());
    assert_eq!(
        with_tail.untrimmed_capture_end_frame(&unrelated).unwrap(),
        Some(unrelated_end)
    );
}

#[test]
fn registered_source_effect_factory_bounds_cover_each_supported_factory() {
    use hex_lattice_contract::NativeDeviceMetadata;
    let (mut editor, _) = authored_fixture(0, false);
    let source = editor.project().tracks()[0].source_id().unwrap();
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    let tracks: Vec<_> = editor
        .project()
        .tracks()
        .iter()
        .map(|track| track.id())
        .collect();
    for track in tracks {
        editor
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    let registry = hex_devices::native_registry().unwrap();
    let definitions: [&dyn NativeDeviceMetadata; 7] = [
        &hex_devices::auto_pan::AUTO_PAN_DEFINITION,
        &hex_devices::compressor::COMPRESSOR_DEFINITION,
        &hex_devices::delay::DEFINITION,
        &hex_devices::eq::EQ_DEFINITION,
        &hex_devices::eq8::EQ8_DEFINITION,
        &hex_devices::gate::GATE_DEFINITION,
        &hex_devices::reverb::REVERB_DEFINITION,
    ];
    for definition in definitions {
        let state = definition.default_state().unwrap();
        metadata::validate_authored(&state, &registry).unwrap();
        editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state,
            })
            .unwrap();
    }
    let unquoted = hex_devices::saturator::SATURATOR_DEFINITION
        .default_state()
        .unwrap();
    assert!(metadata::validate_authored(&unquoted, &registry).is_err());
    let instances = prepare_suffix(editor.project(), source).unwrap();
    assert_eq!(instances.len(), definitions.len());
    let factory_bytes = instances
        .iter()
        .map(|instance| {
            assert!(instance.definition.is_some());
            metadata::factory_bytes(instance, &registry).unwrap()
        })
        .sum::<usize>();
    assert!(factory_bytes > 0);
    let recipes = vec![PreparedSourceEffects {
        source,
        receiver: 0,
        factory_bytes,
        instances: Arc::from(instances),
    }];
    assert!(
        metadata::construction_for([definitions.len()].into_iter()).unwrap()
            >= metadata::retained_bytes(&recipes).unwrap()
    );
}

#[test]
fn ordinary_live_source_attachment_starts_empty_and_retirement_requires_a_quiet_stop() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    let (editor, asset) = authored_fixture(0, false);
    let source = editor.project().tracks()[0].source_id().unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let prepare = || {
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap()
    };
    let dry = Arc::new(prepare());
    let mut wet = prepare();
    wet.test_install_source_effect_recipes(vec![PreparedSourceEffects {
        source,
        receiver: 0,
        instances: Arc::from([recipe(source, Some(0))]),
        factory_bytes: 0,
    }]);
    let wet = Arc::new(wet);
    let playback = |revision| SessionPlayback::new(revision).unwrap();
    let mut previous = playback(Arc::clone(&dry));
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut dry_prefix = [[0.0; 2]; 98];
    previous.render(&mut dry_prefix[..97]).unwrap();
    let mut independent = playback(Arc::clone(&dry));
    independent.apply(SessionPlaybackCommand::Play).unwrap();
    independent.render(&mut dry_prefix).unwrap();

    let mut added = playback(Arc::clone(&wet));
    added.try_inherit_state_live_from(&mut previous).unwrap();
    assert_eq!(added.performance_frame(), 97);
    let mut sample = [[0.0; 2]];
    added.render(&mut sample).unwrap();
    for (channel, clip) in CLIP.into_iter().enumerate() {
        let generator = dry_prefix[97][channel] - clip;
        assert!(generator.abs() > 1e-5);
        assert!((sample[0][channel] - (clip + generator * generator)).abs() < 1e-6);
    }

    let mut removed = playback(Arc::clone(&dry));
    assert!(removed.try_inherit_state_live_from(&mut added).is_err());
    added.apply(SessionPlaybackCommand::Pause).unwrap();
    assert!(removed.try_inherit_state_live_from(&mut added).is_err());
    assert_eq!(added.performance_frame(), 98);
    assert_eq!(removed.performance_frame(), 0);
    added.apply(SessionPlaybackCommand::Stop).unwrap();
    removed.try_inherit_state_live_from(&mut added).unwrap();

    let mut restored = playback(Arc::clone(&wet));
    restored.try_inherit_state_live_from(&mut removed).unwrap();
    restored.apply(SessionPlaybackCommand::Play).unwrap();
    let mut clean = playback(wet);
    clean.apply(SessionPlaybackCommand::Play).unwrap();
    let mut a = [[0.0; 2]; 19];
    let mut b = a;
    restored.render(&mut a).unwrap();
    clean.render(&mut b).unwrap();
    assert_eq!(a, b);
}

#[test]
fn attaching_a_second_source_effect_keeps_the_first_sources_wet_history() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    let (mut editor, asset) = authored_fixture(0, false);
    let Created::Track(second) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "independent source".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state: hex_project::default_instrument_state().unwrap(),
        })
        .unwrap()
    else {
        panic!("second source track");
    };
    let project = editor.project();
    let first = project.tracks()[0].source_id().unwrap();
    let second = project.track(second).unwrap().source_id().unwrap();
    let instance = |source, receiver| {
        let mut effect = recipe(source, Some(0));
        effect.configuration = Some(Arc::new(
            hex_lattice_contract::NativeDeviceState::new(effect.device_type, 1, vec![1]).unwrap(),
        ));
        PreparedSourceEffects {
            source,
            receiver,
            instances: Arc::from([effect]),
            factory_bytes: 0,
        }
    };
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let prepare = |include_second| {
        let mut revision =
            PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
                .unwrap();
        let mut stages = vec![instance(first, 0)];
        if include_second {
            stages.push(instance(second, 2));
        }
        revision.test_install_source_effect_recipes(stages);
        SessionPlayback::new(Arc::new(revision)).unwrap()
    };
    let mut donor = prepare(false);
    let mut reference = prepare(false);
    for session in [&mut donor, &mut reference] {
        session.apply(SessionPlaybackCommand::Play).unwrap();
        session.render(&mut [[0.0; 2]; 97]).unwrap();
    }
    let mut candidate = prepare(true);
    candidate.try_inherit_state_live_from(&mut donor).unwrap();
    let mut a = [[0.0; 2]; 173];
    let mut b = a;
    candidate.render(&mut a).unwrap();
    reference.render(&mut b).unwrap();
    assert_eq!(a, b);
}

fn authored_modulated_suffix(
    state: hex_lattice_contract::NativeDeviceState,
    target: hex_project::DeviceAutomationTarget,
    amount: f32,
    enabled: bool,
) -> (Editor, hex_project::AssetId, hex_project::DeviceId) {
    use hex_devices::modulation::lfo::{Rate, Settings, Waveform};
    use hex_project::{
        DeviceModulation, DeviceModulationRoute, ElementColor, ModulationAmount,
        ModulationDestination, ModulationOutputId, ModulationRange, ModulationRouteId,
        ModulationSourceId, Modulator,
    };

    let (mut editor, asset) = authored_fixture(0, false);
    let receiver = editor.project().tracks()[0].id();
    let contributor = editor.project().tracks()[1].id();
    let source = editor.project().tracks()[0].source_id().unwrap();
    let instrument = editor
        .project()
        .native_source(source)
        .unwrap()
        .instrument()
        .id();
    for track in [receiver, contributor] {
        editor
            .execute(Command::BindTrackSource {
                track,
                source: None,
            })
            .unwrap();
    }
    let device_type = state.type_id();
    let Created::Device(device) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state,
        })
        .unwrap()
    else {
        panic!("source effect");
    };
    let next = editor.project().next_identity();
    let modulator = ModulationSourceId::new(next).unwrap();
    let bank = DeviceModulation::new(
        vec![Modulator {
            id: modulator,
            enabled: true,
            color: ElementColor::default(),
            state: Settings {
                waveform: Waveform::Square,
                phase: 0.0,
                rate: Rate::Hertz(12_000.0),
            }
            .encode()
            .unwrap(),
        }],
        vec![DeviceModulationRoute {
            id: ModulationRouteId::new(next + 1).unwrap(),
            source: modulator,
            output: ModulationOutputId::MAIN,
            destination: ModulationDestination::Device {
                device_type,
                target,
                vst3_class: None,
            },
            amount: ModulationAmount::new(amount).unwrap(),
            range: ModulationRange::default(),
            enabled,
        }],
    )
    .unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation {
            device,
            modulation: bank,
        })
        .unwrap();
    assert_eq!(
        metadata::construction_bytes(editor.project(), None),
        Some(0)
    );
    for track in [receiver, contributor] {
        editor
            .execute(Command::BindTrackSource {
                track,
                source: Some(source),
            })
            .unwrap();
    }
    (editor, asset, device)
}

#[test]
fn a_bound_native_suffix_modulates_only_generator_audio_and_preserves_phase_on_handoff() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::DeviceAutomationTarget;
    let (editor, asset, _) = authored_modulated_suffix(
        UTILITY_DEFINITION.default_state().unwrap(),
        DeviceAutomationTarget::Parameter(parameter::GAIN),
        0.5 / MAX_UTILITY_GAIN,
        true,
    );
    let model = editor.project();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(model, &media, Arc::new(crate::SineTable::new()), None).unwrap(),
    );
    let arrangement = revision.arrangement();
    let recipes = &arrangement.source_effects;
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].receiver, 0);
    assert!(recipes[0].instances[0].modulation.is_some());
    assert!(
        metadata::construction_bytes(model, None).unwrap()
            >= metadata::retained_bytes(recipes).unwrap()
    );

    let mut dry = [[0.0; 2]; WIDTH];
    crate::render_source_generators(arrangement, 0, 0, WIDTH as u64, &mut dry, None).unwrap();
    assert!(dry.iter().any(|frame| frame[0].abs() > 0.001));
    let compare = |frames: &[[f32; 2]; WIDTH]| {
        for (index, (actual, source)) in frames.iter().zip(dry).enumerate() {
            let gain = if index % 4 < 2 { 1.5 } else { 0.5 };
            let expected = [CLIP[0] + source[0] * gain, CLIP[1] + source[1] * gain];
            for (sample, expected) in actual.iter().zip(expected) {
                assert!(
                    (sample - expected).abs() < 1e-6,
                    "frame {index}: {sample} != {expected}"
                );
            }
        }
    };
    let mut offline = Renderer::new(arrangement);
    offline.play();
    let mut pcm = [[0.0; 2]; WIDTH];
    for block in pcm.chunks_mut(143) {
        offline.render(block).unwrap();
    }
    compare(&pcm);

    let mut donor = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    assert!(
        SessionPlayback::interactive_runtime_allowance(&revision).unwrap()
            >= donor.accounted_runtime_storage_bytes().unwrap()
    );
    donor.apply(SessionPlaybackCommand::Play).unwrap();
    let (before, after) = pcm.split_at_mut(97);
    donor.render(before).unwrap();
    let mut successor = SessionPlayback::new(revision).unwrap();
    successor.try_inherit_state_live_from(&mut donor).unwrap();
    assert_eq!(successor.performance_frame(), 97);
    for block in after.chunks_mut(37) {
        successor.render(block).unwrap();
    }
    compare(&pcm);
}

#[test]
fn live_source_route_edits_keep_phase_and_refuse_processing_class_changes() {
    check_live_source_route_edits(false);
}

#[test]
fn utility_matrix_modulation_switches_processing_class_without_applying_the_matrix_twice() {
    check_live_source_route_edits(true);
}

fn check_live_source_route_edits(matrix_route: bool) {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{
        DeviceAutomationTarget, DeviceModulation, ModulationAmount, ModulationDestination,
    };

    let target = if matrix_route {
        DeviceAutomationTarget::Parameter(parameter::GAIN)
    } else {
        DeviceAutomationTarget::HostOutputGain
    };
    let [first_depth, second_depth] = if matrix_route {
        [0.25 / MAX_UTILITY_GAIN, -0.35 / MAX_UTILITY_GAIN]
    } else {
        [0.035, -0.025]
    };
    let (mut editor, asset, device) = authored_modulated_suffix(
        UTILITY_DEFINITION.default_state().unwrap(),
        target,
        first_depth,
        false,
    );
    let state = UTILITY_DEFINITION
        .patch_parameter(
            editor.project().device(device).unwrap().state(),
            parameter::INVERT_LEFT,
            ParameterValue::Bool(true),
        )
        .unwrap();
    editor
        .execute(Command::ReplaceDeviceState { device, state })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let table = Arc::new(crate::SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    let base = prepare(&editor);
    let mut dry = [[0.0; 2]; WIDTH];
    crate::render_source_generators(base.arrangement(), 0, 0, WIDTH as u64, &mut dry, None)
        .unwrap();
    let mut previous = SessionPlayback::new(base).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; WIDTH];
    previous.render(&mut actual[..97]).unwrap();

    let bank = editor
        .project()
        .device(device)
        .unwrap()
        .modulation()
        .clone();
    let mut route = bank.routes()[0];
    route.enabled = true;
    let set = |editor: &mut Editor, routes| {
        editor
            .execute(Command::ReplaceDeviceModulation {
                device,
                modulation: DeviceModulation::new(bank.sources().to_vec(), routes).unwrap(),
            })
            .unwrap();
    };
    let ModulationDestination::Device { device_type, .. } = route.destination else {
        panic!("host route");
    };
    let incompatible = hex_project::DeviceModulationRoute {
        destination: ModulationDestination::Device {
            device_type,
            target: DeviceAutomationTarget::Parameter(parameter::DC_REMOVE),
            vst3_class: None,
        },
        ..route
    };
    set(&mut editor, vec![incompatible]);
    let mut refused = SessionPlayback::new(prepare(&editor)).unwrap();
    assert!(refused.try_inherit_state_live_from(&mut previous).is_err());
    assert_eq!(previous.performance_frame(), 97);
    assert_eq!(refused.performance_frame(), 0);

    for (start, end, depth) in [
        (97, 178, Some(first_depth)),
        (178, 231, Some(second_depth)),
        (231, WIDTH, None),
    ] {
        let routes = depth
            .map(|depth| hex_project::DeviceModulationRoute {
                amount: ModulationAmount::new(depth).unwrap(),
                ..route
            })
            .into_iter()
            .collect();
        set(&mut editor, routes);
        let mut successor = SessionPlayback::new(prepare(&editor)).unwrap();
        assert!(successor.try_inherit_state_from(&mut previous).is_err());
        successor
            .try_inherit_state_live_from(&mut previous)
            .unwrap();
        assert_eq!(successor.performance_frame(), start as u64);
        for frames in actual[start..end].chunks_mut(43) {
            successor.render(frames).unwrap();
        }
        previous = successor;
    }

    for (index, (actual, dry)) in actual.iter().zip(dry).enumerate() {
        let depth = match index {
            97..178 => first_depth,
            178..231 => second_depth,
            _ => 0.0,
        };
        let wave = if index % 4 < 2 { 1.0 } else { -1.0 };
        let gain = if matrix_route {
            1.0 + MAX_UTILITY_GAIN * depth * wave as f32
        } else {
            let normalized = (f64::from(60.0_f32 / 84.0) + f64::from(depth) * wave) as f32;
            let db = (84_000.0 * f64::from(normalized) - 60_000.0).round() as f32 / 1000.0;
            10.0_f32.powf(db / 20.0)
        };
        let expected = [CLIP[0] - dry[0] * gain, CLIP[1] + dry[1] * gain];
        for (sample, expected) in actual.iter().zip(expected) {
            assert!(
                (sample - expected).abs() < 1e-6,
                "frame {index}: {sample} != {expected}"
            );
        }
    }
}

#[test]
fn source_compressor_modulates_only_generator_audio_and_preserves_history_on_route_edit() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::compressor::{COMPRESSOR_DEFINITION, CompressorDefinition, parameter};
    use hex_lattice::{EMPTY_DEVICE_RESOURCES, NativeParameterAutomation, PreparedNativeProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterId, ParameterValue};
    use hex_project::{DeviceAutomationTarget, DeviceModulation, ModulationAmount};

    const HANDOFF: usize = 97;
    struct WetFrames;
    impl NativeParameterAutomation for WetFrames {
        fn parameter_ids(&self) -> &[ParameterId] {
            &[parameter::DRY_WET]
        }

        fn value_at(&self, id: ParameterId, frame: usize) -> Option<ParameterValue> {
            let depth = if frame < HANDOFF { 0.5 } else { 0.25 };
            let wet = if frame % 4 < 2 {
                0.5 + depth
            } else {
                0.5 - depth
            };
            (id == parameter::DRY_WET).then_some(ParameterValue::Float(wet))
        }
    }

    let mut state = COMPRESSOR_DEFINITION.default_state().unwrap();
    for (id, value) in [
        (parameter::THRESHOLD, -50.0),
        (parameter::RATIO, 20.0),
        (parameter::DRY_WET, 0.5),
    ] {
        state = COMPRESSOR_DEFINITION
            .patch_parameter(&state, id, ParameterValue::Float(value))
            .unwrap();
    }
    let (mut editor, asset, device) = authored_modulated_suffix(
        state.clone(),
        DeviceAutomationTarget::Parameter(parameter::DRY_WET),
        0.5,
        true,
    );
    for descriptor in COMPRESSOR_DEFINITION.descriptors() {
        assert!(crate::project_device_modulation_target_supported(
            editor.project(),
            device,
            descriptor.id,
        ));
    }

    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let table = Arc::new(crate::SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    let first = prepare(&editor);
    let bank = editor.project().device(device).unwrap().modulation();
    let mut route = bank.routes()[0];
    route.amount = ModulationAmount::new(0.25).unwrap();
    let modulation = DeviceModulation::new(bank.sources().to_vec(), vec![route]).unwrap();
    editor
        .execute(Command::ReplaceDeviceModulation { device, modulation })
        .unwrap();
    let next = prepare(&editor);

    let mut expected = [[0.0; 2]; WIDTH];
    crate::render_source_generators(first.arrangement(), 0, 0, WIDTH as u64, &mut expected, None)
        .unwrap();
    let dry = expected;
    let factory =
        CompressorDefinition::prepare_owned(&state, 48_000, &[parameter::DRY_WET]).unwrap();
    factory
        .create_runtime()
        .process_block_with_automation(&mut expected, None, &WetFrames, &EMPTY_DEVICE_RESOURCES)
        .unwrap();
    assert_ne!(expected, dry);
    for frame in &mut expected {
        frame[0] += CLIP[0];
        frame[1] += CLIP[1];
    }

    let mut donor = SessionPlayback::new(first).unwrap();
    let mut successor = SessionPlayback::new(next).unwrap();
    donor.apply(SessionPlaybackCommand::Play).unwrap();
    let mut actual = [[0.0; 2]; WIDTH];
    let (before, after) = actual.split_at_mut(HANDOFF);
    donor.render(before).unwrap();
    successor.try_inherit_state_live_from(&mut donor).unwrap();
    for block in after.chunks_mut(37) {
        successor.render(block).unwrap();
    }
    assert_eq!(actual, expected);
}

#[test]
fn bound_source_automation_reads_step_linear_and_loop_on_the_shared_signal() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{MAX_UTILITY_GAIN, UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::NativeDeviceMetadata;
    use hex_project::{
        AutomationInterpolation, AutomationPoint, AutomationValue, DeviceAutomationTarget,
    };

    let target = DeviceAutomationTarget::Parameter(parameter::GAIN);
    let (mut editor, asset, device) = authored_modulated_suffix(
        UTILITY_DEFINITION.default_state().unwrap(),
        target,
        0.0,
        false,
    );
    let source = editor.project().tracks()[0].source_id().unwrap();
    let receiver = editor.project().tracks()[0].id();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    assert!(!crate::project_device_automation_target_supported(
        editor.project(),
        device,
        target
    ));
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(receiver),
        })
        .unwrap();
    assert!(crate::project_device_automation_target_supported(
        editor.project(),
        device,
        target
    ));

    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let prepare = |project: &hex_project::Project| {
        Arc::new(
            PreparedRevision::prepare(project, &media, Arc::new(crate::SineTable::new()), None)
                .unwrap(),
        )
    };
    editor
        .execute(Command::SetLoopRegion {
            start: 24,
            end: 192,
            enabled: true,
        })
        .unwrap();
    let original = editor.project().clone();
    let dry = prepare(&original);
    editor
        .execute(Command::ReplaceDeviceAutomationTarget {
            device,
            target,
            points: [
                (0, 0.25, AutomationInterpolation::Step),
                (64, 0.5, AutomationInterpolation::Linear),
                (160, 1.0, AutomationInterpolation::Step),
            ]
            .into_iter()
            .map(|(frame, gain, outgoing)| AutomationPoint {
                beat: BeatTime::from_ratio(frame, 24_000).unwrap(),
                value: AutomationValue::new(gain / MAX_UTILITY_GAIN).unwrap(),
                outgoing,
            })
            .collect(),
        })
        .unwrap();
    let wet = prepare(editor.project());
    let recipes = &wet.arrangement().source_effects;
    assert_eq!(recipes.len(), 1);
    assert_eq!(recipes[0].receiver, 0);
    assert_eq!(recipes[0].instances[0].device_id, device.value());
    assert!(
        metadata::construction_bytes(editor.project(), None).unwrap()
            >= metadata::retained_bytes(recipes).unwrap()
    );
    let bound = editor.project().clone();
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    let dormant = prepare(editor.project());
    assert!(dormant.arrangement().source_effects.is_empty());
    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(receiver),
        })
        .unwrap();
    assert_eq!(
        editor.project(),
        &bound,
        "unbinding keeps dormant envelopes intact"
    );
    let gain = |frame: usize| {
        if frame < 64 {
            0.25
        } else if frame < 160 {
            0.5 + (frame - 64) as f32 / 192.0
        } else {
            1.0
        }
    };
    let compare = |path: &str,
                   reference: &[Frame],
                   actual: &[Frame],
                   local: &[Frame],
                   content: fn(usize) -> usize| {
        assert!(
            reference
                .iter()
                .zip(local)
                .any(|(frame, clip)| (frame[0] - clip[0]).abs() > 0.001)
        );
        for (index, ((plain, changed), clip)) in reference.iter().zip(actual).zip(local).enumerate()
        {
            for channel in 0..2 {
                let expected =
                    clip[channel] + (plain[channel] - clip[channel]) * gain(content(index));
                assert!(
                    (changed[channel] - expected).abs() < 1e-6,
                    "{path} frame {index}, origin {}, channel {channel}, dry {}, got {}, expected {expected}",
                    content(index),
                    plain[channel],
                    changed[channel]
                );
            }
        }
    };

    let mut before = [[0.0; 2]; WIDTH];
    let mut after = [[0.0; 2]; WIDTH];
    let mut offline_dry = Renderer::new(dry.arrangement());
    let mut offline_wet = Renderer::new(wet.arrangement());
    offline_dry.play();
    offline_wet.play();
    for block in before.chunks_mut(31) {
        offline_dry.render(block).unwrap();
    }
    for block in after.chunks_mut(53) {
        offline_wet.render(block).unwrap();
    }
    let loop_frame = |index| {
        if index < 192 {
            index
        } else {
            24 + (index - 192) % 168
        }
    };
    assert_eq!(offline_wet.position(), WIDTH as u64);
    compare("offline", &before, &after, &[CLIP; WIDTH], |index| index);

    let mut playing_dry = SessionPlayback::new(Arc::clone(&dry)).unwrap();
    let mut playing_wet = SessionPlayback::new(Arc::clone(&wet)).unwrap();
    assert!(
        SessionPlayback::interactive_runtime_allowance(&wet).unwrap()
            >= playing_wet.accounted_runtime_storage_bytes().unwrap()
    );
    playing_dry.apply(SessionPlaybackCommand::Play).unwrap();
    playing_wet.apply(SessionPlaybackCommand::Play).unwrap();
    for block in before.chunks_mut(47) {
        playing_dry.render(block).unwrap();
    }
    for block in after.chunks_mut(73) {
        playing_wet.render(block).unwrap();
    }
    assert_eq!(playing_wet.performance_frame(), WIDTH as u64);
    // Arrangement fades the local audio clip before the loop end. Measure
    // that path by itself so the oracle scales only the generator signal.
    let mut local = [[0.0; 2]; WIDTH];
    let mut playing_clip = SessionPlayback::new(dormant).unwrap();
    playing_clip.apply(SessionPlaybackCommand::Play).unwrap();
    for block in local.chunks_mut(59) {
        playing_clip.render(block).unwrap();
    }
    compare("playing", &before, &after, &local, loop_frame);

    // Installing the lane while the source is already playing keeps its
    // original note phase and applies the program only to future frames.
    let cut = 97;
    let mut donor = SessionPlayback::new(dry).unwrap();
    donor.apply(SessionPlaybackCommand::Play).unwrap();
    let mut live = [[0.0; 2]; WIDTH];
    donor.render(&mut live[..cut]).unwrap();
    let mut successor = SessionPlayback::new(wet).unwrap();
    successor.try_inherit_state_live_from(&mut donor).unwrap();
    assert_eq!(successor.performance_frame(), cut as u64);
    successor.render(&mut live[cut..]).unwrap();
    for (got, want) in live.iter().zip(before[..cut].iter().chain(&after[cut..])) {
        for channel in 0..2 {
            assert!((got[channel] - want[channel]).abs() < 1e-6);
        }
    }
}

#[test]
fn native_source_effect_recording_target_follows_the_exact_receiver() {
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::DeviceAutomationTarget;

    let kind = DeviceAutomationTarget::Parameter(parameter::MUTE);
    let (mut editor, _, device) = authored_modulated_suffix(
        UTILITY_DEFINITION.default_state().unwrap(),
        kind,
        0.0,
        false,
    );
    let source = editor.project().tracks()[0].source_id().unwrap();
    let other = editor.project().tracks()[1].id();
    let input = crate::native_parameter_input_target(editor.project(), device, kind).unwrap();
    let value = crate::NativeTransientValue::Parameter {
        id: parameter::MUTE,
        value: ParameterValue::Bool(true),
    };
    assert_eq!(input.validate_set(editor.project(), value), Ok(value));

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: Some(other),
        })
        .unwrap();
    let next = crate::native_parameter_input_target(editor.project(), device, kind).unwrap();
    assert_ne!(next, input);
    assert_eq!(next.identity(), input.identity());
    assert!(input.validate_set(editor.project(), value).is_err());
    assert_eq!(next.validate_set(editor.project(), value), Ok(value));

    editor
        .execute(Command::SetNativeSourceReceiver {
            source,
            track: None,
        })
        .unwrap();
    assert!(crate::native_parameter_input_target(editor.project(), device, kind).is_err());
}

#[test]
fn live_source_utility_gain_edit_preserves_shared_signal_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const RAMP: f32 = 240.0; // Existing 5 ms matrix law at 48 kHz.
    for partition in [1, 7, 64, 128, 257] {
        let (mut editor, asset) = authored_fixture(0, false);
        let instrument = editor.project().tracks()[0].instrument().unwrap().id();
        let Created::Device(effect) = editor
            .execute(Command::InsertNestedDevice {
                parent: instrument,
                state: UTILITY_DEFINITION.default_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("source effect")
        };
        let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
        let table = Arc::new(crate::SineTable::new());
        let prepare = |editor: &Editor| {
            Arc::new(
                PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                    .unwrap(),
            )
        };
        let original = prepare(&editor);
        let mut previous = SessionPlayback::new(Arc::clone(&original)).unwrap();
        let mut reference = SessionPlayback::new(original).unwrap();
        previous.apply(SessionPlaybackCommand::Play).unwrap();
        reference.apply(SessionPlaybackCommand::Play).unwrap();
        let mut prefix = [[0.0; 2]; PREFIX];
        previous.render(&mut prefix).unwrap();
        reference.render(&mut prefix).unwrap();
        let state = UTILITY_DEFINITION
            .patch_parameter(
                editor.project().device(effect).unwrap().state(),
                parameter::GAIN,
                ParameterValue::Float(2.0),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: effect,
                state,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
        candidate
            .try_inherit_state_live_from(&mut previous)
            .unwrap();
        let mut forward = [[0.0; 2]; SEGMENT];
        let mut baseline = [[0.0; 2]; SEGMENT];
        for chunk in forward.chunks_mut(partition) {
            candidate.render(chunk).unwrap();
        }
        for chunk in baseline.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        assert!(
            baseline
                .iter()
                .any(|frame| (frame[0] - CLIP[0]).abs() > 1e-5),
            "source oracle is silent"
        );
        for (index, (actual, old)) in forward.iter().zip(baseline).enumerate() {
            let gain = 1.0 + (index + 1) as f32 / RAMP;
            for channel in 0..2 {
                let expected = CLIP[channel] + gain * (old[channel] - CLIP[channel]);
                assert!(
                    (actual[channel] - expected).abs() < 1e-4,
                    "forward partition {partition} frame {index}: {} vs {expected}",
                    actual[channel]
                );
            }
        }
        assert!(editor.undo());
        let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
        restored
            .try_inherit_state_live_from(&mut candidate)
            .unwrap();
        let mut reverse = [[0.0; 2]; SEGMENT];
        let mut baseline = [[0.0; 2]; SEGMENT];
        for chunk in reverse.chunks_mut(partition) {
            restored.render(chunk).unwrap();
        }
        for chunk in baseline.chunks_mut(partition) {
            reference.render(chunk).unwrap();
        }
        let start_gain = 1.0 + SEGMENT as f32 / RAMP;
        for (index, (actual, old)) in reverse.iter().zip(baseline).enumerate() {
            let gain = start_gain + (1.0 - start_gain) * (index + 1) as f32 / RAMP;
            let local = if PREFIX + SEGMENT + index < WIDTH {
                CLIP
            } else {
                [0.0; 2]
            };
            for channel in 0..2 {
                let expected = local[channel] + gain * (old[channel] - local[channel]);
                assert!(
                    (actual[channel] - expected).abs() < 1e-4,
                    "reverse partition {partition} frame {index}: {} vs {expected}",
                    actual[channel]
                );
            }
        }
    }
}

#[test]
fn live_source_utility_filter_gain_preserves_filter_history_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const TOTAL: usize = PREFIX + 2 * SEGMENT;
    const RAMP: f32 = 240.0;
    for (dc, bass) in [(true, false), (false, true)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            let filter = if dc {
                parameter::DC_REMOVE
            } else {
                parameter::BASS_MONO
            };
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    filter,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut dry = [[0.0; 2]; TOTAL];
            crate::render_source_generators(
                original.arrangement(),
                0,
                0,
                TOTAL as u64,
                &mut dry,
                None,
            )
            .unwrap();
            assert!(
                dry.iter().flatten().any(|sample| sample.abs() > 1e-5),
                "source oracle is silent"
            );
            let mut oracle = UtilityFilterProcessor::new(
                PreparedUtilityFilters::new(48_000, dc, bass, 200.0, false).unwrap(),
            );
            let mut previous = SessionPlayback::new(Arc::clone(&original)).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; TOTAL];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::GAIN,
                    ParameterValue::Float(2.0),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let other_filter = if dc {
                parameter::BASS_MONO
            } else {
                parameter::DC_REMOVE
            };
            let incompatible = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    other_filter,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state: incompatible,
                })
                .unwrap();
            let mut refused = SessionPlayback::new(prepare(&editor)).unwrap();
            assert!(refused.try_inherit_state_live_from(&mut previous).is_err());
            assert_eq!(previous.performance_frame(), PREFIX as u64);
            assert_eq!(refused.performance_frame(), 0);
            assert!(editor.undo());
            let candidate_revision = prepare(&editor);
            let old_recipe = &original.arrangement().source_effects[0].instances;
            let new_recipe = &candidate_revision.arrangement().source_effects[0].instances;
            let old_chain = NativeRuntimeChain::new_from_source_recipe(old_recipe).unwrap();
            let mut wrong = new_recipe.to_vec();
            wrong[0].owner = ProcessorChainOwner::Source(u64::MAX);
            let wrong_source = NativeRuntimeChain::new_from_source_recipe(&wrong).unwrap();
            assert!(!wrong_source.can_transfer_source_recipe_history_from(&old_chain, true));
            wrong[0].owner = new_recipe[0].owner;
            wrong[0].device_id = u64::MAX;
            let wrong_device = NativeRuntimeChain::new_from_source_recipe(&wrong).unwrap();
            assert!(!wrong_device.can_transfer_source_recipe_history_from(&old_chain, true));
            let mut candidate = SessionPlayback::new(candidate_revision).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for chunk in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(chunk).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for chunk in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(chunk).unwrap();
            }
            for (i, (actual, input)) in actual.iter().zip(dry).enumerate() {
                let gain = if i < PREFIX {
                    1.0
                } else if i < PREFIX + SEGMENT {
                    1.0 + (i - PREFIX + 1) as f32 / RAMP
                } else {
                    let from = 1.0 + SEGMENT as f32 / RAMP;
                    from + (1.0 - from) * (i - PREFIX - SEGMENT + 1) as f32 / RAMP
                };
                let filtered = oracle.process_frame([input[0] * gain, input[1] * gain]);
                let local = if i < WIDTH { CLIP } else { [0.0; 2] };
                for channel in 0..2 {
                    let expected = filtered[channel] + local[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-4,
                        "dc={dc} bass={bass} partition={partition} frame={i} actual={} expected={expected}",
                        actual[channel]
                    );
                }
            }
        }
    }
}

#[test]
fn ordinary_preview_preserves_owned_source_delay_history() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};

    let (mut editor, asset) = authored_fixture(0, false);
    let instrument = editor.project().tracks()[0].instrument().unwrap().id();
    let definition = &hex_devices::delay::DEFINITION;
    let mut state = definition.default_state().unwrap();
    for id in [hex_devices::delay::LEFT_SYNC, hex_devices::delay::FILTER] {
        state = definition
            .patch_parameter(&state, id, ParameterValue::Bool(false))
            .unwrap();
    }
    state = definition
        .patch_parameter(
            &state,
            hex_devices::delay::TIME,
            ParameterValue::Float(10.0),
        )
        .unwrap();
    state = definition
        .patch_parameter(
            &state,
            hex_devices::delay::DRY_WET,
            ParameterValue::Float(1.0),
        )
        .unwrap();
    editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state,
        })
        .unwrap();
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let revision = Arc::new(
        PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(crate::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut previous = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    let mut oracle = SessionPlayback::new(Arc::clone(&revision)).unwrap();
    for runtime in [&mut previous, &mut oracle] {
        runtime.apply(SessionPlaybackCommand::Play).unwrap();
        runtime.render(&mut [[0.0; 2]; 700]).unwrap();
    }
    let mut candidate = SessionPlayback::new(revision).unwrap();
    candidate
        .preview_stream_handoff_with_policy_and_report(
            &mut previous,
            64,
            None,
            &mut |_| Ok(()),
            true,
            false,
        )
        .unwrap();
    let mut actual = [[0.0; 2]; 128];
    let mut expected = actual;
    previous.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-5));
    assert_eq!(
        actual, expected,
        "preview must preserve the donor's owned delay history"
    );
    candidate
        .try_inherit_state_live_from(&mut previous)
        .unwrap();
    candidate.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "commit must transfer the live donor history"
    );
}

#[test]
fn live_source_utility_balance_preserves_stereo_filter_history_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const TOTAL: usize = PREFIX + 2 * SEGMENT;
    const RAMP: f32 = 240.0;
    for (dc, bass) in [(false, false), (true, false), (false, true)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            let mut state = UTILITY_DEFINITION.default_state().unwrap();
            for (id, enabled) in [(parameter::DC_REMOVE, dc), (parameter::BASS_MONO, bass)] {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, id, ParameterValue::Bool(enabled))
                    .unwrap();
            }
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut dry = [[0.0; 2]; TOTAL];
            crate::render_source_generators(
                original.arrangement(),
                0,
                0,
                TOTAL as u64,
                &mut dry,
                None,
            )
            .unwrap();
            assert!(dry.iter().flatten().any(|sample| sample.abs() > 1e-5));
            let mut oracle = UtilityFilterProcessor::new(
                PreparedUtilityFilters::new(48_000, dc, bass, 200.0, false).unwrap(),
            );
            let mut previous = SessionPlayback::new(original).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; TOTAL];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::BALANCE,
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for chunk in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(chunk).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for chunk in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(chunk).unwrap();
            }
            for (i, (actual, input)) in actual.iter().zip(dry).enumerate() {
                let left = if i < PREFIX {
                    1.0
                } else if i < PREFIX + SEGMENT {
                    1.0 - (i - PREFIX + 1) as f32 / RAMP
                } else {
                    let from = 1.0 - SEGMENT as f32 / RAMP;
                    from + (1.0 - from) * (i - PREFIX - SEGMENT + 1) as f32 / RAMP
                };
                let filtered = oracle.process_frame([input[0] * left, input[1]]);
                let local = if i < WIDTH { CLIP } else { [0.0; 2] };
                for channel in 0..2 {
                    let expected = filtered[channel] + local[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-4,
                        "dc={dc} bass={bass} partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                        actual[channel]
                    );
                }
            }
        }
    }
}

#[test]
fn live_source_utility_width_preserves_asymmetric_filter_history_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const TOTAL: usize = PREFIX + 2 * SEGMENT;
    const RAMP: f32 = 240.0;
    for (dc, bass) in [(false, false), (true, false), (false, true)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            // A preceding source Utility isolates the right channel so width
            // has an observable asymmetric input independent of local audio.
            let asymmetric = UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    parameter::BALANCE,
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            let Created::Device(first) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state: asymmetric,
                })
                .unwrap()
            else {
                panic!("first effect")
            };
            let mut state = UTILITY_DEFINITION.default_state().unwrap();
            for (id, enabled) in [(parameter::DC_REMOVE, dc), (parameter::BASS_MONO, bass)] {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, id, ParameterValue::Bool(enabled))
                    .unwrap();
            }
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let source = editor
                .project()
                .track(editor.project().tracks()[0].id())
                .unwrap()
                .source_id()
                .unwrap();
            let effects = editor
                .project()
                .native_source(source)
                .unwrap()
                .post_instrument_effects();
            assert_eq!(
                effects.iter().map(|device| device.id()).collect::<Vec<_>>(),
                [first, effect]
            );
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut dry = [[0.0; 2]; TOTAL];
            crate::render_source_generators(
                original.arrangement(),
                0,
                0,
                TOTAL as u64,
                &mut dry,
                None,
            )
            .unwrap();
            assert!(dry.iter().flatten().any(|sample| sample.abs() > 1e-5));
            let mut oracle = UtilityFilterProcessor::new(
                PreparedUtilityFilters::new(48_000, dc, bass, 200.0, false).unwrap(),
            );
            let mut previous = SessionPlayback::new(original).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; TOTAL];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::WIDTH,
                    ParameterValue::Float(0.0),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for chunk in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(chunk).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for chunk in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(chunk).unwrap();
            }
            for (i, (actual, input)) in actual.iter().zip(dry).enumerate() {
                let left = if i < PREFIX {
                    1.0
                } else if i < PREFIX + SEGMENT {
                    1.0 - (i - PREFIX + 1) as f32 / RAMP
                } else {
                    let from = 1.0 - SEGMENT as f32 / RAMP;
                    from + (1.0 - from) * (i - PREFIX - SEGMENT + 1) as f32 / RAMP
                };
                let mixed = (1.0 - left) * 0.5;
                let filtered = oracle.process_frame([input[1] * mixed, input[1] * (1.0 - mixed)]);
                let local = if i < WIDTH { CLIP } else { [0.0; 2] };
                for channel in 0..2 {
                    let expected = filtered[channel] + local[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-4,
                        "dc={dc} bass={bass} partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                        actual[channel]
                    );
                }
            }
        }
    }
}

#[test]
fn centered_source_utility_mid_side_mode_keeps_live_pcm_and_history() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    for filter in [None, Some(parameter::DC_REMOVE), Some(parameter::BASS_MONO)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            let mut state = UTILITY_DEFINITION.default_state().unwrap();
            if let Some(filter) = filter {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, filter, ParameterValue::Bool(true))
                    .unwrap();
            }
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut previous = SessionPlayback::new(Arc::clone(&original)).unwrap();
            let mut oracle = SessionPlayback::new(original).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            oracle.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; PREFIX + 2 * SEGMENT];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::MID_SIDE_ENABLED,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for part in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(part).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for part in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(part).unwrap();
            }
            let mut expected = [[0.0; 2]; PREFIX + 2 * SEGMENT];
            for part in expected.chunks_mut(partition) {
                oracle.render(part).unwrap();
            }
            assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-5));
            assert_eq!(actual, expected, "filter={filter:?} partition={partition}");
        }
    }
}

#[test]
fn source_utility_equivalent_silent_matrix_refuses_filter_or_mute_changes() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (mut editor, asset) = authored_fixture(0, false);
    let instrument = editor.project().tracks()[0].instrument().unwrap().id();
    let state = UTILITY_DEFINITION
        .patch_parameter(
            &UTILITY_DEFINITION.default_state().unwrap(),
            parameter::DC_REMOVE,
            ParameterValue::Bool(true),
        )
        .unwrap();
    let state = UTILITY_DEFINITION
        .patch_parameter(&state, parameter::GAIN, ParameterValue::Float(0.0))
        .unwrap();
    let Created::Device(effect) = editor
        .execute(Command::InsertNestedDevice {
            parent: instrument,
            state,
        })
        .unwrap()
    else {
        panic!("effect")
    };
    let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
    let table = Arc::new(crate::SineTable::new());
    let prepare = |editor: &Editor| {
        Arc::new(
            PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None).unwrap(),
        )
    };
    let mut previous = SessionPlayback::new(prepare(&editor)).unwrap();
    previous.apply(SessionPlaybackCommand::Play).unwrap();
    previous.render(&mut [[0.0; 2]; 64]).unwrap();
    for parameter in [parameter::BASS_MONO, parameter::MUTE] {
        let state = UTILITY_DEFINITION
            .patch_parameter(
                editor.project().device(effect).unwrap().state(),
                parameter,
                ParameterValue::Bool(true),
            )
            .unwrap();
        editor
            .execute(Command::ReplaceDeviceState {
                device: effect,
                state,
            })
            .unwrap();
        let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
        assert!(
            candidate
                .try_inherit_state_live_from(&mut previous)
                .is_err(),
            "parameter={parameter:?}"
        );
        assert_eq!(previous.performance_frame(), 64);
        assert_eq!(candidate.performance_frame(), 0);
        assert!(editor.undo());
    }
}

#[test]
fn live_source_utility_mid_side_preserves_asymmetric_filter_history_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const TOTAL: usize = PREFIX + 2 * SEGMENT;
    const RAMP: f32 = 240.0;
    for (dc, bass) in [(false, false), (true, false), (false, true)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            // A preceding source Utility isolates the right channel so width
            // has an observable asymmetric input independent of local audio.
            let asymmetric = UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    parameter::BALANCE,
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            let Created::Device(first) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state: asymmetric,
                })
                .unwrap()
            else {
                panic!("first effect")
            };
            let mut state = UTILITY_DEFINITION.default_state().unwrap();
            for (id, enabled) in [(parameter::DC_REMOVE, dc), (parameter::BASS_MONO, bass)] {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, id, ParameterValue::Bool(enabled))
                    .unwrap();
            }
            state = UTILITY_DEFINITION
                .patch_parameter(
                    &state,
                    parameter::MID_SIDE_ENABLED,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let source = editor
                .project()
                .track(editor.project().tracks()[0].id())
                .unwrap()
                .source_id()
                .unwrap();
            let effects = editor
                .project()
                .native_source(source)
                .unwrap()
                .post_instrument_effects();
            assert_eq!(
                effects.iter().map(|device| device.id()).collect::<Vec<_>>(),
                [first, effect]
            );
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut dry = [[0.0; 2]; TOTAL];
            crate::render_source_generators(
                original.arrangement(),
                0,
                0,
                TOTAL as u64,
                &mut dry,
                None,
            )
            .unwrap();
            assert!(dry.iter().flatten().any(|sample| sample.abs() > 1e-5));
            let mut oracle = UtilityFilterProcessor::new(
                PreparedUtilityFilters::new(48_000, dc, bass, 200.0, false).unwrap(),
            );
            let mut previous = SessionPlayback::new(original).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; TOTAL];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::MID_SIDE_BALANCE,
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for chunk in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(chunk).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for chunk in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(chunk).unwrap();
            }
            for (i, (actual, input)) in actual.iter().zip(dry).enumerate() {
                let left = if i < PREFIX {
                    1.0
                } else if i < PREFIX + SEGMENT {
                    1.0 - (i - PREFIX + 1) as f32 / RAMP
                } else {
                    let from = 1.0 - SEGMENT as f32 / RAMP;
                    from + (1.0 - from) * (i - PREFIX - SEGMENT + 1) as f32 / RAMP
                };
                let mixed = (1.0 - left) * 0.5;
                let filtered = oracle.process_frame([-input[1] * mixed, input[1] * (1.0 - mixed)]);
                let local = if i < WIDTH { CLIP } else { [0.0; 2] };
                for channel in 0..2 {
                    let expected = filtered[channel] + local[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-4,
                        "dc={dc} bass={bass} partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                        actual[channel]
                    );
                }
            }
        }
    }
}

#[test]
fn live_source_utility_mode_switch_preserves_asymmetric_filter_history_and_local_audio() {
    use crate::{PreparedRevision, SessionPlayback, SessionPlaybackCommand};
    use hex_devices::utility::{UTILITY_DEFINITION, parameter};
    use hex_devices::utility_filters::{PreparedUtilityFilters, UtilityFilterProcessor};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    const PREFIX: usize = 64;
    const SEGMENT: usize = 128;
    const TOTAL: usize = PREFIX + 2 * SEGMENT;
    const RAMP: f32 = 240.0;
    for (dc, bass) in [(false, false), (true, false), (false, true)] {
        for partition in [1, 7, 64, 128, 257] {
            let (mut editor, asset) = authored_fixture(0, false);
            let instrument = editor.project().tracks()[0].instrument().unwrap().id();
            // A preceding source Utility isolates the right channel so width
            // has an observable asymmetric input independent of local audio.
            let asymmetric = UTILITY_DEFINITION
                .patch_parameter(
                    &UTILITY_DEFINITION.default_state().unwrap(),
                    parameter::BALANCE,
                    ParameterValue::Float(1.0),
                )
                .unwrap();
            let Created::Device(first) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state: asymmetric,
                })
                .unwrap()
            else {
                panic!("first effect")
            };
            let mut state = UTILITY_DEFINITION.default_state().unwrap();
            for (id, enabled) in [(parameter::DC_REMOVE, dc), (parameter::BASS_MONO, bass)] {
                state = UTILITY_DEFINITION
                    .patch_parameter(&state, id, ParameterValue::Bool(enabled))
                    .unwrap();
            }
            state = UTILITY_DEFINITION
                .patch_parameter(&state, parameter::WIDTH, ParameterValue::Float(0.0))
                .unwrap();
            let Created::Device(effect) = editor
                .execute(Command::InsertNestedDevice {
                    parent: instrument,
                    state,
                })
                .unwrap()
            else {
                panic!("source effect")
            };
            let source = editor
                .project()
                .track(editor.project().tracks()[0].id())
                .unwrap()
                .source_id()
                .unwrap();
            let effects = editor
                .project()
                .native_source(source)
                .unwrap()
                .post_instrument_effects();
            assert_eq!(
                effects.iter().map(|device| device.id()).collect::<Vec<_>>(),
                [first, effect]
            );
            let media = BTreeMap::from([(asset, Audio::new(48_000, vec![CLIP; WIDTH]).unwrap())]);
            let table = Arc::new(crate::SineTable::new());
            let prepare = |editor: &Editor| {
                Arc::new(
                    PreparedRevision::prepare(editor.project(), &media, Arc::clone(&table), None)
                        .unwrap(),
                )
            };
            let original = prepare(&editor);
            let mut dry = [[0.0; 2]; TOTAL];
            crate::render_source_generators(
                original.arrangement(),
                0,
                0,
                TOTAL as u64,
                &mut dry,
                None,
            )
            .unwrap();
            assert!(dry.iter().flatten().any(|sample| sample.abs() > 1e-5));
            let mut oracle = UtilityFilterProcessor::new(
                PreparedUtilityFilters::new(48_000, dc, bass, 200.0, false).unwrap(),
            );
            let mut previous = SessionPlayback::new(original).unwrap();
            previous.apply(SessionPlaybackCommand::Play).unwrap();
            let mut actual = [[0.0; 2]; TOTAL];
            previous.render(&mut actual[..PREFIX]).unwrap();
            let state = UTILITY_DEFINITION
                .patch_parameter(
                    editor.project().device(effect).unwrap().state(),
                    parameter::MID_SIDE_ENABLED,
                    ParameterValue::Bool(true),
                )
                .unwrap();
            editor
                .execute(Command::ReplaceDeviceState {
                    device: effect,
                    state,
                })
                .unwrap();
            let mut candidate = SessionPlayback::new(prepare(&editor)).unwrap();
            candidate
                .try_inherit_state_live_from(&mut previous)
                .unwrap();
            for chunk in actual[PREFIX..PREFIX + SEGMENT].chunks_mut(partition) {
                candidate.render(chunk).unwrap();
            }
            assert!(editor.undo());
            let mut restored = SessionPlayback::new(prepare(&editor)).unwrap();
            restored
                .try_inherit_state_live_from(&mut candidate)
                .unwrap();
            for chunk in actual[PREFIX + SEGMENT..].chunks_mut(partition) {
                restored.render(chunk).unwrap();
            }
            for (i, (actual, input)) in actual.iter().zip(dry).enumerate() {
                let left = if i < PREFIX {
                    1.0
                } else if i < PREFIX + SEGMENT {
                    1.0 - (i - PREFIX + 1) as f32 / RAMP
                } else {
                    let from = 1.0 - SEGMENT as f32 / RAMP;
                    from + (1.0 - from) * (i - PREFIX - SEGMENT + 1) as f32 / RAMP
                };
                let mixed = left * 0.5;
                let filtered = oracle.process_frame([input[1] * mixed, input[1] * (1.0 - mixed)]);
                let local = if i < WIDTH { CLIP } else { [0.0; 2] };
                for channel in 0..2 {
                    let expected = filtered[channel] + local[channel];
                    assert!(
                        (actual[channel] - expected).abs() < 1e-4,
                        "dc={dc} bass={bass} partition={partition} frame={i} channel={channel} actual={} expected={expected}",
                        actual[channel]
                    );
                }
            }
        }
    }
}
