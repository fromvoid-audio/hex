use super::*;

use std::sync::{Barrier, Weak};

fn spectrum_test_target(
    owner: hex_engine::ProcessorChainOwner,
    project_generation: u64,
    activation_floor: u64,
    sample_rate_hz: u32,
) -> spectrum_target::SpectrumTarget {
    spectrum_target::SpectrumTarget {
        token: crate::spectrum_worker::SpectrumTargetId(1),
        identity: PostNodeIdentity::new(
            owner,
            11,
            hex_engine::DeviceTypeId::from_static("hex.eq8"),
        ),
        project_generation,
        activation_floor,
        sample_rate_hz,
    }
}

fn spectrum_eq8_graph() -> (Playback, PostNodeIdentity) {
    spectrum_eq8_graph_with_playing(true)
}

fn fresh_spectrum_eq8_graph() -> (Playback, PostNodeIdentity) {
    spectrum_eq8_graph_with_playing(false)
}

fn spectrum_eq8_graph_with_playing(playing: bool) -> (Playback, PostNodeIdentity) {
    spectrum_eq8_graph_with_mode(playing, 0)
}

fn spectrum_eq8_graph_with_mode(playing: bool, mode: u32) -> (Playback, PostNodeIdentity) {
    spectrum_eq8_graph_with_params(playing, mode, 0.0, 1.0)
}

pub(super) fn spectrum_eq8_graph_with_params(
    playing: bool,
    mode: u32,
    gain_db: f32,
    q: f32,
) -> (Playback, PostNodeIdentity) {
    spectrum_eq_graph_with_params(playing, mode, gain_db, q, false, false)
}

pub(super) fn spectrum_bus_graph() -> (Playback, [PostNodeIdentity; 3]) {
    let (playback, track) = spectrum_eq_graph_with_params(true, 0, 12.0, 1.0, false, true);
    (
        playback,
        [
            track,
            PostNodeIdentity::new(
                hex_engine::ProcessorChainOwner::Return(3),
                12,
                track.device_type,
            ),
            PostNodeIdentity::new(hex_engine::ProcessorChainOwner::Main, 13, track.device_type),
        ],
    )
}

fn spectrum_eq_graph_with_params(
    playing: bool,
    mode: u32,
    gain_db: f32,
    q: f32,
    flexible: bool,
    effect_buses: bool,
) -> (Playback, PostNodeIdentity) {
    use hex_lattice_contract::DeviceTypeId;
    use hex_project::{
        AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
        Project as CanonicalProject, ReturnTrackSnapshot, SessionSnapshot, Snapshot, Tempo,
        TempoPoint, TrackContentSnapshot, TrackSnapshot,
    };

    const DEVICE_ID: u64 = 11;
    const TRACK_ID: u64 = 1;
    const ASSET_ID: u64 = 4;
    const CLIP_ID: u64 = 5;
    const FRAMES: u64 = 512;
    let mut state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    // The audition contract intentionally refuses disabled bands. Enable
    // band zero in both stereo banks so this prepared callback fixture
    // exercises the admitted path rather than only waiting/rejection.
    for parameter in [1, 41] {
        state = hex_engine::patch_native_device_parameter(
            &state,
            hex_lattice_contract::ParameterId::from_static(parameter),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
    }
    state = hex_engine::patch_native_device_parameter(
        &state,
        hex_lattice_contract::ParameterId::from_static(201),
        hex_lattice_contract::ParameterValue::Choice(mode),
    )
    .unwrap();
    state = hex_engine::patch_native_device_parameter(
        &state,
        hex_lattice_contract::ParameterId::from_static(4),
        hex_lattice_contract::ParameterValue::Float(gain_db),
    )
    .unwrap();
    state = hex_engine::patch_native_device_parameter(
        &state,
        hex_lattice_contract::ParameterId::from_static(5),
        hex_lattice_contract::ParameterValue::Float(q),
    )
    .unwrap();
    let mut return_state = hex_engine::default_native_device_state_for_class(
        hex_engine::NativeDeviceClass::Effect,
        DeviceTypeId::from_static("hex.eq8"),
    )
    .unwrap();
    for parameter in [1, 41] {
        return_state = hex_engine::patch_native_device_parameter(
            &return_state,
            hex_lattice_contract::ParameterId::from_static(parameter),
            hex_lattice_contract::ParameterValue::Bool(true),
        )
        .unwrap();
    }
    return_state = hex_engine::patch_native_device_parameter(
        &return_state,
        hex_lattice_contract::ParameterId::from_static(201),
        hex_lattice_contract::ParameterValue::Choice(mode),
    )
    .unwrap();
    if flexible {
        use hex_engine::eq::{
            AutomationLane, AutomationPoint, EqState, Node, NodeId, NodeParameter,
        };
        let mut settings = EqState::default();
        for index in 0..64 {
            let id = NodeId::new(1000 + index).unwrap();
            let mut node = Node::new(id, 500.0, if index == 0 { gain_db } else { 0.0 }, 0).unwrap();
            node.q = q;
            settings.add(node).unwrap();
            settings
                .set_lane(AutomationLane {
                    node: id,
                    parameter: NodeParameter::Frequency,
                    points: vec![
                        AutomationPoint {
                            beat: 0,
                            normalized: 0.2,
                        },
                        AutomationPoint {
                            beat: 4 << 32,
                            normalized: 0.8,
                        },
                    ],
                })
                .unwrap();
        }
        state = settings.encode().unwrap();
        return_state = state.clone();
    }
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: if effect_buses {
            vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: DEVICE_ID + 2,
                bypassed: false,
                state: state.clone(),
            }]
        } else {
            Vec::new()
        },
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: DEVICE_ID + if effect_buses { 3 } else { 2 },
        assets: vec![AssetSnapshot {
            original_source: None,
            id: ASSET_ID,
            path: "analyzer-test.wav".into(),
            source_sample_rate: 48_000,
            source_frames: FRAMES,
            playback_frames: FRAMES,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: TRACK_ID,
            name: "analyzer fixture".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: DEVICE_ID,
                bypassed: false,
                state,
            }],
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: CLIP_ID,
                asset: ASSET_ID,
                start: 0,
                duration: FRAMES,
                source_offset: 0,
                source_length: FRAMES,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(FRAMES).unwrap(),
            }]),
            sends: if effect_buses {
                vec![hex_project::SendLevelSnapshot {
                    return_track: 3,
                    gain: 0.5,
                }]
            } else {
                Vec::new()
            },
        }],
        return_tracks: vec![ReturnTrackSnapshot {
            id: 3,
            name: "return fixture".into(),
            gain: 1.0,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: DEVICE_ID + 1,
                bypassed: false,
                state: return_state,
            }],
        }],
        session: SessionSnapshot::default(),
        loop_region: if effect_buses {
            Some(hex_project::LoopRegion::new(0, FRAMES, true).unwrap())
        } else {
            None
        },
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
    })
    .unwrap();
    let asset = project.assets().next().unwrap().id();
    let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize]).unwrap();
    let media = [(asset, audio)].into_iter().collect();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
    if playing {
        playback.apply(PlaybackCommand::Play);
    }
    (
        playback,
        PostNodeIdentity::new(
            hex_engine::ProcessorChainOwner::Track(TRACK_ID),
            DEVICE_ID,
            DeviceTypeId::from_static(if flexible { "hex.eq" } else { "hex.eq8" }),
        ),
    )
}

#[cfg(feature = "allocation-probe")]
fn saturator_track_and_return_graph(drive_db: f32) -> Playback {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let make_state = || {
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.saturator"),
        )
        .expect("registered saturator state");
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(1),
            ParameterValue::Float(drive_db),
        )
        .expect("saturator drive");
        hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(4),
            ParameterValue::Choice(1),
        )
        .expect("saturator curve")
    };
    native_effect_track_and_return_graph(make_state())
}

#[cfg(feature = "allocation-probe")]
fn native_effect_track_and_return_graph(
    state: hex_lattice_contract::NativeDeviceState,
) -> Playback {
    use hex_project::{
        AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
        Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
        Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
    };

    const TRACK_ID: u64 = 1;
    const ASSET_ID: u64 = 4;
    const CLIP_ID: u64 = 5;
    const TRACK_DEVICE_ID: u64 = 6;
    const RETURN_ID: u64 = 7;
    const RETURN_DEVICE_ID: u64 = 8;
    const FRAMES: u64 = 512;
    let make_state = || state.clone();
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: RETURN_DEVICE_ID + 1,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: ASSET_ID,
            path: "saturator-callback-test.wav".into(),
            source_sample_rate: 48_000,
            source_frames: FRAMES,
            playback_frames: FRAMES,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: TRACK_ID,
            name: "saturator callback fixture".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: TRACK_DEVICE_ID,
                bypassed: false,
                state: make_state(),
            }],
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: CLIP_ID,
                asset: ASSET_ID,
                start: 0,
                duration: FRAMES,
                source_offset: 0,
                source_length: FRAMES,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
            }]),
            sends: vec![SendLevelSnapshot {
                return_track: RETURN_ID,
                gain: 0.5,
            }],
        }],
        return_tracks: vec![ReturnTrackSnapshot {
            id: RETURN_ID,
            name: "saturator return fixture".into(),
            gain: 0.75,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: RETURN_DEVICE_ID,
                bypassed: false,
                state: make_state(),
            }],
        }],
        session: SessionSnapshot::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
    })
    .expect("valid track and Return graph");
    let audio =
        hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize]).expect("audio fixture");
    let media = [(project.assets().next().expect("asset").id(), audio)]
        .into_iter()
        .collect();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .expect("prepared track and Return chain"),
    );
    Playback::try_from_prepared_with_session(revision).expect("playback")
}

#[cfg(feature = "allocation-probe")]
fn compressor_track_and_return_graph(threshold_db: f32) -> Playback {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    use hex_project::{
        AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
        Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
        Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
    };

    const TRACK_ID: u64 = 1;
    const ASSET_ID: u64 = 4;
    const CLIP_ID: u64 = 5;
    const TRACK_DEVICE_ID: u64 = 6;
    const RETURN_ID: u64 = 7;
    const RETURN_DEVICE_ID: u64 = 8;
    const FRAMES: u64 = 512;
    let make_state = || {
        let state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.compressor"),
        )
        .expect("registered compressor state");
        hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(1),
            ParameterValue::Float(threshold_db),
        )
        .expect("compressor threshold")
    };
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: RETURN_DEVICE_ID + 1,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: ASSET_ID,
            path: "compressor-callback-test.wav".into(),
            source_sample_rate: 48_000,
            source_frames: FRAMES,
            playback_frames: FRAMES,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: TRACK_ID,
            name: "compressor callback fixture".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: TRACK_DEVICE_ID,
                bypassed: false,
                state: make_state(),
            }],
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: CLIP_ID,
                asset: ASSET_ID,
                start: 0,
                duration: FRAMES,
                source_offset: 0,
                source_length: FRAMES,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
            }]),
            sends: vec![SendLevelSnapshot {
                return_track: RETURN_ID,
                gain: 0.5,
            }],
        }],
        return_tracks: vec![ReturnTrackSnapshot {
            id: RETURN_ID,
            name: "compressor return fixture".into(),
            gain: 0.75,
            muted: false,
            devices: vec![DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: RETURN_DEVICE_ID,
                bypassed: false,
                state: make_state(),
            }],
        }],
        session: SessionSnapshot::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
    })
    .expect("valid compressor track and Return graph");
    let audio =
        hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize]).expect("audio fixture");
    let media = [(project.assets().next().expect("asset").id(), audio)]
        .into_iter()
        .collect();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .expect("prepared compressor track and Return chain"),
    );
    Playback::try_from_prepared_with_session(revision).expect("playback")
}

#[cfg(feature = "allocation-probe")]
fn gate_track_and_return_graph(floor_db: f32) -> Playback {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let make_state = || {
        let mut state = hex_engine::default_native_device_state_for_class(
            hex_engine::NativeDeviceClass::Effect,
            DeviceTypeId::from_static("hex.gate"),
        )
        .expect("registered gate state");
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(1),
            ParameterValue::Float(0.0),
        )
        .expect("gate threshold");
        state = hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(2),
            ParameterValue::Float(-6.0),
        )
        .expect("gate return");
        hex_engine::patch_native_device_parameter(
            &state,
            ParameterId::from_static(3),
            ParameterValue::Float(floor_db),
        )
        .expect("gate floor")
    };
    native_track_and_return_graph(Some(make_state()))
}

#[cfg(feature = "allocation-probe")]
fn native_track_and_return_graph(
    state: Option<hex_lattice_contract::NativeDeviceState>,
) -> Playback {
    use hex_project::{
        AssetSnapshot, ClipFade, ClipSnapshot, DeviceSnapshot, MeterPoint,
        Project as CanonicalProject, ReturnTrackSnapshot, SendLevelSnapshot, SessionSnapshot,
        Snapshot, Tempo, TempoPoint, TrackContentSnapshot, TrackSnapshot,
    };

    const TRACK_ID: u64 = 1;
    const ASSET_ID: u64 = 4;
    const CLIP_ID: u64 = 5;
    const TRACK_DEVICE_ID: u64 = 6;
    const RETURN_ID: u64 = 7;
    const RETURN_DEVICE_ID: u64 = 8;
    const FRAMES: u64 = 512;
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: RETURN_DEVICE_ID + 1,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: ASSET_ID,
            path: "gate-callback-test.wav".into(),
            source_sample_rate: 48_000,
            source_frames: FRAMES,
            playback_frames: FRAMES,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: TRACK_ID,
            name: "gate callback fixture".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: state
                .as_ref()
                .map(|state| DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: TRACK_DEVICE_ID,
                    bypassed: false,
                    state: state.clone(),
                })
                .into_iter()
                .collect(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(vec![ClipSnapshot {
                active: true,
                name: "clip".into(),
                flavor: hex_project::ClipFlavor::default(),
                edit: None,
                gain: hex_project::ClipGain::UNITY,
                id: CLIP_ID,
                asset: ASSET_ID,
                start: 0,
                duration: FRAMES,
                source_offset: 0,
                source_length: FRAMES,
                looping: true,
                source_phase: 0,
                fade: ClipFade::neutral(FRAMES).expect("neutral fade"),
            }]),
            sends: vec![SendLevelSnapshot {
                return_track: RETURN_ID,
                gain: 0.5,
            }],
        }],
        return_tracks: vec![ReturnTrackSnapshot {
            id: RETURN_ID,
            name: "gate return fixture".into(),
            gain: 0.75,
            muted: false,
            devices: state
                .as_ref()
                .map(|state| DeviceSnapshot {
                    modulation: Default::default(),
                    parent: None,
                    automation: Vec::new(),
                    comparison: None,
                    id: RETURN_DEVICE_ID,
                    bypassed: false,
                    state: state.clone(),
                })
                .into_iter()
                .collect(),
        }],
        session: SessionSnapshot::default(),
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
    })
    .expect("valid track and Return Gate graph");
    let audio =
        hex_engine::Audio::new(48_000, vec![[0.25, -0.4]; FRAMES as usize]).expect("audio fixture");
    let media = [(project.assets().next().expect("asset").id(), audio)]
        .into_iter()
        .collect();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .expect("prepared Gate track and Return chain"),
    );
    Playback::try_from_prepared_with_session(revision).expect("playback")
}

#[cfg(feature = "allocation-probe")]
fn compressor_routed_reference(
    track_threshold_db: f64,
    return_threshold_db: f64,
    track_history: &mut CompressorReferenceHistory,
    return_history: &mut CompressorReferenceHistory,
    input: &[[f32; 2]],
) -> Vec<[f32; 2]> {
    let process = |frame: [f32; 2], threshold_db: f64, history: &mut CompressorReferenceHistory| {
        let peak = f64::from(frame[0]).abs().max(f64::from(frame[1]).abs());
        let alpha = (-1.0_f64 / (0.010 * 48_000.0)).exp();
        history.envelope = alpha * history.envelope + (1.0 - alpha) * peak;
        let level_db = if history.envelope == 0.0 {
            -120.0
        } else {
            20.0 * history.envelope.log10()
        };
        let over = level_db - threshold_db;
        let slope = 0.25 - 1.0;
        let reduction_db = if over <= -3.0 {
            0.0
        } else if over >= 3.0 {
            slope * over
        } else {
            slope * (over + 3.0).powi(2) / 12.0
        };
        let gain_alpha = (-1.0_f64 / (0.005 * 48_000.0)).exp();
        history.gain_db = gain_alpha * history.gain_db + (1.0 - gain_alpha) * reduction_db;
        let gain = 10.0_f64.powf(history.gain_db / 20.0);
        [
            (f64::from(frame[0]) * gain) as f32,
            (f64::from(frame[1]) * gain) as f32,
        ]
    };
    input
        .iter()
        .copied()
        .map(|frame| {
            let direct = process(frame, track_threshold_db, track_history);
            let send_input = [direct[0] * 0.5, direct[1] * 0.5];
            let returned = process(send_input, return_threshold_db, return_history);
            [
                direct[0] + returned[0] * 0.75,
                direct[1] + returned[1] * 0.75,
            ]
        })
        .collect()
}

#[cfg(feature = "allocation-probe")]
#[derive(Default)]
struct CompressorReferenceHistory {
    envelope: f64,
    gain_db: f64,
}

pub(super) fn metronome_graph() -> Playback {
    let mut project = hex_project::Project::new(8_000).unwrap().snapshot();
    project.loop_region = Some(hex_project::LoopRegion::new(0, 4_050, true).unwrap());
    let revision = hex_engine::PreparedRevision::prepare(
        &hex_project::Project::from_snapshot(project).unwrap(),
        &std::collections::BTreeMap::new(),
        Arc::new(hex_engine::SineTable::new()),
        None,
    )
    .unwrap();
    Playback::from_prepared(Arc::new(revision))
}

fn metronome_prerolled_seek(
    revision: Arc<hex_engine::PreparedRevision>,
    destination: u64,
) -> Playback {
    std::thread::Builder::new()
        .stack_size(8 * 1024 * 1024)
        .spawn(move || {
            let runtime =
                hex_engine::SessionPlayback::prepare_interactive_arrangement_preroll_from_revision(
                    revision,
                    destination,
                    destination,
                    true,
                    &AtomicBool::new(false),
                )
                .unwrap();
            Playback::from_interactive_arrangement_preroll(runtime).unwrap()
        })
        .unwrap()
        .join()
        .unwrap()
}

#[cfg(feature = "allocation-probe")]
fn check_held_arrangement_loop_handoff(old_enabled: bool, new_enabled: bool, block_frames: usize) {
    use hex_project::{BeatTime, Command, Created, Editor, Project};
    let class = hex_engine::NativeDeviceClass::Instrument;
    let state = hex_engine::default_native_device_state_for_class(
        class,
        hex_engine::DeviceTypeId::from_static("hex.instrument.operator"),
    )
    .unwrap();
    let state = hex_engine::patch_native_device_parameter_for_class(
        class,
        &state,
        hex_lattice_contract::ParameterId::from_static(78),
        hex_lattice_contract::ParameterValue::Float(0.4),
    )
    .unwrap();
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "loop probe".into(),
            before_track: None,
            slot: hex_project::TrackDeviceSlot::Instrument,
            state,
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::from_quarters(10).unwrap(),
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ONE,
            duration: BeatTime::from_quarters(2).unwrap(),
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    // Leave room for the native instrument's conservative release bound.
    editor
        .execute(Command::SetLoopRegion {
            start: 48_000,
            end: 96_000_000,
            enabled: old_enabled,
        })
        .unwrap();
    let prepare = |project: &Project| {
        Playback::from_prepared(Arc::new(
            hex_engine::PreparedRevision::prepare(
                project,
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        ))
    };
    let mut reference = prepare(editor.project());
    reference.apply(PlaybackCommand::Play);
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(prepare(editor.project())).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut prefix = vec![[0.0; 2]; 30_000];
    for chunk in prefix.chunks_mut(256) {
        callback.begin_block();
        callback.render(chunk).unwrap();
    }
    let mut expected_prefix = prefix.clone();
    reference.render(&mut expected_prefix).unwrap();
    assert_eq!(prefix, expected_prefix);
    editor
        .execute(Command::SetLoopRegion {
            start: 24_000,
            end: 120_000_000,
            enabled: new_enabled,
        })
        .unwrap();
    let revision = control.publish(prepare(editor.project())).unwrap();
    let mut actual = vec![[0.0; 2]; block_frames * 3];
    let measured = allocation_counter::measure(|| {
        for chunk in actual.chunks_mut(block_frames) {
            callback.begin_block();
            callback.render(chunk).unwrap();
        }
    });
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Relaxed), revision);
    assert_eq!(control.status.rejected_revision(), None);
    let mut expected = vec![[0.0; 2]; actual.len()];
    for chunk in expected.chunks_mut(block_frames) {
        reference.render(chunk).unwrap();
    }
    assert!(expected.iter().flatten().any(|sample| sample.abs() > 1e-6));
    assert_eq!(actual, expected, "held source must keep its DSP history");
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

fn spectrum_test_channel() -> (
    Control,
    Callback,
    crate::spectrum_worker::SpectrumReader,
    crate::spectrum_worker::SpectrumReaper,
) {
    let stamp = SpectrumStamp {
        target: crate::spectrum_worker::SpectrumTargetId(0),
        generation: 0,
        sample_rate_hz: 48_000,
        applied_revision: 0,
    };
    let (capture, reader, reaper) = crate::spectrum_worker::spawn_spectrum_worker(stamp).unwrap();
    let (control, callback) = channel_with_spectrum(48_000, Some(capture)).unwrap();
    (control, callback, reader, reaper)
}

fn shutdown_spectrum_test_channel(
    mut control: Control,
    mut callback: Callback,
    reaper: crate::spectrum_worker::SpectrumReaper,
) {
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
    drop(callback);
    drop(control);
    reaper.shutdown_and_join().unwrap();
}

fn assert_fresh_fence_rejection(control: &mut Control, expected_revision: u64) {
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            expected_revision,
            PlaybackHandoffRejection::RuntimeFenceChanged
        ))
    );
    assert!(!control.status.publication_pending.load(Acquire));
}

fn assert_consumed_stop_invalidates_fresh_fence(resident: bool) {
    let make_graph = || {
        if resident {
            session_audio_graph().0
        } else {
            graph().0
        }
    };
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(make_graph()).unwrap();
    begin_outcome_test_block(&mut callback);
    control.command(PlaybackCommand::Play).unwrap();
    begin_outcome_test_block(&mut callback);
    assert!(control.status.playing.load(Acquire));
    let stale = control.capture_fresh_runtime_fence().unwrap();
    control.stop();
    assert!(control.capture_fresh_runtime_fence().is_none());
    begin_outcome_test_block(&mut callback);
    assert!(!control.status.playing.load(Acquire));
    let mut candidate = make_graph();
    candidate.apply(PlaybackCommand::Play);
    let rejected = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.status.playing.load(Acquire));
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 1);
    let fresh = control.capture_fresh_runtime_fence().unwrap();
    let mut candidate = make_graph();
    candidate.apply(PlaybackCommand::Play);
    let accepted = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fresh),
        )
        .unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(control.status.applied_revision.load(Acquire), accepted);
    assert!(control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
fn source_bound_start_fences(note_chase: hex_engine::NoteChase) {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use hex_engine::AudioSourceContext;

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let old = AudioSourceContext {
        project_generation: 0,
        editor_revision: 4,
    };
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntime,
            0,
            true,
            false,
            Some(old),
        )
        .unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.bind_resident_transport(0);
    let source = client.status().unwrap().recording_source().unwrap();
    let wrong_output = AppliedAudioSource {
        output_identity: source.output_identity + 1,
        ..source
    };
    assert!(
        client
            .play_from_with_note_chase_source_with_ticket(1, note_chase, wrong_output)
            .is_err()
    );
    let wrong_context = AppliedAudioSource {
        context: AudioSourceContext {
            project_generation: 1,
            ..old
        },
        ..source
    };
    assert!(
        client
            .play_from_with_note_chase_source_with_ticket(1, note_chase, wrong_context)
            .is_err()
    );

    let wrong_revision = AppliedAudioSource {
        audio_revision: source.audio_revision + 1,
        ..source
    };
    assert!(
        client
            .play_from_with_note_chase_source_with_ticket(17, note_chase, wrong_revision)
            .is_err()
    );
    let ticket = client
        .play_from_with_note_chase_source_with_ticket(17, note_chase, source)
        .unwrap();
    // Inspect the actual borrowed consumer slot without removing it: a
    // cursor-only oracle would miss an endpoint that discarded the choice.
    data.owner_mut().unwrap().run(|callback, _| {
        let queued = callback.commands.peek().unwrap();
        assert_eq!(queued.ticket, ticket);
        assert_eq!(
            queued.action,
            PlaybackAction::Transport(PlaybackCommand::PlayFromWithNoteChase {
                frame: 17,
                note_chase
            })
        );
        assert_eq!(queued.expected_source, Some(source));
    });
    let measured =
        allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()));
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Applied
        })
    );
    assert!(client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 17);
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());

    let control = publication.control().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    // A graph can be replaced even while its canonical context is unchanged.
    let next = old;
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            0,
            true,
            false,
            Some(next),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    data.owner_mut()
        .unwrap()
        .run(|c, _| c.test_after_fresh_fence_validation = Some(Arc::clone(&barrier)));
    let ticket = std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            allocation_counter::measure(|| data.owner_mut().unwrap().run(|c, _| c.begin_block()))
        });
        barrier.wait();
        let ticket = client
            .play_from_with_note_chase_source_with_ticket(47, note_chase, source)
            .unwrap();
        barrier.wait();
        let measured = callback.join().unwrap();
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        ticket
    });
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Rejected(PlaybackActionRejection::AudioSourceChanged)
        })
    );
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_eq!(
        client.status().unwrap().recording_source().unwrap().context,
        next
    );
    assert_ne!(
        client
            .status()
            .unwrap()
            .recording_source()
            .unwrap()
            .audio_revision,
        source.audio_revision
    );
    assert!(
        client
            .play_from_with_note_chase_source_with_ticket(47, note_chase, source)
            .is_err()
    );
    let source = client.status().unwrap().recording_source().unwrap();
    for _ in 0..COMMANDS {
        client
            .play_from_with_note_chase_source_with_ticket(2, note_chase, source)
            .unwrap();
    }
    assert!(
        client
            .play_from_with_note_chase_source_with_ticket(2, note_chase, source)
            .is_err()
    );
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert!(!client.status().unwrap().playing.load(Acquire));
    client.close();
    assert_eq!(
        client.play_from_with_note_chase_source_with_ticket(2, note_chase, source),
        Err(PlaybackCommand::PlayFromWithNoteChase {
            frame: 2,
            note_chase
        }),
    );
    publication.close();
    drop(data);
    drop(errors);
    assert!(
        crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert!(retirement.advance().unwrap());
}

fn admission_gain_target() -> crate::gain_recording::Target {
    crate::gain_recording::Target {
        track_id: 1,
        project_generation: 1,
        canonical_revision: 1,
        audio_revision: 1,
        recording_epoch: 1,
        transport_epoch: 1,
        gesture_serial: 1,
        sample_rate: 48_000,
    }
}

fn graph() -> (Playback, Weak<hex_engine::Project>) {
    graph_with_frame([0.25; 2])
}

fn wire_action_outcomes(control: &mut Control, callback: &mut Callback) -> action_outcomes::Reader {
    control.enable_action_outcomes_on_control(callback).unwrap()
}

fn begin_outcome_test_block(callback: &mut Callback) {
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(|| callback.begin_block());
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
}

fn graph_with_frame(frame: hex_engine::Frame) -> (Playback, Weak<hex_engine::Project>) {
    let audio = hex_engine::Audio::new(48_000, vec![frame]).unwrap();
    let clip = hex_engine::Clip::new(audio, 0, 100, true).unwrap();
    let track = hex_engine::Track::new(vec![clip], 1.0, false).unwrap();
    let project = Arc::new(hex_engine::Project::new(48_000, vec![track]).unwrap());
    let weak = Arc::downgrade(&project);
    (Playback::from_shared(project), weak)
}

fn session_audio_graph() -> (
    Playback,
    Weak<hex_engine::PreparedRevision>,
    hex_project::ClipId,
    hex_project::TrackId,
) {
    use hex_project::{
        AssetSnapshot, Project as CanonicalProject, SceneSnapshot, SessionAudioClipSnapshot,
        SessionCellContentSnapshot, SessionCellSnapshot, SessionSnapshot, Snapshot, Tempo,
        TempoPoint, TrackContentSnapshot, TrackSnapshot,
    };
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: 5,
        assets: vec![AssetSnapshot {
            original_source: None,
            id: 4,
            path: "session.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 2,
            playback_frames: 2,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        }],
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: 1,
            name: "audio".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Audio(Vec::new()),
            sends: Vec::new(),
        }],
        session: SessionSnapshot {
            scenes: vec![SceneSnapshot {
                id: 2,
                name: "scene".into(),
            }],
            cells: vec![SessionCellSnapshot {
                track: 1,
                scene: 2,
                content: SessionCellContentSnapshot::Audio(SessionAudioClipSnapshot {
                    loop_range: None,
                    active: true,
                    flavor: hex_project::ClipFlavor::default(),
                    gain: hex_project::ClipGain::UNITY,
                    id: 3,
                    name: "one shot".into(),
                    asset: 4,
                    source_start: 0,
                    source_end: 2,
                }),
            }],
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })
    .unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.25, -0.5], [0.75, -1.0]]).unwrap();
    let media = [(project.assets().next().unwrap().id(), audio)]
        .into_iter()
        .collect();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let weak = Arc::downgrade(&revision);
    let clip = revision.session().audio_clips()[0].id;
    let track = revision.session().audio_clips()[0].track;
    (
        Playback::try_from_prepared_with_session(revision).unwrap(),
        weak,
        clip,
        track,
    )
}

fn session_playback(track_id: u64) -> Playback {
    use hex_project::{
        Project as CanonicalProject, SessionSnapshot, Snapshot, Tempo, TempoPoint,
        TrackContentSnapshot, TrackSnapshot,
    };
    let project = CanonicalProject::from_legacy_snapshot(Snapshot {
        automatic_track_names: Vec::new(),
        mixer_automation: Vec::new(),
        frozen_tracks: Vec::new(),
        track_io: Vec::new(),
        retained_source_paths: Vec::new(),
        aliases: Default::default(),
        native_sources: Vec::new(),
        arrangement_sections: Vec::new(),
        arrangement_locators: Vec::new(),
        main_devices: Vec::new(),
        tuning: Default::default(),
        audio_feeds: Vec::new(),
        midi_routes: Vec::new(),
        sidechains: Vec::new(),
        return_feeds: Vec::new(),
        import_provenance: None,
        sample_rate: 48_000,
        sample_rate_mode: Default::default(),
        next_id: track_id + 1,
        assets: Vec::new(),
        tracks: vec![TrackSnapshot {
            group_output_mutes: Vec::new(),
            tags: Vec::new(),
            kind: hex_project::TrackKind::Ordinary,
            parent_group: None,
            color: hex_project::ElementColor::default(),
            gain_envelope: Vec::new(),
            gain_touch_spans: Vec::new(),
            gain_touch_epoch: 0,
            id: track_id,
            name: "keys".into(),
            gain: 1.0,
            balance: 0.0,
            muted: false,
            soloed: false,
            devices: Vec::new(),
            instrument: None,
            instrument_position: None,
            content: TrackContentSnapshot::Instrument(Vec::new()),
            sends: Vec::new(),
        }],
        session: SessionSnapshot {
            scenes: Vec::new(),
            cells: Vec::new(),
        },
        loop_region: None,
        master_gain: 1.0,
        tempo_points: vec![TempoPoint::new(hex_project::BeatTime::ZERO, Tempo::DEFAULT)],
        meter_points: vec![hex_project::MeterPoint::new(
            hex_project::BeatTime::ZERO,
            hex_project::Meter::COMMON_TIME,
        )],
        return_tracks: Vec::new(),
    })
    .unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            &project,
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    Playback::try_from_prepared_with_session(revision).unwrap()
}

#[path = "admission_tests.rs"]
mod admission_tests;
mod allocation_probe_tests;
mod audition_and_fault_tests;
#[path = "endpoints_tests.rs"]
mod endpoints_tests;
#[path = "ordinary_streaming_tests.rs"]
mod ordinary_streaming_tests;
#[path = "publication_receipt_tests.rs"]
mod publication_receipt_tests;
mod recording_and_seek_tests;
mod route_and_keyboard_tests;
mod spectrum_and_context_tests;
