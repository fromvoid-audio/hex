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

    // Render the bound singleton separately. Author the same source's suffix
    // while detached; production cannot bind a source with effects yet.
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
        crate::compile::prepare_source_effect_recipes(compiled, editor.project(), &native, &context)
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
    assert_eq!(metadata::construction_bytes(editor.project()), Some(0));
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
