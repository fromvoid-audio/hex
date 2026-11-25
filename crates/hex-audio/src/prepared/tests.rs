use super::*;
use std::sync::{Barrier, Weak};

#[test]
fn prepared_channel_accepts_only_supported_sample_rates() {
    for rate in [0, 7_999, 192_001, u32::MAX] {
        assert!(channel(rate).is_err(), "accepted {rate} hz");
    }
    for rate in [8_000, 192_000] {
        assert!(channel(rate).is_ok(), "rejected {rate} hz");
    }
}

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

#[test]
fn metronome_admission_is_reported_and_priority_stop_discards_older_toggle() {
    let (mut control, mut callback) = channel(8_000).unwrap();
    control.publish(metronome_graph()).unwrap();
    callback.begin_block();
    let ticket = control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    assert!(!control.status.metronome_enabled.load(Relaxed));
    callback.begin_block();
    assert!(control.status.metronome_enabled.load(Relaxed));
    assert!(control.status.consumed_command.load(Acquire) >= ticket);
    control
        .action_with_ticket(PlaybackAction::SetMetronome(false))
        .unwrap();
    control.stop();
    callback.begin_block();
    assert!(control.status.metronome_enabled.load(Relaxed));
    assert!(!control.status.playing.load(Relaxed));
    control
        .action_with_ticket(PlaybackAction::SetMetronome(false))
        .unwrap();
    callback.begin_block();
    assert!(!control.status.metronome_enabled.load(Relaxed));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
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

#[test]
fn metronome_fresh_seek_preserves_only_monitoring_after_accepted_exact_source_fence() {
    for (same_source, same_context, conditional, enabled) in [
        (true, true, true, true),
        (false, true, true, false),
        (true, false, true, false),
        (true, true, false, false),
    ] {
        let (old, _) = fresh_spectrum_eq8_graph();
        let old_source = Arc::clone(old.prepared_revision().unwrap());
        assert!(old_source.arrangement_seek_requires_preroll());
        let source = if same_source {
            Arc::clone(&old_source)
        } else {
            Arc::clone(fresh_spectrum_eq8_graph().0.prepared_revision().unwrap())
        };
        let mut oracle = metronome_prerolled_seek(Arc::clone(&source), 200);
        let mut expected = [[0.0; 2]; 128];
        oracle.render(&mut expected).unwrap();
        let candidate = metronome_prerolled_seek(source, 200);
        assert_eq!(
            Arc::ptr_eq(candidate.prepared_revision().unwrap(), &old_source),
            same_source
        );
        assert!(!candidate.metronome_enabled());
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                12,
            )
            .unwrap();
        control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 37]).unwrap();
        assert!(control.status.metronome_enabled.load(Relaxed));
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let policy = if conditional {
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence)
        } else {
            PublicationPolicy::InstallFreshRuntime
        };
        let revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                policy,
                if same_context { 12 } else { 13 },
            )
            .unwrap();
        let mut output = [[0.0; 2]; 128];
        let render = || {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        };
        #[cfg(feature = "allocation-probe")]
        {
            let allocations = allocation_counter::measure(render);
            assert_eq!(
                (
                    allocations.count_total,
                    allocations.bytes_total,
                    allocations.count_current,
                    allocations.bytes_current
                ),
                (0, 0, 0, 0)
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        {
            let mut render = render;
            render();
        }
        assert_eq!(control.status.applied_revision.load(Acquire), revision);
        assert_eq!(control.status.metronome_enabled.load(Relaxed), enabled);
        assert_eq!(control.status.position.load(Relaxed), 328);
        assert_eq!(control.status.performance_frame.load(Relaxed), 328);
        assert_eq!(
            output, expected,
            "fresh DSP/transport history and a reset click tail must match the preroll oracle"
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn metronome_rejected_fresh_seek_keeps_active_toggle_and_position() {
    for cancelled in [false, true] {
        let (old, _) = fresh_spectrum_eq8_graph();
        let source = Arc::clone(old.prepared_revision().unwrap());
        let candidate = metronome_prerolled_seek(source, 200);
        let (mut control, mut callback) = channel(48_000).unwrap();
        let active_revision = control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                12,
            )
            .unwrap();
        control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 37]).unwrap();
        let fence = control.capture_fresh_runtime_fence().unwrap();
        let rejected = control
            .publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                12,
            )
            .unwrap();
        if cancelled {
            assert_eq!(
                control.cancel_fresh_runtime_publication(rejected),
                FreshRuntimeCancellationResult::Cancelled
            );
        } else {
            control.command(PlaybackCommand::Pause).unwrap();
        }
        callback.begin_block();
        assert_eq!(
            control.status.applied_revision.load(Acquire),
            active_revision
        );
        assert_eq!(
            control.status.rejected_revision(),
            Some((
                rejected,
                if cancelled {
                    PlaybackHandoffRejection::FreshRuntimeCancelled
                } else {
                    PlaybackHandoffRejection::RuntimeFenceChanged
                }
            ))
        );
        assert!(control.status.metronome_enabled.load(Relaxed));
        assert_eq!(control.status.position.load(Relaxed), 37);
        // A queued Pause is Applied when consumed but keeps playing
        // through its fade until a render reaches its audible end (#4151).
        assert!(control.status.playing.load(Relaxed));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn clip_gain_with_shaped_fades_and_replacement_allocates_and_frees_nothing_in_callback() {
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
    let hex_project::Created::Imported { asset, clip, .. } = editor
        .execute(hex_project::Command::ImportAudioTrack {
            name: "clip gain".into(),
            path: "gain.wav".into(),
            sample_rate: 48_000,
            frames: 1_024,
        })
        .unwrap()
    else {
        panic!("import")
    };
    let fade = hex_project::ClipFade::new(1_024, 0, 64, 64)
        .unwrap()
        .with_shape(4, 4, 700, -700)
        .unwrap()
        .with_curve_positions(250, 750)
        .unwrap()
        .with_curve_laws([
            hex_project::ClipFade::DEFAULT_S_CURVE,
            hex_project::FadeCurveLaw::Linear,
        ]);
    editor
        .execute(hex_project::Command::SetClipFadeEnvelope { clip, fade })
        .unwrap();
    let audio = hex_engine::Audio::new(48_000, vec![[0.5, -0.25]; 1_024]).unwrap();
    let media = std::collections::BTreeMap::from([(asset, audio)]);
    let prepare = |editor: &hex_project::Editor| {
        let revision = hex_engine::PreparedRevision::prepare(
            editor.project(),
            &media,
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap();
        Playback::try_from_prepared_with_session(Arc::new(revision)).unwrap()
    };
    editor
        .execute(hex_project::Command::SetAudioClipGain {
            clip,
            millidb: -6_000,
        })
        .unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(prepare(&editor)).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut prefix = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let gain = hex_project::ClipGain::new(-6_000).unwrap().linear();
    assert_eq!(prefix[0], [0.0; 2]);
    assert_eq!(
        prefix[32],
        [
            0.5 * fade.gain_at(32) * gain,
            -0.25 * fade.gain_at(32) * gain
        ]
    );
    assert_eq!(prefix[68], [0.5 * gain, -0.25 * gain]);
    editor
        .execute(hex_project::Command::SetAudioClipGain {
            clip,
            millidb: 6_000,
        })
        .unwrap();
    control.publish(prepare(&editor)).unwrap();
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let gain = hex_project::ClipGain::new(6_000).unwrap().linear();
    assert_eq!(output, [[0.5 * gain, -0.25 * gain]; 128]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn metronome_commands_loops_and_handoff_allocate_and_free_nothing_in_callback() {
    let (mut control, mut callback) = channel(8_000).unwrap();
    control.publish(metronome_graph()).unwrap();
    callback.begin_block();
    control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        for _ in 0..65 {
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(control.status.metronome_enabled.load(Relaxed));
    control.publish(metronome_graph()).unwrap();
    control.command(PlaybackCommand::Seek(0)).unwrap();
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    // The Seek applies after its 21-frame fade at 8 kHz, where the
    // undamped metronome click at frame zero follows (#4140).
    assert_eq!(output[21], [0.16; 2]);
    control.command(PlaybackCommand::Pause).unwrap();
    let allocations = allocation_counter::measure(|| {
        // The first block is the queued Pause's 128-frame fade (#4151).
        for _ in 0..2 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(output, [[0.0; 2]; 128]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn held_mutable_arrangement_loop_handoff_preserves_pcm_without_callback_allocation() {
    for (old_enabled, new_enabled, block_frames) in
        [(true, true, 256), (false, true, 17), (true, false, 1)]
    {
        check_held_arrangement_loop_handoff(old_enabled, new_enabled, block_frames);
    }
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

#[cfg(feature = "allocation-probe")]
#[test]
fn saturator_track_and_return_revision_admission_and_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let playback = saturator_track_and_return_graph(36.0);
    control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    let mut original_output = [[0.0; 2]; 128];
    callback
        .render(&mut original_output)
        .expect("initial callback render");
    // Each oversampled Saturator answers a step with exact DC once its
    // declared latency and finite tail have elapsed. Main waits for the
    // return path, two Saturators deep; pre-epoch output is silent.
    let saturator = hex_devices::saturator::SaturatorDefinition::prepare_owned(
        &hex_lattice_contract::NativeDeviceMetadata::default_state(
            &hex_devices::saturator::SATURATOR_DEFINITION,
        )
        .expect("default saturator"),
        48_000,
        &[],
    )
    .expect("standalone saturator");
    let latency = hex_lattice::PreparedNativeProcessor::algorithmic_latency_frames(&saturator)
        .expect("declared latency") as usize;
    let hex_lattice::OutputTail::Finite { max_tail_frames } =
        hex_lattice::PreparedNativeProcessor::output_tail(&saturator)
    else {
        panic!("finite saturator tail");
    };
    let settled = 2 * (latency + max_tail_frames as usize);
    assert_eq!((latency, settled), (11, 72));
    assert!(
        original_output[..2 * latency]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );
    assert!(
        original_output[settled..]
            .iter()
            .all(|frame| { (frame[0] - 1.75).abs() < 1.0e-6 && (frame[1] + 1.75).abs() < 1.0e-6 })
    );

    control
        .publish(saturator_track_and_return_graph(0.0))
        .expect("publish edited prepared revision");
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).expect("callback render");
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("callback render during ramp");
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("callback render after ramp");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert_ne!(output, original_output);
    // At 48 kHz the 5 ms ramp from the edit at frame 128 ends at input
    // frame 368; Main settles on the exact target `settled` frames later,
    // inside the third 128-frame block (frames 384..512).
    let target = 128 + 240 + settled - 384;
    assert_eq!(target, 56);
    assert!(
        output[target..].iter().all(|frame| {
            (frame[0] - 0.34375).abs() < 1.0e-6 && (frame[1] + 0.55).abs() < 1.0e-6
        })
    );
    assert!(output[..target].iter().any(|frame| {
        (frame[0] - 0.34375).abs() >= 1.0e-6 || (frame[1] + 0.55).abs() >= 1.0e-6
    }));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn overdrive_track_return_handoff_and_render_allocate_nothing() {
    use hex_devices::overdrive::{OVERDRIVE_DEFINITION as d, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let original = d.default_state().unwrap();
    control
        .publish(native_effect_track_and_return_graph(original.clone()))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    let changed = d
        .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
        .unwrap();
    control
        .publish(native_effect_track_and_return_graph(changed))
        .unwrap();
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn rattle_track_return_handoff_and_render_allocate_nothing() {
    use hex_devices::rattle::{RATTLE_DEFINITION as d, parameter};
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let original = d.default_state().unwrap();
    control
        .publish(native_effect_track_and_return_graph(original.clone()))
        .unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    let mut output = [[0.0; 2]; 128];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    // The edited revision inherits oversampler/noise history and glides drive.
    let changed = d
        .patch_parameter(&original, parameter::DRIVE, ParameterValue::Float(1.0))
        .unwrap();
    control
        .publish(native_effect_track_and_return_graph(changed))
        .unwrap();
    let allocations = allocation_counter::measure(|| {
        for _ in 0..4 {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        }
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(output.iter().flatten().all(|sample| sample.is_finite()));
    assert!(output.iter().flatten().any(|sample| *sample != 0.0));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn compressor_track_and_return_revision_admission_and_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    control
        .publish(compressor_track_and_return_graph(-18.0))
        .expect("publish compressor graph");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();

    let input = [[0.25, -0.4]; 128];
    let mut reference_track = CompressorReferenceHistory::default();
    let mut reference_return = CompressorReferenceHistory::default();
    let expected_before = compressor_routed_reference(
        -18.0,
        -18.0,
        &mut reference_track,
        &mut reference_return,
        &input,
    );
    let mut original_output = [[0.0; 2]; 128];
    callback
        .render(&mut original_output)
        .expect("initial callback render");
    assert_eq!(original_output.as_slice(), expected_before.as_slice());

    let mut changed_track = CompressorReferenceHistory {
        envelope: reference_track.envelope,
        gain_db: reference_track.gain_db,
    };
    let mut changed_return = CompressorReferenceHistory {
        envelope: reference_return.envelope,
        gain_db: reference_return.gain_db,
    };
    let expected_after = compressor_routed_reference(
        -42.0,
        -42.0,
        &mut changed_track,
        &mut changed_return,
        &input,
    );
    control
        .publish(compressor_track_and_return_graph(-42.0))
        .expect("publish edited compressor revision");
    let mut output = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut output)
            .expect("edited callback render");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert_eq!(output.as_slice(), expected_after.as_slice());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn reverb_track_return_render_edit_removal_stop_seek_and_shutdown_allocate_nothing() {
    use hex_lattice_contract::{DeviceTypeId, ParameterId, ParameterValue};
    let state =
        hex_engine::default_native_device_state(DeviceTypeId::from_static("hex.reverb")).unwrap();
    let changed = hex_engine::patch_native_device_parameter(
        &state,
        ParameterId::from_static(1),
        ParameterValue::Float(2.5),
    )
    .unwrap();
    let frozen = hex_engine::patch_native_device_parameter(
        &changed,
        ParameterId::from_static(26),
        ParameterValue::Bool(true),
    )
    .unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut block = [[0.0; 2]; 127];
    for state in [state, changed, frozen] {
        control
            .publish(native_track_and_return_graph(Some(state)))
            .unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let counts = allocation_counter::measure(|| {
            for _ in 0..40 {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(block.iter().flatten().any(|v| v.abs() > 0.0));
        control.reclaim();
    }
    control
        .publish(native_track_and_return_graph(None))
        .unwrap();
    let counts = allocation_counter::measure(|| {
        for _ in 0..8 {
            callback.begin_block();
            callback.render(&mut block).unwrap();
        }
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(
        block.iter().flatten().any(|v| v.abs() > 0.0),
        "removed effects drain their real boxed histories"
    );
    control.reclaim();
    for command in [PlaybackCommand::Seek(0), PlaybackCommand::Stop] {
        control.command(command).unwrap();
        let counts = allocation_counter::measure(|| {
            // A queued Stop fades over 128 frames, past one block (#4151).
            for _ in 0..1 + usize::from(command == PlaybackCommand::Stop) {
                callback.begin_block();
                callback.render(&mut block).unwrap();
            }
        });
        assert_eq!(
            (
                counts.count_total,
                counts.bytes_total,
                counts.count_current,
                counts.bytes_current
            ),
            (0, 0, 0, 0)
        );
    }
    assert_eq!(block, [[0.0; 2]; 127]);
    control.request_shutdown();
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
    });
    assert_eq!(
        (
            counts.count_total,
            counts.bytes_total,
            counts.count_current,
            counts.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn gate_track_and_return_initial_and_edited_callback_render_allocate_nothing() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    control
        .publish(gate_track_and_return_graph(-60.0))
        .expect("publish gate graph");
    control.command(PlaybackCommand::Play).expect("play");
    let mut initial = [[0.0; 2]; 128];
    let initial_allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut initial)
            .expect("initial callback render");
    });
    assert_eq!(
        (
            initial_allocations.count_total,
            initial_allocations.bytes_total,
            initial_allocations.count_current,
            initial_allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(initial.iter().all(|frame| {
        (frame[0] - 0.00025009375).abs() < 1.0e-8 && (frame[1] + 0.00040015).abs() < 1.0e-8
    }));

    control
        .publish(gate_track_and_return_graph(-20.0))
        .expect("publish floor-edited Gate graph");
    let mut edited = [[0.0; 2]; 128];
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback
            .render(&mut edited)
            .expect("edited callback render");
    });
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    let start = 10.0_f64.powf(-60.0 / 20.0);
    let target = 10.0_f64.powf(-20.0 / 20.0);
    let step = (target - start) / 4_800.0;
    for (index, frame) in edited.iter().enumerate() {
        let gain = start + step * (index as f64 + 1.0);
        let direct_left = 0.25 * gain;
        let direct_right = -0.4 * gain;
        let expected = [
            direct_left + direct_left * 0.5 * gain * 0.75,
            direct_right + direct_right * 0.5 * gain * 0.75,
        ];
        assert!((f64::from(frame[0]) - expected[0]).abs() < 2.0e-8);
        assert!((f64::from(frame[1]) - expected[1]).abs() < 2.0e-8);
    }
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn seek_cancels_a_waiting_begin_without_blocking_a_later_fresh_press() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        first_revision
    );

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, 2, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    control.command(PlaybackCommand::Seek(0)).unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);

    let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision,
        "publication rejection: {:?}",
        callback.status.rejected_revision()
    );
    assert_eq!(callback.status.rejected_revision(), None);
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    assert_eq!(callback.processor_audition_lease, None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control
        .processor_audition
        .select(identity, 3, second_revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn audition_waits_for_actual_generation_and_revision_floor_then_activates() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        first_revision
    );

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, first_revision + 1, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    let (replacement, replacement_identity) = fresh_spectrum_eq8_graph();
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .control_context_generation,
        3
    );
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    assert!(output.iter().flatten().any(|value| value.abs() > 1.0e-6));

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn sparse_eq_callback_publication_held_stable_audition_release_and_retirement_allocate_nothing() {
    let (mut control, mut callback) = channel(48000).unwrap();
    let (playback, identity) = spectrum_eq_graph_with_params(true, 0, 3.0, 1.0, true, false);
    let first = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    let mut prefix = [[0.0; 2]; 17];
    let admission = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut prefix).unwrap();
    });
    assert_eq!(
        (
            admission.count_total,
            admission.bytes_total,
            admission.count_current,
            admission.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let selection = hex_engine::ProcessorAuditionSelection {
        index: 1000,
        lane: 0,
    };
    control
        .processor_audition
        .select(identity, 2, first, 48000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    let held = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            held.count_total,
            held.bytes_total,
            held.count_current,
            held.bytes_current
        ),
        (0, 0, 0, 0)
    );
    let (candidate, _) = spectrum_eq_graph_with_params(false, 0, 6.0, 2.0, true, false);
    let revision = control
        .publish_with_policy_and_context_generation(candidate, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    let publication = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            publication.count_total,
            publication.bytes_total,
            publication.count_current,
            publication.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    control.processor_audition.release().unwrap();
    let mut fade = [[0.0; 2]; 256];
    let release = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut fade).unwrap();
    });
    assert_eq!(
        (
            release.count_total,
            release.bytes_total,
            release.count_current,
            release.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(callback.processor_audition_lease.is_none());
    control
        .processor_audition
        .select(identity, 2, revision, 48000, selection)
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    control.processor_audition.cancel_immediately().unwrap();
    let cancel = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    assert_eq!(
        (
            cancel.count_total,
            cancel.bytes_total,
            cancel.count_current,
            cancel.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(callback.processor_audition_lease.is_none());
    control.request_shutdown();
    let retire = allocation_counter::measure(|| {
        callback.begin_block();
    });
    assert_eq!(
        (
            retire.count_total,
            retire.bytes_total,
            retire.count_current,
            retire.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
}

#[test]
fn compatible_parameter_publications_preserve_the_held_audition() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, first_revision, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    for (gain, q) in [(6.0, 1.0), (6.0, 2.0)] {
        let (candidate, candidate_identity) = spectrum_eq8_graph_with_params(false, 0, gain, q);
        assert_eq!(candidate_identity, identity);
        let revision = control
            .publish_with_policy_and_context_generation(
                candidate,
                PublicationPolicy::InheritRuntime,
                2,
            )
            .unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert_eq!(
            callback
                .active
                .as_ref()
                .and_then(ActivePublication::legacy)
                .unwrap()
                .revision,
            revision
        );
        assert_eq!(
            callback.processor_audition_lease,
            control.processor_audition.desired()
        );
        assert!(
            callback
                .active
                .as_mut()
                .and_then(ActivePublication::legacy_mut)
                .unwrap()
                .playback
                .processor_audition_is_active(identity, selection)
        );
    }

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn wrong_sample_rate_target_is_rejected_without_replaying() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, revision, 44_100, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    let epoch = callback.processor_audition.current_epoch();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn mode_change_invalidates_the_old_audition_epoch_and_needs_a_fresh_press() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, first_revision, 48_000, selection)
        .unwrap();
    let mut output = [[0.0; 2]; 64];
    callback.begin_block();
    let epoch = callback.processor_audition.current_epoch();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );

    // Stereo -> left/right invalidates the prior transient selection while
    // keeping the stable device ID. The old held epoch must stay rejected.
    let (replacement, replacement_identity) = spectrum_eq8_graph_with_mode(false, 1);
    assert_eq!(replacement_identity, identity);
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InheritRuntime,
            2,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition_rejected_epoch, epoch);

    control
        .processor_audition
        .select(identity, 2, second_revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let fresh_epoch = callback.processor_audition.current_epoch();
    assert!(fresh_epoch > epoch);
    callback.render(&mut output).unwrap();
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn a_fresh_audition_admitted_after_priority_stop_survives_same_block_consumption() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    control.stop();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 2, revision, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();

    assert!(
        !callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .playing()
    );
    assert_eq!(
        callback.processor_audition_lease,
        control.processor_audition.desired()
    );
    assert!(
        callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn render_fault_refuses_new_capture_admissions() {
    let (control, mut callback) = channel(48_000).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 0);
    assert!(control.status.reserve_midi_begin().is_err());
    assert!(control.status.reserve_gain_begin().is_err());
    assert!(control.status.reserve_native_parameter_input().is_err());
}

#[test]
fn stale_play_queued_before_fault_does_not_restart_resident_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let mut output = [[0.0; 2]; 16];
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(
        !control.status.playing.load(Acquire),
        "pre-fault queued Play restarted the faulted graph"
    );
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn admission_delayed_across_fault_cannot_restart_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    // Model a producer suspended after reading the actual admission fields
    // but before publishing its fixed-size command into the ring.
    let epoch = control.status.transport_epoch.load(Acquire);
    let gate = control.status.sampler_edit.gate();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let ticket = control.command_ticket + 1;
    assert!(
        control
            .commands
            .push(QueuedCommand {
                ticket,
                action: PlaybackAction::Transport(PlaybackCommand::PlayFrom(45)),
                project_generation: None,
                expected_source: None,
                sampler_edit_gate: gate,
                transport_epoch: epoch,
            })
            .is_ok()
    );
    control.command_ticket = ticket;
    control.status.admitted_ticket.store(ticket, Release);
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 32);
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stale_play_is_rejected_after_replacement_and_new_play_is_allowed() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    let stale = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let stale_second = control
        .command_with_ticket(PlaybackCommand::PlayFrom(45))
        .unwrap();
    let (replacement, _) = fresh_spectrum_eq8_graph();
    let revision = control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| callback.begin_block());
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(
        control.status.rejected_action(),
        Some((stale_second, PlaybackActionRejection::OutputFailed))
    );
    for ticket in [stale, stale_second] {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::OutputFailed),
            })
        );
    }
    assert!(!control.status.playing.load(Acquire));
    let fresh = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: fresh,
            outcome: ActionOutcome::Applied,
        })
    );
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_play_from_after_fault_retains_explicit_cursor() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 32]).unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(45))
        .unwrap();
    let mut output = [[1.0; 2]; 16];
    #[cfg(feature = "allocation-probe")]
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 45);
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    assert_eq!(output, [[0.0; 2]; 16]);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn priority_stop_preserves_zero_cursor_over_post_fault_play_from() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    control.command(PlaybackCommand::PlayFrom(45)).unwrap();
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.recovery_position(), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn multiple_rejected_actions_are_each_observable_after_one_callback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let (playback, _, _, _) = session_audio_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    let first = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Stop)
        .unwrap();
    let second = control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::Seek(4))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.consumed_command.load(Acquire), second);
    assert_eq!(
        control.status.rejected_action(),
        Some((second, PlaybackActionRejection::InvalidSessionAction)),
    );
    for ticket in [first, second] {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::InvalidSessionAction),
            })
        );
    }
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn first_render_cause_is_latched_before_stop_and_new_admission_clears_it() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let (playback, _) = spectrum_eq8_graph();
    let revision = control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 128]).expect("prefix");
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        callback.report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
    });
    #[cfg(not(feature = "allocation-probe"))]
    callback.report_render_fault(Some(&Invalid("ongoing midi query exceeds 256 edges")), 128);
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0),
    );
    assert!(!control.status.playing.load(Relaxed));
    assert_eq!(control.status.position.load(Relaxed), 0);
    assert_eq!(control.status.recovery_position(), 128);
    let fault = control.status.render_fault_snapshot().expect("fault");
    assert_eq!(fault.message, "ongoing midi query exceeds 256 edges");
    assert_eq!((fault.audio_revision, fault.frame), (revision, 128));
    callback.report_render_fault(None, 0);
    assert_eq!(control.status.render_fault_snapshot(), Some(fault));
    assert_eq!(control.status.recovery_position(), 128);
    control.command(PlaybackCommand::Seek(37)).expect("seek");
    callback.begin_block();
    assert_eq!(control.status.position.load(Relaxed), 37);
    assert_eq!(control.status.recovery_position(), 37);
    callback.report_render_fault(None, 37);
    assert_eq!(control.status.recovery_position(), 37);
    control.stop();
    assert_eq!(control.status.recovery_position(), 0);
    control
        .command(PlaybackCommand::Seek(45))
        .expect("later seek");
    callback.begin_block();
    assert_eq!(control.status.recovery_position(), 45);
    let (replacement, _) = spectrum_eq8_graph();
    let replacement_revision = control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .expect("replacement");
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        replacement_revision
    );
    assert!(!control.status.render_fault.load(Acquire));
    assert!(control.status.render_fault_snapshot().is_none());
    assert_eq!(control.status.recovery_position(), 0);
    callback.report_render_fault(None, 256);
    assert_eq!(
        control
            .status
            .render_fault_snapshot()
            .expect("nonfinite")
            .message,
        "audio output contains nonfinite samples"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn device_fault_keeps_recovery_cursor_across_graph_admission() {
    let (mut control, mut callback) = channel(48_000).expect("channel");
    let (playback, _) = spectrum_eq8_graph();
    control.publish(playback).expect("publish");
    control.command(PlaybackCommand::Play).expect("play");
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 64]).expect("prefix");
    control.status.device_fault.store(true, Release);
    callback.stop_after_fault();
    assert_eq!(control.status.position.load(Relaxed), 0);
    assert_eq!(control.status.recovery_position(), 64);
    let (replacement, _) = spectrum_eq8_graph();
    control
        .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
        .expect("replacement");
    callback.begin_block();
    assert_eq!(control.status.recovery_position(), 64);
    control.stop();
    assert_eq!(control.status.recovery_position(), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn render_fault_fences_a_waiting_begin_before_a_future_graph_is_published() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, identity) = spectrum_eq8_graph();
    let first_revision = control
        .publish_with_policy_and_context_generation(playback, PublicationPolicy::InheritRuntime, 2)
        .unwrap();
    callback.begin_block();

    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    control
        .processor_audition
        .select(identity, 3, first_revision + 1, 48_000, selection)
        .unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 64];
    callback.render(&mut output).unwrap();
    assert_eq!(callback.processor_audition_lease, None);
    assert!(callback.processor_audition.current().is_some());

    callback.stop_after_fault();
    assert_eq!(callback.processor_audition.current(), None);
    let (replacement, _) = spectrum_eq8_graph();
    let second_revision = control
        .publish_with_policy_and_context_generation(
            replacement,
            PublicationPolicy::InstallFreshRuntime,
            3,
        )
        .unwrap();
    callback.begin_block();
    callback.render(&mut output).unwrap();

    assert_eq!(second_revision, first_revision + 1);
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .revision,
        second_revision
    );
    assert_eq!(callback.processor_audition_lease, None);
    assert_eq!(callback.processor_audition.current(), None);
    assert!(
        !callback
            .active
            .as_mut()
            .and_then(ActivePublication::legacy_mut)
            .unwrap()
            .playback
            .processor_audition_is_active(identity, selection)
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn playback_audition_routes_to_arrangement_session_runtime_and_return_owner() {
    let (mut playback, _) = spectrum_eq8_graph();
    let selection = hex_engine::ProcessorAuditionSelection { index: 0, lane: 0 };
    let return_identity = PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Return(3),
        12,
        hex_engine::DeviceTypeId::from_static("hex.eq8"),
    );
    assert!(playback.select_processor_audition(return_identity, Some(selection)));
    assert!(playback.processor_audition_is_active(return_identity, selection));
    let wrong_owner = PostNodeIdentity::new(
        hex_engine::ProcessorChainOwner::Track(3),
        12,
        hex_engine::DeviceTypeId::from_static("hex.eq8"),
    );
    assert!(!playback.select_processor_audition(wrong_owner, Some(selection)));
    playback.cancel_all_processor_auditions();
    assert!(!playback.processor_audition_is_active(return_identity, selection));
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

#[test]
fn spectrum_stage_requires_independent_exact_phase_coverage() {
    use hex_engine::NodeSignalPhase::{Input, Output};
    let target = spectrum_test_target(hex_engine::ProcessorChainOwner::Track(3), 0, 1, 48_000);
    let wrong_owner =
        spectrum_test_target(hex_engine::ProcessorChainOwner::Return(3), 0, 1, 48_000);
    let input = [[0.1, -0.2], [0.3, -0.4], [0.5, -0.6], [0.7, -0.8]];
    let output = [[0.2, -0.4], [0.6, -0.8], [1.0, -1.2], [1.4, -1.6]];
    let mut stage = SpectrumStage::default();
    stage.begin(target, 1, 4);
    stage.observe_signal(target.identity, Input, 0, &input);
    assert!(!stage.complete(), "input alone cannot publish");
    stage.observe_signal(target.identity, Output, 2, &output[2..]);
    stage.observe_signal(wrong_owner.identity, Output, 0, &output[..2]);
    assert!(
        !stage.complete(),
        "the other owner's samples cannot complete a pair"
    );
    stage.observe_signal(target.identity, Output, 0, &output[..2]);
    assert!(stage.complete());
    assert_eq!(stage.input_frames[..4], input);
    assert_eq!(stage.frames[..4], output);
    stage.clear();
    assert!(!stage.complete());

    for phase in [Input, Output] {
        for (offset, count) in [(1, 1), (4, 1), (usize::MAX, 1)] {
            stage.begin(target, 2, 4);
            stage.observe_signal(target.identity, Input, 0, &input);
            stage.observe_signal(target.identity, Output, 0, &output);
            assert!(stage.complete());
            stage.observe_signal(target.identity, phase, offset, &input[..count]);
            assert!(
                !stage.complete(),
                "overlap or out-of-bounds in either phase invalidates the pair"
            );
        }
    }
    stage.begin(target, 3, 257);
    assert!(!stage.complete());
}

#[test]
fn prepared_eq_captures_actual_input_and_processed_output_without_changing_pcm() {
    let (mut control, mut callback, mut reader, reaper) = spectrum_test_channel();
    let (playback, identity) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
    let (mut oracle, _) = spectrum_eq8_graph_with_params(true, 0, 12.0, 1.0);
    let revision = control.publish(playback).unwrap();
    control
        .spectrum_target
        .select(identity, 0, revision, 48_000)
        .unwrap();
    let mut actual = [[0.0; 2]; 256];
    let mut expected = actual;
    oracle.render(&mut expected).unwrap();
    let mut render = || {
        callback.begin_block();
        callback.render(&mut actual).unwrap();
    };
    #[cfg(feature = "allocation-probe")]
    {
        let measured = allocation_counter::measure(&mut render);
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
    render();
    assert_eq!(actual, expected);
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().input_frames,
        [[0.25, -0.4]; 256]
    );
    assert_eq!(callback.spectrum_stages.get(0).unwrap().frames, actual);
    assert!(actual.iter().any(|frame| (frame[0] - 0.25).abs() > 0.001));
    // Even a fully covered pair must be discarded if final caller validation fails.
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), false),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );
    assert!(!callback.spectrum_stages.get(0).unwrap().complete());
    assert!(reader.try_latest_pair().is_none());
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn existing_publication_methods_use_control_context_generation_zero() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(0)
    );
    control
        .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(0)
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn analyzer_waits_for_exact_publication_generation_and_activation_floor() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let generation_a = 41;
    let generation_b = 42;
    let (playback_a, identity) = spectrum_eq8_graph();
    let revision_a = control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            generation_a,
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );

    let target_a = spectrum_test_target(identity.owner, generation_a, revision_a, 48_000);
    control
        .spectrum_target
        .select(identity, generation_a, revision_a, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_a));

    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());

    let (playback_b, identity_b) = spectrum_eq8_graph();
    assert_eq!(identity_b, identity);
    let revision_b = control
        .publish_with_policy_and_context_generation(
            playback_b,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    assert!(revision_b > target_a.activation_floor);
    callback.begin_block();
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );

    // A project can switch after render but before the adapter submits its
    // block. The final admission check must reject the old project's tap.
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    let (target_b, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_b + 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    callback.render(&mut output).unwrap();
    assert!(
        !callback.spectrum_stages.get(0).unwrap().valid,
        "activation floor is not reached"
    );
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
    );

    let (playback_c, identity_c) = spectrum_eq8_graph();
    assert_eq!(identity_c, identity);
    let revision_c = control
        .publish_with_policy_and_context_generation(
            playback_c,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    assert_eq!(revision_c, revision_b + 1);
    callback.begin_block();
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
    ));

    // A later ordinary publication in the same project must keep the
    // selected token and its original lower-bound activation revision.
    let revision_d = control
        .publish_with_policy_and_context_generation(
            spectrum_eq8_graph().0,
            PublicationPolicy::InheritRuntime,
            generation_b,
        )
        .unwrap();
    assert!(revision_d > revision_c);
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    callback.render(&mut output).unwrap();
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 3 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 3 })
    ));

    let (target_wrong_rate, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_d, 44_100)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_wrong_rate));
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 4 })
    );
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn generation_context_survives_full_queue_retry_and_deferred_handoff() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let generation_a = 71;
    let generation_b = 72;
    let (playback_a, identity) = spectrum_eq8_graph();
    let revision_a = control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            generation_a,
        )
        .unwrap();
    let rejected = control
        .publish_with_policy_and_context_generation(
            spectrum_eq8_graph().0,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap_err();
    assert_eq!(rejected.reason, "playback publication already pending");
    assert_eq!(control.revision, revision_a);

    let (target_b, _) = control
        .spectrum_target
        .select(identity, generation_b, revision_a + 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );
    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    control
        .publish_with_policy_and_context_generation(
            rejected.playback,
            PublicationPolicy::InstallFreshRuntime,
            generation_b,
        )
        .unwrap();
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert_eq!(
        callback
            .deferred
            .as_ref()
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_a)
    );
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 1 })
    );

    callback.begin_block();
    assert!(callback.deferred.is_none());
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(generation_b)
    );
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 2 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 2 })
    ));
    shutdown_spectrum_test_channel(control, callback, reaper);
}

#[test]
fn selected_target_is_rejected_when_project_switches_at_block_boundary() {
    let (mut control, mut callback, _reader, reaper) = spectrum_test_channel();
    let (playback_a, identity) = spectrum_eq8_graph();
    control
        .publish_with_policy_and_context_generation(
            playback_a,
            PublicationPolicy::InstallFreshRuntime,
            0,
        )
        .unwrap();
    callback.begin_block();
    let (target_a, _) = control
        .spectrum_target
        .select(identity, 0, 1, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_a));

    let after_target_selection = Arc::new(Barrier::new(2));
    callback.test_after_spectrum_target_boundary = Some(Arc::clone(&after_target_selection));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        after_target_selection.wait();
        let (playback_b, identity_b) = spectrum_eq8_graph();
        assert_eq!(identity_b, identity);
        let revision_b = control
            .publish_with_policy_and_context_generation(
                playback_b,
                PublicationPolicy::InstallFreshRuntime,
                1,
            )
            .unwrap();
        after_target_selection.wait();
        callback_thread.join().unwrap();
        assert_eq!(revision_b, 2);
    });
    assert_eq!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .map(|publication| publication.control_context_generation),
        Some(1)
    );
    let mut output = [[0.0; 2]; 8];
    assert_eq!(callback.spectrum_target.current(), Some(target_a));
    assert_eq!(
        callback.spectrum_target.current(),
        control.spectrum_target.desired()
    );
    callback.render(&mut output).unwrap();
    assert!(!callback.spectrum_stages.get(0).unwrap().valid);
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_a)
    );
    assert_eq!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Skipped { sequence: 0 })
    );

    let (target_b, _) = control
        .spectrum_target
        .select(identity, 1, 2, 48_000)
        .unwrap();
    callback.begin_block();
    assert_eq!(callback.spectrum_target.current(), Some(target_b));
    assert_eq!(target_b.project_generation, 1);
    callback.render(&mut output).unwrap();
    assert!(callback.spectrum_stages.get(0).unwrap().complete());
    assert_eq!(
        callback.spectrum_stages.get(0).unwrap().target,
        Some(target_b)
    );
    assert!(matches!(
        callback.finish_spectrum_block(Instant::now(), true),
        Some(crate::spectrum_worker::CaptureOutcome::Queued { sequence: 1 })
            | Some(crate::spectrum_worker::CaptureOutcome::QueueFull { sequence: 1 })
    ));
    shutdown_spectrum_test_channel(control, callback, reaper);
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

#[test]
fn running_transport_fence_rejects_stale_and_pending_command_observations() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    control
        .publish_with_policy_and_context_generation(
            session_playback(1),
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    callback.begin_block();
    assert!(control.capture_running_transport_fence().is_none());
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    let (observed, fence) = control.capture_running_transport_fence().unwrap();
    assert_eq!(observed.consumed_command_ticket, play);
    assert_eq!(fence.admitted_command_ticket(), play);

    let seek = control
        .command_with_ticket(PlaybackCommand::Seek(0))
        .unwrap();
    assert!(control.capture_running_transport_fence().is_none());
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();
    let current = control.status.transport_observation().unwrap();
    assert_eq!(current.consumed_command_ticket, seek);
    control.status.transport_observation.publish(Some(observed));
    assert!(control.capture_running_transport_fence().is_none());
    control.status.transport_observation.publish(Some(current));
    assert!(control.capture_running_transport_fence().is_some());

    control
        .status
        .transport_observation
        .publish(Some(TransportObservation {
            audio_revision: current.audio_revision + 1,
            ..current
        }));
    assert!(control.capture_running_transport_fence().is_none());
    control
        .status
        .transport_observation
        .publish(Some(TransportObservation {
            control_generation: 0,
            ..current
        }));
    assert!(control.capture_running_transport_fence().is_none());
    control.status.transport_observation.publish(Some(current));
    control.status.device_fault.store(true, Release);
    assert!(control.capture_running_transport_fence().is_none());
    control.status.device_fault.store(false, Release);
    assert!(control.capture_running_transport_fence().is_some());
}

#[test]
fn fresh_runtime_fence_accepts_empty_and_exact_active_snapshots() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let empty = control.capture_fresh_runtime_fence().unwrap();
    assert_eq!(empty.expected_active_revision(), 0);
    assert_eq!(empty.route_state_generation(), 0);
    assert_eq!(empty.admitted_command_ticket(), 0);
    let first_revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(empty),
        )
        .unwrap();
    assert_eq!(control.status.admitted_ticket.load(Acquire), 0);
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        first_revision
    );

    let current = control.capture_fresh_runtime_fence().unwrap();
    let second_revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(current),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        second_revision
    );
    assert_eq!(control.status.rejected_revision(), None);
    assert!(!control.status.publication_pending.load(Acquire));
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn cancelled_fresh_publication_returns_candidate_and_allows_successor() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let active_revision = control.status.applied_revision.load(Acquire);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let candidate_revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();

    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision + 1),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision),
        FreshRuntimeCancellationResult::Cancelled
    );
    callback.begin_block();
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        active_revision
    );
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            candidate_revision,
            PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    assert!(!control.status.publication_pending.load(Acquire));
    let mut output = [[0.0; 2]; 1];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]], "the old runtime remains active");
    assert_eq!(control.reclaim(), 1, "the candidate retires to control");

    let successor = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), successor);
    assert_eq!(
        control.cancel_fresh_runtime_publication(candidate_revision),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    assert_eq!(
        control.reclaim(),
        1,
        "the replaced graph retires to control"
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn cancellation_and_final_apply_cas_have_one_winner() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let active_revision = control.status.applied_revision.load(Acquire);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled_revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let cancelled = control.fresh_runtime_receipt(cancelled_revision).unwrap();
    let before_apply = Arc::new(Barrier::new(2));
    callback.test_before_fresh_runtime_apply = Some(Arc::clone(&before_apply));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        before_apply.wait();
        assert_eq!(
            control.cancel_fresh_runtime_publication(cancelled_revision),
            FreshRuntimeCancellationResult::Cancelled
        );
        assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
        control.stop();
        before_apply.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        active_revision
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_revision(),
        Some((
            cancelled_revision,
            PlaybackHandoffRejection::FreshRuntimeCancelled
        ))
    );
    assert_eq!(control.reclaim(), 1);

    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let applying_revision = control
        .publish_with_policy(
            graph_with_frame([0.5, -0.4]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let installing = control.fresh_runtime_receipt(applying_revision).unwrap();
    let after_apply_decision = Arc::new(Barrier::new(2));
    callback.test_after_fresh_runtime_apply_decision = Some(Arc::clone(&after_apply_decision));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        after_apply_decision.wait();
        assert_eq!(
            control.cancel_fresh_runtime_publication(applying_revision),
            FreshRuntimeCancellationResult::Applying
        );
        assert_eq!(installing.state(), PublicationReceiptState::Applying);
        control.stop();
        after_apply_decision.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(
        control.status.applied_revision.load(Acquire),
        applying_revision
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_revision().unwrap().0,
        cancelled_revision
    );
    assert_eq!(
        control.cancel_fresh_runtime_publication(applying_revision),
        FreshRuntimeCancellationResult::Complete
    );
    assert_eq!(installing.state(), PublicationReceiptState::Installed);
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn publication_receipts_keep_exact_terminals_after_replacement_and_shutdown() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let output = control.output_identity().unwrap();
    let context = hex_engine::AudioSourceContext {
        project_generation: 4,
        editor_revision: 6,
    };
    let submit = |control: &mut Control, fence| {
        let revision = control
            .publish_unaccounted_pcm_obligation(
                graph().0,
                PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
                context.project_generation,
                true,
                false,
                Some(context),
            )
            .unwrap();
        let receipt = control.fresh_runtime_receipt(revision).unwrap();
        assert_eq!(receipt.output_identity(), output);
        assert_eq!(receipt.context(), Some(context));
        assert_eq!(receipt.revision(), revision);
        assert_eq!(receipt.state(), PublicationReceiptState::Pending);
        receipt
    };
    let begin = |callback: &mut Callback| {
        #[cfg(feature = "allocation-probe")]
        {
            let counts = allocation_counter::measure(|| callback.begin_block());
            assert_eq!(
                (
                    counts.count_total,
                    counts.bytes_total,
                    counts.count_current,
                    counts.bytes_current
                ),
                (0, 0, 0, 0),
            );
        }
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
    };

    let old_fence = control.capture_fresh_runtime_fence().unwrap();
    let installed = submit(&mut control, old_fence);
    begin(&mut callback);
    control.reclaim();
    // Both later candidates have the same context. Their revisions and
    // terminals must stay distinct from the installed receipt.
    let refused = submit(&mut control, old_fence);
    begin(&mut callback);
    control.reclaim();
    assert_eq!(
        refused.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
    );
    let current_fence = control.capture_fresh_runtime_fence().unwrap();
    let cancelled = submit(&mut control, current_fence);
    assert_eq!(
        cancelled.cancel(),
        FreshRuntimeCancellationResult::Cancelled
    );
    begin(&mut callback);
    control.reclaim();
    assert_eq!(
        control.status.rejected_revision().unwrap().0,
        cancelled.revision()
    );
    let pending = submit(&mut control, current_fence);
    control.request_shutdown();
    begin(&mut callback);
    control.reclaim();
    assert_eq!(pending.state(), PublicationReceiptState::OutputClosed);
    assert_eq!(installed.state(), PublicationReceiptState::Installed);
    assert_eq!(
        refused.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::RuntimeFenceChanged)
    );
    assert_eq!(cancelled.state(), PublicationReceiptState::Cancelled);
}

#[test]
fn destroying_output_without_another_callback_closes_its_pending_receipt() {
    let (mut control, callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let receipt = control.fresh_runtime_receipt(revision).unwrap();
    assert_eq!(receipt.state(), PublicationReceiptState::Pending);
    drop(control);
    drop(callback);
    assert_eq!(receipt.state(), PublicationReceiptState::OutputClosed);
}

#[test]
fn failed_enqueue_does_not_leave_a_cancellable_revision_or_block_next_publish() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publications
        .push(QueuedPublication::Legacy(Publication {
            pcm_frames: 0,
            forwarded_pcm: false,
            revision: 99,
            control_context_generation: 0,
            explicit_control_context: false,
            source_context: None,
            policy: PublicationPolicy::InstallFreshRuntime,
            fresh_runtime_cancellation: None,
            scheduled_cut_frame: None,
            streams: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_references: None,
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            native_continuation: None,
            playback: Box::new(graph().0),
        }))
        .unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap_err();
    assert_eq!(rejected.reason, "playback publication queue full");
    assert_eq!(control.revision, 0);
    assert_eq!(
        control.cancel_fresh_runtime_publication(1),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    let queued = callback.publications.pop().unwrap();
    drop(queued);
    assert_eq!(control.publish(rejected.playback).unwrap(), 1);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn ordinary_inherited_publications_have_no_fresh_cancellation_decision() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let first = control.publish(graph().0).unwrap();
    callback.begin_block();
    let next = control.publish(graph_with_frame([0.5, -0.4]).0).unwrap();
    assert_eq!(
        control.cancel_fresh_runtime_publication(next),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), next);
    assert_eq!(
        control.cancel_fresh_runtime_publication(first),
        FreshRuntimeCancellationResult::UnknownRevision
    );
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fault_invalidates_prepared_playing_runtime_fence() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (mut prepared, _) = fresh_spectrum_eq8_graph();
    prepared.apply(PlaybackCommand::Play);
    let revision = control
        .publish_with_policy(
            prepared,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    callback.begin_block();
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    assert!(!control.status.playing.load(Acquire));
}

#[test]
fn latched_fault_refuses_prepared_runtime_even_after_epoch_exhaustion() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control.status.transport_epoch.store(u64::MAX, Release);
    control
        .status
        .last_output_fault_epoch
        .store(u64::MAX, Release);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (mut prepared, _) = fresh_spectrum_eq8_graph();
    prepared.apply(PlaybackCommand::Play);
    let revision = control
        .publish_with_policy(
            prepared,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 32);
    callback.begin_block();
    assert_eq!(
        control.status.rejected_revision(),
        Some((revision, PlaybackHandoffRejection::RuntimeFenceChanged))
    );
    assert!(!control.status.playing.load(Acquire));
}

#[test]
fn fresh_runtime_fence_rejects_stale_active_revision() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let stale = control.capture_fresh_runtime_fence().unwrap();
    let intervening = control
        .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), intervening);

    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), intervening);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 2);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stale_route_generation_independently() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, _, clip, _) = session_audio_graph();
    let (mut oracle, _, oracle_clip, _) = session_audio_graph();
    assert_eq!(clip, oracle_clip);
    control
        .publish(old)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    control
        .launch_and_play_with_ticket(hex_engine::SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: hex_engine::LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    oracle
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: hex_engine::LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    oracle.apply(PlaybackCommand::Play);

    let stale = control.capture_fresh_runtime_fence().unwrap();
    let ticket_before = control.status.admitted_ticket.load(Acquire);
    callback
        .active
        .as_mut()
        .and_then(ActivePublication::legacy_mut)
        .unwrap()
        .playback
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Seek(1),
        ))
        .unwrap();
    oracle
        .try_apply_action(hex_engine::PlaybackAction::Session(
            hex_engine::SessionPlaybackCommand::Seek(1),
        ))
        .unwrap();
    callback.publish_active_routes(true);
    assert_eq!(control.status.admitted_ticket.load(Acquire), ticket_before);
    assert_ne!(
        callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .session_route_state_generation(),
        stale.route_state_generation()
    );

    let (candidate, _) = graph_with_frame([0.91, -0.83]);
    let rejected = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_fresh_fence_rejection(&mut control, rejected);
    let mut actual = [[0.0; 2]; 2];
    let mut expected = [[0.0; 2]; 2];
    callback.render(&mut actual).unwrap();
    oracle.render(&mut expected).unwrap();
    assert_eq!(
        actual, expected,
        "stale fresh candidate replaced active PCM"
    );
    assert_ne!(actual[0], [0.91, -0.83]);
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        },
        "stale rejection must preserve the active Session route owner"
    );
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stale_command_ticket_and_applies_prefix_to_old_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let stale = control.capture_fresh_runtime_fence().unwrap();
    let ticket = control
        .command_with_ticket(PlaybackCommand::Seek(19))
        .unwrap();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert_eq!(control.status.position.load(Acquire), 19);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_pending_stop_and_stops_the_old_graph() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    let stale = control.capture_fresh_runtime_fence().unwrap();
    control.stop();
    let rejected = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(stale),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_fence_rejects_stop_consumed_before_publication() {
    assert_consumed_stop_invalidates_fresh_fence(false);
}

#[test]
fn resident_fresh_runtime_fence_rejects_stop_consumed_before_publication() {
    assert_consumed_stop_invalidates_fresh_fence(true);
}

#[test]
fn fresh_fence_refuses_stop_between_marker_clear_and_transport_ack() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(session_audio_graph().0).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    control.stop();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let worker = scope.spawn(|| callback.begin_block());
        barrier.wait();
        assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
        assert!(control.capture_fresh_runtime_fence().is_none());
        barrier.wait();
        worker.join().unwrap();
    });
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control
            .capture_fresh_runtime_fence()
            .unwrap()
            .consumed_stop_generation(),
        2
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
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

#[test]
fn exhausted_stop_generation_refuses_new_fences_without_blocking_stop() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control
        .status
        .consumed_stop_generation
        .store(u64::MAX - 1, Release);
    control.stop();
    callback.begin_block();
    assert_eq!(
        control.status.consumed_stop_generation.load(Acquire),
        u64::MAX
    );
    assert!(control.capture_fresh_runtime_fence().is_none());
    control.stop();
    callback.begin_block();
    assert_eq!(
        control.status.consumed_stop_generation.load(Acquire),
        u64::MAX
    );
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn command_admitted_after_fresh_fence_validation_applies_to_new_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish(graph().0)
        .unwrap_or_else(|error| panic!("{}", error.reason));
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let barrier = std::sync::Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let ticket = control
            .command_with_ticket(PlaybackCommand::Seek(47))
            .unwrap();
        barrier.wait();
        callback_thread.join().unwrap();
        assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    });
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert_eq!(control.status.position.load(Acquire), 47);
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
#[cfg(feature = "allocation-probe")]
fn bound_timing_only_capture_tracks_loop_spans_and_output_confirmation() {
    use crate::sample_recording::{OutputClock, OutputClockIndex};
    use hex_project::{Command, Editor, Project};
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    editor
        .execute(Command::AddTrack {
            name: "input".into(),
        })
        .unwrap();
    editor
        .execute(Command::SetLoopRegion {
            start: 32,
            end: 160,
            enabled: true,
        })
        .unwrap();
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &std::collections::BTreeMap::new(),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    let playback = Playback::try_from_prepared_with_session(revision).unwrap();
    let (mut control, mut callback) = channel(48_000).unwrap();
    control
        .publish_unaccounted_pcm_obligation(
            playback,
            PublicationPolicy::InstallFreshRuntime,
            7,
            true,
            false,
            Some(hex_engine::AudioSourceContext {
                project_generation: 7,
                editor_revision: 3,
            }),
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let source = callback.recording_source().unwrap();
    let mut reader = control.sample_recording.start_bound(source).unwrap();
    assert!(reader.sources().is_empty());
    let mut index = OutputClockIndex::new(&reader);
    assert!(reader.ready());
    for block in 0..4 {
        let mut output = [[0.0; 2]; 64];
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
            callback.set_sample_recording_output_clock(Some(OutputClock {
                playback: cpal::StreamInstant::new(10, 0),
                offset_frames: block * 64,
                graph_latency_frames: None,
                origin: crate::sample_recording::OutputClockOrigin::Unknown,
            }));
            callback.finish_sample_recording(true);
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
        assert_eq!(index.poll(&mut reader), Ok(None));
    }
    let starts: Vec<_> = (0..index.len())
        .map(|n| {
            index
                .packet(n)
                .unwrap()
                .timing
                .iter()
                .map(|span| (span.take_start, span.take_end, span.timeline_start))
                .collect::<Vec<_>>()
        })
        .collect();
    assert_eq!(
        starts,
        vec![
            vec![(0, 64, 0)],
            vec![(0, 64, 64)],
            vec![(0, 32, 128), (32, 64, 32)],
            vec![(0, 64, 64)]
        ]
    );
    control.sample_recording.stop();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    assert_eq!(
        index.poll(&mut reader),
        Ok(Some(crate::sample_recording::End::Stopped))
    );
}

#[test]
#[cfg(feature = "allocation-probe")]
fn recorded_audible_clock_accounts_for_actual_main_limiter_delay() {
    use crate::input_capture::{
        InputConfig, InputTake, InputTakeEnd, InputTiming, SharedClock, align_audible_take,
    };
    use crate::sample_recording::{End, OutputClock, OutputClockIndex};
    use hex_engine::AudioSourceContext;
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor, Project};
    use std::collections::BTreeMap;

    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Imported { track, asset, clip } = editor
        .execute(Command::ImportPreparedAudioTrack {
            name: "transient".into(),
            path: "memory/transient.wav".into(),
            source_sample_rate: 48_000,
            source_frames: 4_096,
            playback_frames: 4_096,
            conversion_version: hex_project::IDENTITY_CONVERSION,
        })
        .unwrap()
    else {
        panic!("import")
    };
    editor
        .execute(Command::MoveClip {
            clip,
            track,
            start: 4_800,
        })
        .unwrap();
    let definition = &hex_devices::limiter::DEFINITION;
    let state = definition
        .patch_parameter(
            &definition.default_state().unwrap(),
            hex_devices::limiter::parameter::LOOKAHEAD,
            ParameterValue::Choice(2),
        )
        .unwrap();
    editor
        .execute(Command::InsertMainDevice {
            before: None,
            state,
        })
        .unwrap();
    let mut pcm = vec![[0.0; 2]; 4_096];
    pcm[0] = [0.25; 2];
    let revision = Arc::new(
        hex_engine::PreparedRevision::prepare(
            editor.project(),
            &BTreeMap::from([(asset, hex_engine::Audio::new(48_000, pcm).unwrap())]),
            Arc::new(hex_engine::SineTable::new()),
            None,
        )
        .unwrap(),
    );
    assert_eq!(revision.arrangement().main_latency_frames(), 288);

    for block_size in [63, 256] {
        let playback = Playback::try_from_prepared_with_session(Arc::clone(&revision)).unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control
            .publish_unaccounted_pcm_obligation(
                playback,
                PublicationPolicy::InstallFreshRuntime,
                7,
                true,
                false,
                Some(AudioSourceContext {
                    project_generation: 7,
                    editor_revision: 3,
                }),
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        let source = callback.recording_source().unwrap();
        let mut reader = control.sample_recording.start_bound(source).unwrap();
        assert!(
            reader.sources().is_empty(),
            "source-bound capture retains timing only"
        );
        let mut index = OutputClockIndex::new(&reader);
        assert!(reader.ready());
        let origin = cpal::StreamInstant::new(10, 0);
        let mut output = vec![[0.0; 2]; 6_000];
        for (number, block) in output.chunks_mut(block_size).enumerate() {
            let measured = allocation_counter::measure(|| {
                callback.begin_block();
                callback.render(block).unwrap();
                callback.set_sample_recording_output_clock(Some(OutputClock {
                    playback: origin,
                    offset_frames: (number * block_size) as u32,
                    graph_latency_frames: None,
                    origin: crate::sample_recording::OutputClockOrigin::Unknown,
                }));
                callback.finish_sample_recording(true);
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
            assert_eq!(index.poll(&mut reader), Ok(None));
        }
        assert!(output[5_088][0] > 0.01);
        control.sample_recording.stop();
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        assert_eq!(index.poll(&mut reader), Ok(Some(End::Stopped)));
        let take = InputTake {
            path: std::path::PathBuf::new(),
            input_device: format!("{}:test-input", cpal::default_host().id())
                .parse()
                .unwrap(),
            input_config: InputConfig {
                sample_rate: 48_000,
                channels: 1,
            },
            sample_rate: 48_000,
            frames: 16,
            preroll_frames: 0,
            timing: vec![InputTiming {
                take_start: 0,
                frames: 16,
                captured_at: origin,
                offset_frames: 5_088,
            }],
            end: InputTakeEnd::Stopped,
            open_error: None,
        };
        assert_eq!(
            align_audible_take(SharedClock, &take, &index)
                .unwrap()
                .timeline_start,
            4_800
        );
        assert_eq!(
            index.packet(0).unwrap().clock.graph_latency_frames,
            Some(288)
        );
        control.sample_recording.reclaim();
        let mut seek_take = control.sample_recording.start_bound(source).unwrap();
        assert!(seek_take.ready());
        callback.begin_block();
        callback.render(&mut [[0.0; 2]; 1]).unwrap();
        callback.finish_sample_recording(true);
        let position = control.status.position.load(Acquire);
        control.command(PlaybackCommand::Seek(position)).unwrap();
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut [[0.0; 2]; 1]).unwrap();
            callback.finish_sample_recording(true);
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
        assert_eq!(seek_take.drain(|_, _, _| {}), Some(End::TransportChanged));
    }
}

#[test]
#[cfg(feature = "allocation-probe")]
fn bound_output_recording_rejects_replaced_source() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use crate::sample_recording::{End, OutputClock, OutputClockIndex};
    use hex_engine::AudioSourceContext;

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let context = AudioSourceContext {
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
            Some(context),
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
    let wrong = AppliedAudioSource {
        audio_revision: source.audio_revision + 1,
        ..source
    };
    assert!(client.start_bound_output_recording(wrong).is_err());
    let mut take = client.start_bound_output_recording(source).unwrap();
    let mut index = OutputClockIndex::new(&take);
    assert_eq!(index.source_binding(), Some(source));
    assert!(take.ready());
    let mut output = [[0.0; 2]; 8];
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
            c.set_sample_recording_output_clock(Some(OutputClock {
                playback: cpal::StreamInstant::new(10, 0),
                offset_frames: 0,
                graph_latency_frames: Some(0),
                origin: crate::sample_recording::OutputClockOrigin::Unknown,
            }));
            c.finish_sample_recording(true);
        });
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
    assert_eq!(index.poll(&mut take), Ok(None));
    assert_eq!(index.len(), 1);

    // Hold a rendered packet until after the next source has been applied.
    data.owner_mut().unwrap().run(|c, _| {
        c.begin_block();
        c.render(&mut output).unwrap();
        c.set_sample_recording_output_clock(Some(OutputClock {
            playback: cpal::StreamInstant::new(11, 0),
            offset_frames: 0,
            graph_latency_frames: Some(0),
            origin: crate::sample_recording::OutputClockOrigin::Unknown,
        }));
    });
    let control = publication.control().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InheritRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.finish_sample_recording(true);
        });
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
    assert_eq!(index.poll(&mut take), Ok(Some(End::SourceChanged)));
    assert_eq!(index.len(), 1);
    assert!(client.start_bound_output_recording(source).is_err());
    client.reclaim_sample_recording();
    let current = client.status().unwrap().recording_source().unwrap();
    let mut next = client.start_bound_output_recording(current).unwrap();
    assert!(next.ready());
    let control = publication.control().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InheritRuntime,
            0,
            true,
            false,
            Some(context),
        )
        .unwrap();
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
            c.finish_sample_recording(true);
        });
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
    assert_eq!(
        next.drain(|_, _, _| panic!("changed source published a packet")),
        Some(End::SourceChanged)
    );
    client.close();
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

#[test]
#[cfg(feature = "allocation-probe")]
fn source_bound_seek_preserves_playback_and_rejects_replaced_source() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use hex_engine::AudioSourceContext;

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let context = AudioSourceContext {
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
            Some(context),
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
            .seek_from_source_with_ticket(1, wrong_output)
            .is_err()
    );
    let wrong_context = AppliedAudioSource {
        context: AudioSourceContext {
            project_generation: 1,
            ..context
        },
        ..source
    };
    assert!(
        client
            .seek_from_source_with_ticket(1, wrong_context)
            .is_err()
    );

    let ticket = client.seek_from_source_with_ticket(17, source).unwrap();
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
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 17);

    let start = client.play_from_source_with_ticket(17, source).unwrap();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: start,
            outcome: ActionOutcome::Applied
        })
    );
    let ticket = client.seek_from_source_with_ticket(29, source).unwrap();
    let mut output = [[0.0; 2]; 129];
    // Running transport seeks complete after the 128-frame declick fade.
    let measured = allocation_counter::measure(|| {
        data.owner_mut().unwrap().run(|c, _| {
            c.begin_block();
            c.render(&mut output).unwrap();
        })
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
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket,
            outcome: ActionOutcome::Applied
        })
    );
    assert!(client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 30);
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());

    let control = publication.control().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            graph,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            0,
            true,
            false,
            Some(context),
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
        let ticket = client.seek_from_source_with_ticket(47, source).unwrap();
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
    assert_ne!(client.status().unwrap().position.load(Acquire), 47);
    assert!(client.seek_from_source_with_ticket(47, source).is_err());
    let current = client.status().unwrap().recording_source().unwrap();
    for _ in 0..COMMANDS {
        client.seek_from_source_with_ticket(2, current).unwrap();
    }
    assert!(client.seek_from_source_with_ticket(2, current).is_err());
    client.stop();
    data.owner_mut().unwrap().run(|c, _| c.begin_block());
    assert!(!client.status().unwrap().playing.load(Acquire));
    client.close();
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

#[test]
#[cfg(feature = "allocation-probe")]
fn source_bound_start_rejects_same_project_replacement_and_preserves_stop() {
    for note_chase in [hex_engine::NoteChase::On, hex_engine::NoteChase::Off] {
        source_bound_start_fences(note_chase);
    }
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

#[test]
#[cfg(feature = "allocation-probe")]
fn document_bound_client_rejects_commands_racing_a_successor_graph() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    use hex_engine::{AudioSourceContext, LaunchQuantization, SessionPlaybackCommand};

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let old = AudioSourceContext {
        project_generation: 0,
        editor_revision: 4,
    };
    let (old_graph, _, clip, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            old_graph,
            PublicationPolicy::InstallFreshRuntime,
            old.project_generation,
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
    client.bind_resident_transport(old.project_generation);

    let control = publication.control().unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let new = AudioSourceContext {
        project_generation: 2,
        editor_revision: 1,
    };
    let (new_graph, _, new_clip, _) = session_audio_graph();
    assert_eq!(clip, new_clip);
    control
        .publish_unaccounted_pcm_obligation(
            new_graph,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
            2,
            true,
            false,
            Some(new),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    data.owner_mut().unwrap().run(|callback, _| {
        callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    });
    let actions = [
        PlaybackAction::Transport(PlaybackCommand::PlayFrom(47)),
        PlaybackAction::Transport(PlaybackCommand::Seek(61)),
        PlaybackAction::LaunchAndPlay(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        }),
    ];
    let tickets = std::thread::scope(|scope| {
        let callback = scope.spawn(|| {
            allocation_counter::measure(|| {
                data.owner_mut()
                    .unwrap()
                    .run(|callback, _| callback.begin_block());
            })
        });
        barrier.wait();
        let tickets = actions.map(|action| client.action_with_ticket(action));
        barrier.wait();
        let count = callback.join().unwrap();
        assert_eq!(
            (
                count.count_total,
                count.bytes_total,
                count.count_current,
                count.bytes_current
            ),
            (0, 0, 0, 0)
        );
        tickets.map(Result::unwrap)
    });
    for ticket in tickets {
        assert_eq!(
            reader.try_take(),
            Some(ActionOutcomeReceipt {
                ticket,
                outcome: ActionOutcome::Rejected(PlaybackActionRejection::ProjectChanged),
            })
        );
    }
    let status = client.status().unwrap();
    assert_eq!(
        status.rejected_action(),
        Some((tickets[2], PlaybackActionRejection::ProjectChanged))
    );
    assert_eq!(status.recording_source().unwrap().context, new);
    assert_eq!(status.position.load(Acquire), 0);
    assert!(!status.playing.load(Acquire));
    assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());

    client.bind_resident_transport(new.project_generation);
    let control = publication.control().unwrap();
    assert_eq!(control.reclaim(), 1);
    let edited = AudioSourceContext {
        editor_revision: 2,
        ..new
    };
    let (edited_graph, _, _, _) = session_audio_graph();
    control
        .publish_unaccounted_pcm_obligation(
            edited_graph,
            PublicationPolicy::InstallFreshRuntime,
            2,
            true,
            false,
            Some(edited),
        )
        .unwrap();
    let next = client
        .command_with_ticket(PlaybackCommand::Seek(29))
        .unwrap();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        reader.try_take(),
        Some(ActionOutcomeReceipt {
            ticket: next,
            outcome: ActionOutcome::Applied
        })
    );
    assert_eq!(
        client.status().unwrap().recording_source().unwrap().context,
        edited
    );
    assert_eq!(client.status().unwrap().position.load(Acquire), 29);
    for _ in 0..COMMANDS {
        client.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    assert!(client.command_with_ticket(PlaybackCommand::Play).is_err());
    client.stop();
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert!(!client.status().unwrap().playing.load(Acquire));
    assert_eq!(client.status().unwrap().position.load(Acquire), 0);

    client.close();
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

#[test]
fn stop_admitted_after_fresh_fence_validation_stops_new_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap_or_else(|error| panic!("{}", error.reason));
    let barrier = std::sync::Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let callback_thread = scope.spawn(|| callback.begin_block());
        barrier.wait();
        control.stop();
        barrier.wait();
        callback_thread.join().unwrap();
    });
    assert_eq!(control.status.applied_revision.load(Acquire), revision);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    assert_eq!(control.status.rejected_revision(), None);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn stale_fresh_runtime_fence_is_rechecked_after_retirement_backpressure() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    callback.begin_block();
    for _ in 0..RETIRED {
        control
            .publish_with_policy(graph().0, PublicationPolicy::InstallFreshRuntime)
            .unwrap();
        callback.begin_block();
    }
    assert!(callback.retired.is_full());
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph_with_frame([0.91, -0.83]).0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control
        .command_with_ticket(PlaybackCommand::Seek(13))
        .unwrap();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    // A Seek while playing fades out, jumps and ramps in (#4140).
    assert_eq!(control.status.position.load(Acquire), 0);
    assert!(control.status.publication_pending.load(Acquire));
    let mut fade_out = [[0.0; 2]; 128];
    callback.render(&mut fade_out).unwrap();
    assert_eq!(control.status.position.load(Acquire), 13);
    let ramp = |frame: usize| [0.25 * (frame + 1) as f32 / 128.0; 2];
    let mut before_retry = [[0.0; 2]; 8];
    callback.render(&mut before_retry).unwrap();
    assert_eq!(before_retry, std::array::from_fn(ramp));
    assert_eq!(control.status.position.load(Acquire), 21);
    assert_eq!(control.reclaim(), RETIRED);
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), revision - 1);
    assert_fresh_fence_rejection(&mut control, revision);
    let mut after_retry = [[0.0; 2]; 8];
    callback.render(&mut after_retry).unwrap();
    assert_eq!(after_retry, std::array::from_fn(|frame| ramp(frame + 8)));
    assert_eq!(control.status.position.load(Acquire), 29);
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn rejected_candidate_is_held_until_control_reclaims_retirement_space() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    for revision in 10..10 + RETIRED as u64 {
        callback
            .retired
            .push(ReturnedPublication::Legacy(Publication {
                pcm_frames: 0,
                forwarded_pcm: false,
                revision,
                control_context_generation: 0,
                explicit_control_context: false,
                source_context: None,
                policy: PublicationPolicy::InstallFreshRuntime,
                fresh_runtime_cancellation: None,
                scheduled_cut_frame: None,
                streams: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_references: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_continuation: None,
                playback: Box::new(graph().0),
            }))
            .unwrap();
    }
    assert!(callback.retired.is_full());

    let fence = control.capture_fresh_runtime_fence().unwrap();
    let (candidate, weak_candidate, _, _) = session_audio_graph();
    let rejected = control
        .publish_with_policy(
            candidate,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    control.stop();
    callback.begin_block();

    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_fresh_fence_rejection(&mut control, rejected);
    assert!(callback.held_retirement.is_some());
    assert!(weak_candidate.upgrade().is_some());

    assert_eq!(control.reclaim(), RETIRED);
    callback.begin_block();
    assert!(callback.held_retirement.is_none());
    assert!(weak_candidate.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(weak_candidate.upgrade().is_none());

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn command_cut_distinguishes_captured_prefix_and_stop_boundary() {
    assert_eq!(
        command_cut(7, 4),
        CommandCut {
            stop_through: Some(3),
            process_through: 7,
        }
    );
    assert_eq!(
        command_cut(2, 8),
        CommandCut {
            stop_through: Some(7),
            process_through: 7,
        }
    );
    assert_eq!(
        command_cut(7, 0),
        CommandCut {
            stop_through: None,
            process_through: 7,
        }
    );
}

#[test]
fn stop_cut_suppresses_tickets_at_or_before_boundary_only() {
    for (stop_through, process_through, expected_position) in [
        (Some(2), 1, 0),
        (Some(2), 2, 0),
        (Some(2), 3, 33),
        (None, 3, 33),
    ] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        assert!(control.publish(graph().0).is_ok());
        callback.begin_block();
        for frame in [11, 22, 33] {
            control.command(PlaybackCommand::Seek(frame)).unwrap();
        }
        callback.consume_commands_through(CommandCut {
            stop_through,
            process_through,
        });
        assert_eq!(control.status.position.load(Acquire), expected_position);
        assert_eq!(
            control.status.consumed_command.load(Acquire),
            process_through
        );
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn full_queue_does_not_advance_admitted_ticket_and_stop_never_wraps() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    for expected in 1..=COMMANDS as u64 {
        assert_eq!(
            control
                .command_with_ticket(PlaybackCommand::Seek(expected))
                .unwrap(),
            expected
        );
    }
    assert!(
        control
            .command_with_ticket(PlaybackCommand::Seek(99))
            .is_err()
    );
    assert_eq!(
        control.status.admitted_ticket.load(Acquire),
        COMMANDS as u64
    );
    callback.begin_block();
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        COMMANDS as u64
    );

    control.command_ticket = u64::MAX - 1;
    control.status.admitted_ticket.store(u64::MAX - 1, Release);
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    control.stop();
    assert_eq!(control.status.pending_stop_marker.load(Acquire), u64::MAX);
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.pending_stop_marker.load(Acquire), 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn play_from_uses_one_ticket_and_priority_stop_cuts_it_off_under_full_queue() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    callback.begin_block();

    let ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(37))
        .unwrap();
    assert_eq!(ticket, 1, "seek and start are admitted as one command");
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 37);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);

    control.command(PlaybackCommand::Pause).unwrap();
    callback.begin_block();
    for frame in 1..COMMANDS as u64 {
        control.command(PlaybackCommand::Seek(frame)).unwrap();
    }
    let play_from_ticket = control
        .command_with_ticket(PlaybackCommand::PlayFrom(91))
        .unwrap();
    assert_eq!(play_from_ticket, COMMANDS as u64 + 2);
    assert!(control.command(PlaybackCommand::Play).is_err());
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.position.load(Acquire), 0);
    assert_eq!(
        control.status.consumed_command.load(Acquire),
        play_from_ticket,
        "priority Stop consumes the queued prefix while suppressing playback"
    );

    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn post_cut_admissions_remain_queued_until_the_next_actual_block() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));

    // Capture the actual callback cut and marker, then admit Stop twice
    // and Play after that cut. Both Stops coalesce to the same cutoff.
    let entry_ticket = control.status.admitted_ticket.load(Acquire);
    let marker = control.status.pending_stop_marker.swap(0, AcqRel);
    let cut = command_cut(entry_ticket, marker);
    assert_eq!(cut.stop_through, None);
    control.stop();
    control.stop();
    let post_cut_play = control.command_with_ticket(PlaybackCommand::Play).unwrap();

    // Run the same captured cut production uses after the swap. The later
    // Play remains queued and the post-entry Stop marker is still pending.
    callback.consume_commands_through(cut);
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), 0);
    assert_eq!(
        control.status.pending_stop_marker.load(Acquire),
        post_cut_play
    );
    assert_eq!(callback.commands.peek().unwrap().ticket, post_cut_play);

    // The next real block captures Stop + Play and applies them in order.
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), post_cut_play);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn derived_pcm_render_replace_and_retire_never_allocate_or_drop_on_callback() {
    fn transformed(cents: i16) -> (Playback, Weak<hex_engine::PreparedRevision>) {
        use hex_project::{Command, Created, Editor, Project};
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Imported { clip, asset, track } = editor
            .execute(Command::ImportAudioTrack {
                name: "audio".into(),
                path: "derived.wav".into(),
                sample_rate: 48_000,
                frames: 2_048,
            })
            .unwrap()
        else {
            panic!("import")
        };
        editor
            .execute(Command::SetAudioClipPitch { clip, cents })
            .unwrap();
        let Created::Track(target) = editor
            .execute(Command::AddTrack {
                name: "routed".into(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        editor
            .execute(Command::SetAudioFeed {
                source: hex_project::RoutingOwner::Track(track),
                destination: target,
                gain: 0.5,
            })
            .unwrap();
        editor
            .execute(Command::ReplaceTrackGainEnvelope {
                track,
                points: vec![hex_project::GainPoint {
                    incoming_gain: None,
                    beat: hex_project::BeatTime::ZERO,
                    gain: 0.5,
                    outgoing: hex_project::GainInterpolation::Step,
                }],
            })
            .unwrap();
        let audio = hex_engine::Audio::new(
            48_000,
            (0..2_048)
                .map(|i| {
                    let x = (i as f32 * 0.173).sin() * 0.2;
                    [x, -x]
                })
                .collect(),
        )
        .unwrap();
        // The temporary default preparation cache is destroyed here. Only the
        // returned graph owns derived Audio; no UI/source/cache masks retirement.
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::from([(asset, audio)]),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let weak = Arc::downgrade(&revision);
        (
            Playback::try_from_prepared_with_session(revision).unwrap(),
            weak,
        )
    }
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut roots = Vec::new();
    let mut output = [[0.0; 2]; 31];
    for cents in [137, 211, 337, 411] {
        let (graph, weak) = transformed(cents);
        roots.push(weak);
        control.publish(graph).unwrap();
        control.command(PlaybackCommand::Play).unwrap();
        let allocation = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                allocation.count_total,
                allocation.bytes_total,
                allocation.count_current,
                allocation.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(
            roots.iter().all(|root| root.strong_count() > 0),
            "retired derived roots remain owned until control reclamation"
        );
    }
    control.request_shutdown();
    for _ in 0..3 {
        let allocation = allocation_counter::measure(|| {
            callback.begin_block();
        });
        assert_eq!(
            (
                allocation.count_total,
                allocation.bytes_total,
                allocation.count_current,
                allocation.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
    }
    assert!(roots.iter().all(|root| root.strong_count() == 0));
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
#[test]
fn split_gain_and_publication_each_reserve_before_the_other_actor_mutates() {
    for gain_first in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.status.split_endpoints.store(true, Release);
        if gain_first {
            control
                .gain_recording
                .begin(admission_gain_target(), 0.5)
                .unwrap();
            let (playback, weak) = graph();
            let refused = control.publish(playback).unwrap_err();
            assert_eq!(control.revision, 0);
            assert_eq!(
                control.publications.slots(),
                control.publications.buffer().capacity()
            );
            assert!(weak.upgrade().is_some());
            drop(refused);
            assert!(weak.upgrade().is_none());
        } else {
            control.publish(graph().0).unwrap();
            assert!(
                control
                    .gain_recording
                    .begin(admission_gain_target(), 0.5)
                    .is_err()
            );
            assert!(control.gain_recording.target().is_none());
            // Callback completion clears only publication; the next Begin
            // succeeds without any UI publication receipt collection.
            callback.begin_block();
            control
                .gain_recording
                .begin(admission_gain_target(), 0.5)
                .unwrap();
        }
    }
}
#[test]
fn unsplit_legacy_control_keeps_publish_then_begin_ordering() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.publish(graph().0).unwrap();
    control
        .gain_recording
        .begin(admission_gain_target(), 0.5)
        .unwrap();
    callback.begin_block();
    assert!(!control.status.publication_pending.load(Acquire));
    assert!(!control.status.publication_pending.reserve_publication());
    // Publication completion did not clear gain's independent bit.
    assert_eq!(
        control.gain_recording.target(),
        Some(admission_gain_target())
    );
}

#[test]
fn arrangement_stopped_gate_holds_play_through_install_and_stop_keeps_priority() {
    for stop_stage in 0..3 {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let mut old = graph().0;
        old.apply(PlaybackCommand::Seek(8));
        let revision = control
            .publish_with_policy_and_context_generation(
                old,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
        callback.begin_block();
        assert_eq!(control.arrangement_edit_position(gate), Some(8));
        assert!(
            control.publish(graph().0).is_err(),
            "unrelated publication cannot cross gate"
        );
        let _ = control.command(PlaybackCommand::Play);
        callback.begin_block();
        assert!(!control.status.playing.load(Acquire));
        if stop_stage == 1 {
            control.stop();
            callback.begin_block();
        }
        let mut next = graph().0;
        next.apply(PlaybackCommand::Seek(16));
        let installed = control
            .publish_with_policy_and_context_generation(
                next,
                PublicationPolicy::InstallUnderStoppedArrangement(gate),
                7,
            )
            .unwrap();
        let _ = control.command(PlaybackCommand::Play);
        if stop_stage == 2 {
            control.stop();
        }
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| callback.begin_block());
        #[cfg(not(feature = "allocation-probe"))]
        callback.begin_block();
        #[cfg(feature = "allocation-probe")]
        {
            assert_eq!(measured.count_total, 0);
            assert_eq!(measured.bytes_total, 0);
            assert_eq!(measured.count_current, 0);
            assert_eq!(measured.bytes_current, 0);
        }
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert!(!control.status.playing.load(Acquire));
        let position = callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap()
            .playback
            .position();
        assert_eq!(position, if stop_stage == 0 { 16 } else { 0 });
        let _ = control.command(PlaybackCommand::Play);
        callback.begin_block();
        assert!(
            !control.status.playing.load(Acquire),
            "accepted candidate retains gate until ACK owner retires it"
        );
        control.release_stopped_edit_fence(gate);
        control.command(PlaybackCommand::Play).unwrap();
        callback.begin_block();
        assert!(control.status.playing.load(Acquire));
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn live_arrangement_gate_inherits_installation_cursor_and_priority_stop() {
    for stop in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let revision = control
            .publish_with_policy_and_context_generation(
                graph().0,
                PublicationPolicy::InstallFreshRuntime,
                7,
            )
            .unwrap();
        callback.begin_block();
        control.command(PlaybackCommand::Play).unwrap();
        let gate = control
            .request_live_arrangement_edit_fence(7, revision)
            .unwrap();
        let mut pcm = [[0.0; 2]; 8];
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        assert_eq!(control.arrangement_edit_playing(gate), Some(true));
        let captured = control.arrangement_edit_position(gate).unwrap();
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        callback.begin_block();
        callback.render(&mut pcm).unwrap();
        assert!(control.publish(graph().0).is_err());
        let installed = control
            .publish_with_policy_and_context_generation(
                graph_with_frame([0.5; 2]).0,
                PublicationPolicy::InheritUnderArrangement(gate),
                7,
            )
            .unwrap();
        if stop {
            control.stop();
        }
        #[cfg(feature = "allocation-probe")]
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
        });
        #[cfg(not(feature = "allocation-probe"))]
        {
            callback.begin_block();
            callback.render(&mut pcm).unwrap();
        }
        #[cfg(feature = "allocation-probe")]
        assert_eq!(
            (
                measured.count_total,
                measured.bytes_total,
                measured.count_current,
                measured.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(control.status.applied_revision.load(Acquire), installed);
        assert_eq!(control.status.playing.load(Acquire), !stop);
        let active = callback
            .active
            .as_ref()
            .and_then(ActivePublication::legacy)
            .unwrap();
        if stop {
            assert_eq!(active.playback.position(), 0);
            assert_eq!(pcm, [[0.0; 2]; 8]);
        } else {
            assert!(active.playback.position() > captured + 8);
            assert_eq!(pcm, [[0.5; 2]; 8]);
        }
        control.release_stopped_edit_fence(gate);
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn arrangement_stopped_gate_refuses_an_earlier_play_without_changing_runtime() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let revision = control
        .publish_with_policy_and_context_generation(
            graph().0,
            PublicationPolicy::InstallFreshRuntime,
            7,
        )
        .unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    let gate = control.request_arrangement_edit_fence(7, revision).unwrap();
    callback.begin_block();
    assert_eq!(
        control.stopped_edit_fence_state(gate),
        StoppedEditFenceState::Refused
    );
    assert!(control.status.playing.load(Acquire));
    control.release_stopped_edit_fence(gate);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
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

#[test]
fn action_outcomes_require_a_vacant_matching_constructor_and_account_storage() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (_, mut foreign) = channel(48_000).unwrap();
    assert!(
        control
            .enable_action_outcomes_on_control(&mut foreign)
            .is_err()
    );
    let before = callback.constructor_channel_storage().unwrap();
    let reader = control
        .enable_action_outcomes_on_control(&mut callback)
        .unwrap();
    let after = callback.constructor_channel_storage().unwrap();
    assert_eq!(
        after.data_bytes - before.data_bytes,
        reader.retained_data_bytes()
    );
    assert_eq!(after.shared_groups - before.shared_groups, 1);
    assert_eq!(after.shared_handles - before.shared_handles, 3);
    assert!(
        control
            .enable_action_outcomes_on_control(&mut callback)
            .is_err()
    );
    let (mut late, mut late_callback) = channel(48_000).unwrap();
    late_callback.begin_block();
    assert!(
        late.enable_action_outcomes_on_control(&mut late_callback)
            .is_err()
    );
}

#[test]
fn legacy_quiescent_retirement_refuses_foreign_callback_and_interrupts_pending() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (_, mut foreign) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    control.request_shutdown();
    assert!(
        control
            .retire_action_outcomes_on_control(&mut foreign)
            .is_err()
    );
    assert_eq!(reader.try_take(), None);
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    assert_eq!(reader.failure(), None);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    callback.begin_block();
    control.reclaim();
}

#[test]
fn abandoning_legacy_endpoint_reports_unknown_instead_of_a_terminal() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    control.command_with_ticket(PlaybackCommand::Play).unwrap();
    drop(control);
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::EndpointAbandoned)
    );
    assert_eq!(reader.try_take(), None);
    drop(callback);
}

#[test]
fn resident_action_outcomes_preserve_distinct_rejections_before_polling() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let no_graph = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    begin_outcome_test_block(&mut callback);
    control.reclaim();
    control.publish(graph().0).unwrap();
    let metronome = control
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: no_graph,
            outcome: action_outcomes::Outcome::Rejected(PlaybackActionRejection::NoActivePlayback)
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: metronome,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::MetronomeUnavailable
            )
        })
    );
    assert_eq!(reader.try_take(), None);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcomes_cross_concurrent_admission_and_callback_handoff() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_after_fresh_fence_validation = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let session = control
            .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
            .unwrap();
        let metronome = control
            .action_with_ticket(PlaybackAction::SetMetronome(true))
            .unwrap();
        barrier.wait();
        audio.join().unwrap();
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: session,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::SessionNotPrepared
                ),
            })
        );
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket: metronome,
                outcome: action_outcomes::Outcome::Rejected(
                    PlaybackActionRejection::MetronomeUnavailable
                ),
            })
        );
    });
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn action_receipt_waits_for_transport_and_consumed_status() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    callback.test_before_action_outcome_status = Some(Arc::clone(&barrier));
    std::thread::scope(|scope| {
        let audio = scope.spawn(|| callback.begin_block());
        barrier.wait();
        let early = reader.try_take();
        let playing = control.status.playing.load(Acquire);
        let consumed = control.status.consumed_command.load(Acquire);
        barrier.wait();
        audio.join().unwrap();
        assert_eq!(early, None);
        assert!(!playing);
        assert!(consumed < ticket);
    });
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcome_backpressure_preserves_priority_stop_and_retries() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let mut tickets = Vec::new();
    for _ in 0..COMMANDS {
        tickets.push(control.command_with_ticket(PlaybackCommand::Play).unwrap());
    }
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    control.stop();
    begin_outcome_test_block(&mut callback);
    assert!(!control.status.playing.load(Acquire));
    assert!(control.command_with_ticket(PlaybackCommand::Play).is_err());
    for ticket in tickets {
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Superseded
            })
        );
    }
    let retried = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    begin_outcome_test_block(&mut callback);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: retried,
            outcome: action_outcomes::Outcome::Applied
        })
    );
    assert!(control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn resident_outcomes_report_each_pre_fault_action_and_stop() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let play = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    let pause = control.command_with_ticket(PlaybackCommand::Pause).unwrap();
    let stop = control.command_with_ticket(PlaybackCommand::Stop).unwrap();
    control.status.device_fault.store(true, Release);
    begin_outcome_test_block(&mut callback);
    for ticket in [play, pause] {
        assert_eq!(
            reader.try_take(),
            Some(action_outcomes::Receipt {
                ticket,
                outcome: action_outcomes::Outcome::Rejected(PlaybackActionRejection::OutputFailed),
            })
        );
    }
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: stop,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    control
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn action_outcomes_survive_graph_and_output_replacement_without_ticket_aliasing() {
    let (mut old, mut old_callback) = channel(48_000).unwrap();
    let mut old_reader = wire_action_outcomes(&mut old, &mut old_callback);
    old.publish(graph().0).unwrap();
    old_callback.begin_block();
    let first = old.command_with_ticket(PlaybackCommand::Play).unwrap();
    old.publish(graph().0).unwrap();
    old_callback.begin_block();
    let pending = old.command_with_ticket(PlaybackCommand::Pause).unwrap();
    assert!(!old_reader.is_retired());
    old.request_shutdown();
    old.retire_action_outcomes_on_control(&mut old_callback)
        .unwrap();
    assert!(old_reader.is_retired());

    let (mut replacement, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut replacement, &mut callback);
    assert_ne!(old_reader.output_identity(), reader.output_identity());
    replacement.publish(graph().0).unwrap();
    callback.begin_block();
    let replacement_ticket = replacement
        .command_with_ticket(PlaybackCommand::Play)
        .unwrap();
    callback.begin_block();
    assert_eq!(replacement_ticket, first);
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: replacement_ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: first,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: pending,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    old_callback.begin_block();
    old.reclaim();
    replacement.request_shutdown();
    replacement
        .retire_action_outcomes_on_control(&mut callback)
        .unwrap();
    callback.begin_block();
    replacement.reclaim();
}

#[test]
#[cfg(feature = "allocation-probe")]
fn split_client_admission_reaches_the_actual_callback_writer() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};

    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let session = client
        .session_command_with_ticket(hex_engine::SessionPlaybackCommand::Play)
        .unwrap();
    let metronome = client
        .action_with_ticket(PlaybackAction::SetMetronome(true))
        .unwrap();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    {
        let measured = allocation_counter::measure(|| {
            data.owner_mut()
                .unwrap()
                .run(|callback, _| callback.begin_block());
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
    }
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: session,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::SessionNotPrepared
            ),
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: metronome,
            outcome: action_outcomes::Outcome::Rejected(
                PlaybackActionRejection::MetronomeUnavailable
            ),
        })
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Applied
        })
    );
    let pending = client.command_with_ticket(PlaybackCommand::Pause).unwrap();
    client.close();
    publication.close();
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
    drop(data);
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
    drop(errors);
    // Both guard carriers are back, but the stream handle is not yet disposed.
    assert!(!retirement.advance().unwrap());
    assert_eq!(reader.try_take(), None);
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
    assert!(reader.is_retired());
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: pending,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    assert_eq!(reader.try_take(), None);
    assert_eq!(reader.failure(), None);
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn action_outcomes_wait_for_client_after_stream_and_callback_retirement() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    publication.close();
    drop(errors);
    drop(data);
    assert!(
        !crate::lifecycle::test_dispose_backend(
            &mut Some(()),
            &mut false,
            &mut false,
            &mut retirement,
            |_| Ok(())
        )
        .unwrap()
    );
    assert_eq!(reader.try_take(), None);
    assert!(
        !reader.is_retired(),
        "callback return alone is not quiescence"
    );
    client.close();
    assert!(retirement.advance().unwrap());
    assert!(
        reader.is_retired(),
        "retirement frontier must follow exact endpoint return"
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn retired_client_reader_stays_bound_to_its_output_after_replacement() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut old_reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let old_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    client.close();
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
    super::candidate_tests::finish(&mut retirement);

    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut new_reader = wire_action_outcomes(&mut control, &mut callback);
    assert_ne!(old_reader.output_identity(), new_reader.output_identity());
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (mut data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let new_ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert_eq!(new_ticket, old_ticket);
    data.owner_mut()
        .unwrap()
        .run(|callback, _| callback.begin_block());
    assert_eq!(
        new_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: new_ticket,
            outcome: action_outcomes::Outcome::Applied,
        })
    );
    assert_eq!(
        old_reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket: old_ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    client.close();
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
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn action_outcomes_report_retirement_failure_without_inventing_a_terminal() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let mut reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    let ticket = client.command_with_ticket(PlaybackCommand::Play).unwrap();
    retirement.fail_retained_receipts();
    assert!(!reader.is_retired());
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::RetirementUnavailable)
    );
    assert_eq!(reader.try_take(), None);
    assert!(client.command_with_ticket(PlaybackCommand::Pause).is_err());
    client.close();
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
    assert!(reader.is_retired());
    assert_eq!(
        reader.failure(),
        Some(ActionOutcomeFailure::RetirementUnavailable)
    );
    assert_eq!(
        reader.try_take(),
        Some(action_outcomes::Receipt {
            ticket,
            outcome: action_outcomes::Outcome::Interrupted,
        })
    );
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn abandoned_action_reader_does_not_destroy_ledger_on_callback_guard_drop() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let reader = wire_action_outcomes(&mut control, &mut callback);
    control.publish(graph().0).unwrap();
    callback.begin_block();
    let root = control.native_candidate_root();
    let (data, errors, terminals, mut client, mut publication) =
        reserve_native_worker(control, callback, None);
    let mut retirement = NativeRetirement::new(terminals, root);
    client.activate();
    client.command_with_ticket(PlaybackCommand::Play).unwrap();
    drop(reader);
    client.close();
    publication.close();
    let measured = allocation_counter::measure(|| drop(data));
    assert_eq!(
        (
            measured.count_total,
            measured.bytes_total,
            measured.count_current,
            measured.bytes_current
        ),
        (0, 0, 0, 0)
    );
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
    super::candidate_tests::finish(&mut retirement);
}

#[test]
#[cfg(feature = "allocation-probe")]
fn reliable_output_accepts_ordinary_stream_configuration() {
    let _serial = super::candidate_tests::serial();
    let (mut control, mut callback) = channel(48_000).unwrap();
    let _reader = wire_action_outcomes(&mut control, &mut callback);
    control.enable_native_custody().unwrap();
    control
        .configure_device_resource_publication_on_control(
            hex_engine::AudioStorageLedgerReservation {
                data_rows: 512,
                handle_rows: 256,
                root_rows: 128,
                max_retained_bytes: 20_000_000,
                max_construction_peak_bytes: 20_000_000,
            },
        )
        .unwrap();
    assert!(
        control
            .configure_ordinary_streaming_on_control(
                crate::AudioCandidateDomainReservation {
                    max_retained_data_bytes: 8 * 1024 * 1024,
                    max_construction_peak_data_bytes: 8 * 1024 * 1024,
                },
                3,
                4096,
            )
            .is_ok()
    );
    assert!(control.candidate_root.ordinary_streams_configured());
}

#[test]
#[cfg(feature = "allocation-probe")]
fn verified_streaming_accepts_both_outcome_modes() {
    use crate::native_callback_owner::{NativeRetirement, reserve_native_worker};
    let _serial = super::candidate_tests::serial();
    for reliable in [false, true] {
        let (mut control, mut callback) = channel(48_000).unwrap();
        let _reader = reliable.then(|| wire_action_outcomes(&mut control, &mut callback));
        let root = control.native_candidate_root();
        let (data, errors, terminals, mut client, mut publication) =
            reserve_native_worker(control, callback, None);
        let mut retirement = NativeRetirement::new(terminals, root);
        let worker = publication.control().unwrap();
        worker.enable_native_custody().unwrap();
        worker
            .configure_audio_candidate_publication(crate::AudioCandidatePublicationReservation {
                runtime_slots: 2,
                archive_slots: 2,
                member_rows: 256,
                transfer_rows: 256,
                receipt_rows: 3,
                data_rows: 4096,
                handle_rows: 2048,
                max_retained_data_bytes: 96 * 1024 * 1024,
                max_construction_peak_data_bytes: 96 * 1024 * 1024,
            })
            .unwrap();
        assert!(worker.configure_audio_candidate_streaming(3).is_ok());
        client.close();
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
        super::candidate_tests::finish(&mut retirement);
    }
}

#[test]
fn initial_stale_fence_rejects_play_without_active_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    let revision = control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    #[cfg(not(feature = "allocation-probe"))]
    callback.begin_block();
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
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_eq!(
        control.status.rejected_revision().map(|r| r.0),
        Some(revision)
    );
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::NoActivePlayback))
    );
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn priority_stop_discards_play_without_active_playback() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let fence = control.capture_fresh_runtime_fence().unwrap();
    control
        .publish_with_policy(
            graph().0,
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(fence),
        )
        .unwrap();
    let ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 0);
    assert_eq!(control.status.consumed_command.load(Acquire), ticket);
    assert_eq!(control.status.rejected_action(), None);
    assert!(!control.status.playing.load(Acquire));
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
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

#[test]
fn pre_fault_session_launch_cannot_restart_or_change_route() {
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, _, clip, _) = session_audio_graph();
    control.publish(playback).unwrap();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 16]).unwrap();
    let ticket = control
        .action_with_ticket(PlaybackAction::LaunchAndPlay(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    callback.report_render_fault(Some(&Invalid("injected failure")), 16);
    let mut output = [[0.0; 2]; 16];
    #[cfg(feature = "allocation-probe")]
    let allocations = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    });
    #[cfg(not(feature = "allocation-probe"))]
    {
        callback.begin_block();
        callback.render(&mut output).unwrap();
    }
    #[cfg(feature = "allocation-probe")]
    assert_eq!(
        (
            allocations.count_total,
            allocations.bytes_total,
            allocations.count_current,
            allocations.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(
        control.status.rejected_action(),
        Some((ticket, PlaybackActionRejection::OutputFailed))
    );
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert!(routes[0].pending.is_none());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[cfg(feature = "allocation-probe")]
#[test]
fn priority_stop_clears_pending_and_queued_session_launches_before_later_play() {
    use hex_engine::{LaunchQuantization, PlaybackAction, SessionPlaybackCommand};
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, weak, clip, track) = session_audio_graph();
    control.publish(playback).unwrap();
    callback.begin_block();
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 1]).unwrap();
    let quantized = SessionPlaybackCommand::LaunchClip {
        clip,
        quantization: LaunchQuantization::FourQuarter,
    };
    control
        .action_with_ticket(PlaybackAction::Session(quantized))
        .unwrap();
    callback.begin_block();
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].track_id, track.value());
    assert!(routes[0].pending.is_some());
    assert_eq!(
        routes[0].pending.unwrap().target,
        SessionRouteTarget::Session {
            clip_id: clip.value()
        }
    );
    let old_due = routes[0].pending.unwrap().performance_frame;
    assert_eq!(old_due, 96_000);

    let old_ticket = control
        .action_with_ticket(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    control.stop();
    let play_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    assert!(play_ticket > old_ticket);
    let mut output = [[0.0; 2]; 128];
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output).unwrap();
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
    assert_eq!(output, [[0.0; 2]; 128]);
    assert!(control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), play_ticket);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);

    // Cross the old due frame with real callback-sized blocks: no stale
    // Session audio or deferred launch may reappear after the Stop.
    let mut rendered = output.len() as u64;
    while rendered <= old_due {
        let n = ((old_due + 1 - rendered) as usize).min(output.len());
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output[..n]).unwrap();
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
        assert!(output[..n].iter().all(|frame| *frame == [0.0; 2]));
        rendered += n as u64;
    }
    assert_eq!(control.status.position.load(Acquire), old_due + 1);
    assert_eq!(control.status.performance_frame.load(Acquire), old_due + 1);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);
    control
        .action_with_ticket(PlaybackAction::Session(
            SessionPlaybackCommand::LaunchClip {
                clip,
                quantization: LaunchQuantization::Immediate,
            },
        ))
        .unwrap();
    let measured = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output[..4]).unwrap();
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
    assert_eq!(
        &output[..4],
        &[[0.25, -0.5], [0.75, -1.0], [0.0; 2], [0.0; 2]]
    );
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    control.stop();
    let stopped = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(&mut output[..4]).unwrap();
    });
    assert_eq!(
        (
            stopped.count_total,
            stopped.bytes_total,
            stopped.count_current,
            stopped.bytes_current
        ),
        (0, 0, 0, 0)
    );
    assert_eq!(&output[..4], &[[0.0; 2]; 4]);
    control.request_shutdown();
    let shutdown = allocation_counter::measure(|| callback.begin_block());
    assert_eq!(
        (
            shutdown.count_total,
            shutdown.bytes_total,
            shutdown.count_current,
            shutdown.bytes_current
        ),
        (0, 0, 0, 0)
    );
    control.reclaim();
    assert!(weak.upgrade().is_none());
}

#[test]
fn retired_session_audio_pcm_stays_owned_until_control_reclaims_publication() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (playback, weak_revision, _, _) = session_audio_graph();
    callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
        pcm_frames: 0,
        forwarded_pcm: false,
        revision: 1,
        control_context_generation: 0,
        explicit_control_context: false,
        source_context: None,
        policy: PublicationPolicy::InheritRuntime,
        fresh_runtime_cancellation: None,
        scheduled_cut_frame: None,
        streams: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_references: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_continuation: None,
        playback: Box::new(playback),
    }));
    assert!(weak_revision.upgrade().is_some());
    callback.begin_block();
    assert!(weak_revision.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(weak_revision.upgrade().is_none());
}

#[test]
fn reset_publication_drops_same_id_session_owner_after_stop_and_reclaim() {
    use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, old_weak, clip, track) = session_audio_graph();
    assert!(control.publish(old).is_ok());
    callback.begin_block();
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    let mut first_audio = [[0.0; 2]; 8];
    callback.render(&mut first_audio).unwrap();
    assert_eq!(control.status.performance_frame.load(Acquire), 8);
    let mut old_routes = [SessionRouteEntry::default(); 1];
    let old_snapshot = control
        .status
        .session_routes(&mut old_routes)
        .expect("old active route");
    assert_eq!(old_routes[0].track_id, track.value());
    assert_eq!(
        old_routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );

    let (replacement, _, same_clip, same_track) = session_audio_graph();
    assert_eq!(same_clip, clip);
    assert_eq!(same_track, track);
    assert!(
        control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .is_ok()
    );
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert!(callback.deferred.is_some());
    assert!(old_weak.upgrade().is_some());
    assert_eq!(
        old_routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    control.stop();
    callback.begin_block();
    assert!(callback.deferred.is_none());
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    let mut new_routes = [SessionRouteEntry::default(); 1];
    let new_snapshot = control
        .status
        .session_routes(&mut new_routes)
        .expect("replacement active route");
    assert!(new_snapshot.audio_revision > old_snapshot.audio_revision);
    assert_eq!(new_routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(new_routes[0].pending, None);
    assert!(old_weak.upgrade().is_some());

    let mut paused = [[1.0; 2]; 4];
    callback.render(&mut paused).unwrap();
    assert_eq!(paused, [[0.0; 2]; 4]);
    assert_eq!(control.reclaim(), 1);
    assert!(old_weak.upgrade().is_none());
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn fresh_runtime_waits_for_retirement_space_without_delaying_stop() {
    use hex_engine::{LaunchQuantization, SessionPlaybackCommand};

    let (mut control, mut callback) = channel(48_000).unwrap();
    let (old, old_weak, clip, _) = session_audio_graph();
    assert!(control.publish(old).is_ok());
    callback.begin_block();
    control
        .launch_and_play_with_ticket(SessionPlaybackCommand::LaunchClip {
            clip,
            quantization: LaunchQuantization::Immediate,
        })
        .unwrap();
    callback.begin_block();
    callback.render(&mut [[0.0; 2]; 8]).unwrap();

    // Fill the real retirement ring: publication must wait for control-side
    // reclamation rather than dropping the active graph on the callback.
    let mut retired_weak = Vec::new();
    for revision in 10..10 + RETIRED as u64 {
        let (playback, weak, _, _) = session_audio_graph();
        assert!(
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(playback),
                }))
                .is_ok()
        );
        retired_weak.push(weak);
    }
    let (replacement, replacement_weak, _, _) = session_audio_graph();
    assert!(
        control
            .publish_with_policy(replacement, PublicationPolicy::InstallFreshRuntime)
            .is_ok()
    );
    control.stop();
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(control.status.publication_pending.load(Acquire));
    assert!(!control.status.playing.load(Acquire));
    let mut routes = [SessionRouteEntry::default(); 1];
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(
        routes[0].owner,
        SessionRouteOwner::Session {
            clip_id: clip.value()
        }
    );
    let mut output = [[9.0; 2]; 8];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);
    assert!(old_weak.upgrade().is_some());
    assert!(replacement_weak.upgrade().is_some());
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));

    assert_eq!(control.reclaim(), RETIRED);
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(!control.status.publication_pending.load(Acquire));
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.performance_frame.load(Acquire), 0);
    control.status.session_routes(&mut routes).unwrap();
    assert_eq!(routes[0].owner, SessionRouteOwner::Arrangement);
    assert_eq!(routes[0].pending, None);
    assert!(old_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(old_weak.upgrade().is_none());
    control.request_shutdown();
    callback.begin_block();
    assert!(replacement_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), 1);
    assert!(replacement_weak.upgrade().is_none());
}

#[test]
fn held_retirement_does_not_block_stop_or_drop_ownership() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(graph().0).is_ok());
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    assert!(control.status.playing.load(Acquire));

    // Force the defensive retention path. Normal reserved swaps cannot
    // fill it, but it must retain ownership and honor Stop if exercised.
    let mut retired_weak = Vec::new();
    for revision in 2..=3 {
        let (playback, weak) = graph();
        assert!(
            callback
                .retired
                .push(ReturnedPublication::Legacy(Publication {
                    pcm_frames: 0,
                    forwarded_pcm: false,
                    revision,
                    control_context_generation: 0,
                    explicit_control_context: false,
                    source_context: None,
                    policy: PublicationPolicy::InheritRuntime,
                    fresh_runtime_cancellation: None,
                    scheduled_cut_frame: None,
                    streams: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_references: None,
                    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                    native_continuation: None,
                    playback: Box::new(playback),
                }))
                .is_ok()
        );
        retired_weak.push(weak);
    }
    let (held, held_weak) = graph();
    callback.held_retirement = Some(ReturnedPublication::Legacy(Publication {
        pcm_frames: 0,
        forwarded_pcm: false,
        revision: 4,
        control_context_generation: 0,
        explicit_control_context: false,
        source_context: None,
        policy: PublicationPolicy::InheritRuntime,
        fresh_runtime_cancellation: None,
        scheduled_cut_frame: None,
        streams: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_references: None,
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        native_continuation: None,
        playback: Box::new(held),
    }));
    let mut last_ticket = 0;
    for _ in 0..COMMANDS {
        last_ticket = control.command_with_ticket(PlaybackCommand::Play).unwrap();
    }
    control.stop();
    callback.begin_block();
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.status.consumed_command.load(Acquire), last_ticket);
    assert!(callback.held_retirement.is_some());
    assert!(held_weak.upgrade().is_some());
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_some()));
    let mut output = [[9.0; 2]; 8];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 8]);

    assert_eq!(control.reclaim(), RETIRED);
    assert!(retired_weak.iter().all(|weak| weak.upgrade().is_none()));
    control.request_shutdown();
    callback.begin_block();
    assert!(control.status.shutdown_complete.load(Acquire));
    assert!(held_weak.upgrade().is_some());
    assert_eq!(control.reclaim(), RETIRED);
    assert!(held_weak.upgrade().is_none());
}

#[test]
fn transient_handoff_keeps_candidate_gated_retries_and_old_graph_renders() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    control.set_audio_diagnostics(AudioDiagnosticsFlags::READINESS);
    let (old, _) = graph();
    assert_eq!(
        control
            .publish(old)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        1
    );
    control.command(PlaybackCommand::Play).unwrap();
    callback.begin_block();
    let mut output = [[0.0; 2]; 16];
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 16]);

    let (candidate, _) = graph();
    assert_eq!(
        control
            .publish(candidate)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        2
    );
    callback.test_transient_handoff_refusals = 2;
    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    let waiting = control.take_audio_diagnostic().unwrap();
    assert_eq!(waiting.revision, 2);
    assert_eq!(
        waiting.kind,
        AudioDiagnosticKind::PublicationWaiting {
            reason: PublicationWaitReason::RuntimeHandoff(
                hex_engine::PlaybackHandoffError::Session(
                    hex_engine::SessionPlaybackError::ReleaseBankFull,
                ),
            ),
        }
    );
    assert!(callback.deferred.is_some());
    assert_eq!(
        control.publish(graph().0).unwrap_err().reason,
        "playback publication already pending"
    );
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.25; 2]; 16]);

    control.stop();
    callback.begin_block();
    assert!(callback.deferred.is_some());
    assert_eq!(control.status.applied_revision.load(Acquire), 1);
    assert!(!control.status.playing.load(Acquire));
    callback.render(&mut output).unwrap();
    assert_eq!(output, [[0.0; 2]; 16]);

    callback.begin_block();
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    assert!(callback.deferred.is_none());
    assert!(!control.status.playing.load(Acquire));
    assert_eq!(control.reclaim(), 1);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn rejection_snapshots_never_pair_a_new_reason_with_an_old_ticket() {
    let status = Arc::new(Status::default());
    let writer = Arc::clone(&status);
    let thread = std::thread::spawn(move || {
        for ticket in 1..=50_000 {
            let reason = if ticket & 1 == 0 {
                PlaybackActionRejection::SessionNotPrepared
            } else {
                PlaybackActionRejection::SessionUnavailable
            };
            writer.publish_rejected_action(ticket, reason);
        }
    });
    while !thread.is_finished() {
        if let Some((ticket, reason)) = status.rejected_action() {
            let expected = if ticket & 1 == 0 {
                PlaybackActionRejection::SessionNotPrepared
            } else {
                PlaybackActionRejection::SessionUnavailable
            };
            assert_eq!(reason, expected);
        }
    }
    thread.join().unwrap();
    assert_eq!(
        status.rejected_action(),
        Some((50_000, PlaybackActionRejection::SessionNotPrepared))
    );
}

#[test]
fn shutdown_returns_active_deferred_and_queued_graphs_for_control_drops() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    let (active, active_weak) = graph();
    assert_eq!(
        control
            .publish(active)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        1
    );
    callback.begin_block();

    let (deferred, deferred_weak) = graph();
    assert_eq!(
        control
            .publish(deferred)
            .unwrap_or_else(|error| panic!("{}", error.reason)),
        2
    );
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    assert_eq!(
        callback.deferred.as_ref().map(|value| value.revision),
        Some(2)
    );

    // Model an already queued final owner while shutdown admission closes.
    let (queued, queued_weak) = graph();
    assert!(
        control
            .publications
            .push(QueuedPublication::Legacy(Publication {
                pcm_frames: 0,
                forwarded_pcm: false,
                revision: 3,
                control_context_generation: 0,
                explicit_control_context: false,
                source_context: None,
                policy: PublicationPolicy::InheritRuntime,
                fresh_runtime_cancellation: None,
                scheduled_cut_frame: None,
                streams: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_references: None,
                #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                native_continuation: None,
                playback: Box::new(queued),
            }))
            .is_ok()
    );
    control.request_shutdown();
    callback.begin_block();
    assert!(active_weak.upgrade().is_some());
    assert!(deferred_weak.upgrade().is_some());
    assert!(queued_weak.upgrade().is_some());
    while !control.status.shutdown_complete.load(Acquire) {
        control.reclaim();
        callback.begin_block();
    }
    control.reclaim();
    assert!(active_weak.upgrade().is_none());
    assert!(deferred_weak.upgrade().is_none());
    assert!(queued_weak.upgrade().is_none());
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

#[test]
fn unchanged_blocks_do_not_rescan_or_rewrite_route_slots() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(session_playback(101)).is_ok());
    callback.begin_block();
    let sequence = control.status.route_sequence.load(SeqCst);
    let slot_visits = callback.route_slot_visits;
    let mut output = [[0.0; 2]; 128];
    callback.render(&mut output).unwrap();
    assert_eq!(control.status.route_sequence.load(SeqCst), sequence);
    assert_eq!(callback.route_slot_visits, slot_visits);
    assert_eq!(slot_visits, MAX_TRACKS);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn deferred_route_candidate_keeps_active_snapshot_until_acceptance() {
    let (mut control, mut callback) = channel(48_000).unwrap();
    assert!(control.publish(session_playback(101)).is_ok());
    callback.begin_block();
    let mut routes = [SessionRouteEntry::default(); 1];
    let active = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(active.audio_revision, 1);
    assert_eq!(routes[0].track_id, 101);

    assert!(control.publish(session_playback(202)).is_ok());
    callback.test_transient_handoff_refusals = 1;
    callback.begin_block();
    let deferred = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(deferred.audio_revision, 1);
    assert_eq!(routes[0].track_id, 101);
    assert_eq!(control.status.applied_revision.load(Acquire), 1);

    callback.begin_block();
    let accepted = control.status.session_routes(&mut routes).unwrap();
    assert_eq!(accepted.audio_revision, 2);
    assert_eq!(routes[0].track_id, 202);
    assert_eq!(control.status.applied_revision.load(Acquire), 2);
    control.reclaim();
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}

#[test]
fn concurrent_route_reader_never_observes_mixed_revision_or_track_list() {
    use std::sync::atomic::AtomicBool;
    use std::thread;

    let (mut control, mut callback) = channel(48_000).unwrap();
    let status = Arc::clone(&control.status);
    let done = Arc::new(AtomicBool::new(false));
    let reader_done = Arc::clone(&done);
    let reader = thread::spawn(move || {
        let mut entries = [SessionRouteEntry {
            track_id: 0,
            owner: SessionRouteOwner::Silence,
            active_origin: None,
            audio_launch_frame: None,
            pending: None,
        }; 1];
        let mut observed = 0;
        while !reader_done.load(Acquire) {
            let Some(snapshot) = status.session_routes(&mut entries) else {
                continue;
            };
            if snapshot.audio_revision == 0 {
                assert_eq!(snapshot.count, 0);
                assert_eq!(snapshot.capability, SessionCapabilityStatus::Legacy);
            } else {
                assert_eq!(snapshot.count, 1);
                assert_eq!(snapshot.capability, SessionCapabilityStatus::Available);
                let expected_track = if snapshot.audio_revision % 2 == 1 {
                    101
                } else {
                    202
                };
                assert_eq!(entries[0].track_id, expected_track);
            }
            observed += 1;
        }
        observed
    });

    for revision in 1..=64 {
        let track_id = if revision % 2 == 1 { 101 } else { 202 };
        assert_eq!(
            control
                .publish(session_playback(track_id))
                .unwrap_or_else(|error| panic!("{}", error.reason)),
            revision
        );
        callback.begin_block();
        control.reclaim();
    }
    done.store(true, Release);
    assert!(reader.join().unwrap() > 0);
    control.request_shutdown();
    callback.begin_block();
    control.reclaim();
}
#[cfg(feature = "allocation-probe")]
#[test]
fn contextual_monitor_input_rebind_keeps_keys_across_edits_and_fresh_seek_without_history() {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_project::{Command, Created, Editor, Project};
    // An explicit generation zero is legitimate; the legacy default zero is unknown.
    for (old_context, next_context, fresh, retained) in [
        (Some(0), Some(0), false, true),
        (Some(7), Some(7), true, true),
        (None, None, false, false),
        (Some(7), Some(8), false, false),
    ] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().unwrap(),
            })
            .unwrap()
        else {
            panic!("track")
        };
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        let audition = Arc::new(hex_engine::tuning_audition::TuningAuditionInput::new(
            old_context.unwrap_or(0),
        ));
        audition.set_key(0, Some((64, 100, 2))).unwrap();
        let graph = |project: &Project| {
            let revision = Arc::new(
                hex_engine::PreparedRevision::prepare(
                    project,
                    &std::collections::BTreeMap::new(),
                    Arc::new(hex_engine::SineTable::new()),
                    None,
                )
                .unwrap(),
            );
            let mut playback = Playback::try_from_prepared_with_session(revision).unwrap();
            playback
                .prepare_computer_keyboard(project, track, Arc::clone(&input))
                .unwrap();
            playback
                .prepare_tuning_audition(project, Arc::clone(&audition))
                .unwrap();
            playback
        };
        let (mut control, mut callback) = channel(48_000).unwrap();
        let first = graph(editor.project());
        match old_context {
            Some(generation) => control
                .publish_with_policy_and_context_generation(
                    first,
                    PublicationPolicy::InheritRuntime,
                    generation,
                )
                .unwrap(),
            None => control.publish(first).unwrap(),
        };
        let mut output = [[0.0; 2]; 64];
        callback.begin_block();
        callback.render(&mut output).unwrap();
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        editor.execute(Command::Gain { track, gain: 0.7 }).unwrap();
        let next = graph(editor.project());
        let policy = if fresh {
            PublicationPolicy::InstallFreshRuntimeIfUnchanged(
                control.capture_fresh_runtime_fence().unwrap(),
            )
        } else {
            PublicationPolicy::InheritRuntime
        };
        let revision = match next_context {
            Some(generation) => control
                .publish_with_policy_and_context_generation(next, policy, generation)
                .unwrap(),
            None => control.publish_with_policy(next, policy).unwrap(),
        };
        // Queue pressure returns the candidate and cannot detach the audible input.
        let refused = control.publish(graph(editor.project())).unwrap_err();
        drop(refused);
        assert!(!input.is_cancelled());
        let measured = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
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
        if retained {
            assert_eq!(control.status.applied_revision.load(Acquire), revision);
            assert!(!input.is_cancelled());
        } else {
            // Without a matching input-context generation, the callback
            // cannot transfer source-owned live IDs safely. Keep the old
            // graph and input installed rather than retiring their only
            // shared DSP owner or pretending the new revision applied.
            assert_eq!(control.status.applied_revision.load(Acquire), 1);
            assert_eq!(
                control.status.rejected_revision(),
                Some((revision, PlaybackHandoffRejection::SessionRuntime)),
                "context={old_context:?}->{next_context:?}, fresh={fresh}"
            );
            assert!(!input.is_cancelled());
        }
        // Normal inheritance retains the existing certified audition continuation. Fresh
        // runtime installation instead rebinds only its input after the final fence cut.
        if fresh {
            assert!(!audition.is_cancelled());
        }
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        control.reclaim();
        control.stop();
        callback.begin_block();
        assert!(input.is_cancelled());
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[cfg(feature = "allocation-probe")]
#[test]
fn computer_keyboard_callback_publication_polyphony_release_pressure_stop_and_retirement_allocate_nothing()
 {
    use hex_engine::computer_keyboard::ComputerKeyboardInput;
    use hex_project::{Command, Created, Editor, Project};
    for (routed, mutable) in [(false, false), (true, false), (false, true), (true, true)] {
        let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
        let Created::Track(track) = editor
            .execute(Command::InsertTrackWithDevice {
                name: "keys".into(),
                before_track: None,
                slot: hex_project::TrackDeviceSlot::Instrument,
                state: hex_project::default_instrument_state().expect("fixture instrument state"),
            })
            .unwrap()
        else {
            panic!("track")
        };
        if mutable {
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
            let device = editor
                .project()
                .track(track)
                .unwrap()
                .instrument()
                .unwrap()
                .id();
            editor
                .execute(Command::ReplaceDeviceState { device, state })
                .unwrap();
        }
        if routed {
            let Created::Track(bus) = editor
                .execute(Command::AddTrack { name: "bus".into() })
                .unwrap()
            else {
                panic!("bus")
            };
            editor
                .execute(Command::SetAudioFeed {
                    source: hex_project::RoutingOwner::Track(track),
                    destination: bus,
                    gain: 0.5,
                })
                .unwrap();
        }
        let revision = Arc::new(
            hex_engine::PreparedRevision::prepare(
                editor.project(),
                &std::collections::BTreeMap::new(),
                Arc::new(hex_engine::SineTable::new()),
                None,
            )
            .unwrap(),
        );
        let input = Arc::new(ComputerKeyboardInput::default());
        input.set_key(0, Some((60, 100, 1))).unwrap();
        input.set_key(1, Some((64, 80, 2))).unwrap();
        let mut playback = Playback::from_prepared(Arc::clone(&revision));
        playback
            .prepare_computer_keyboard(editor.project(), track, Arc::clone(&input))
            .unwrap();
        let (mut control, mut callback) = channel(48_000).unwrap();
        control.publish(playback).unwrap();
        let mut output = [[0.0; 2]; 256];
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(output.iter().flatten().any(|sample| sample.abs() > 1e-6));
        input.set_key(0, None).unwrap();
        input.set_key(1, None).unwrap();
        let measure = allocation_counter::measure(|| {
            for _ in 0..if mutable { 2000 } else { 12 } {
                callback.begin_block();
                callback.render(&mut output).unwrap();
            }
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert_eq!(output, [[0.0; 2]; 256]);
        input.set_key(0, Some((60, 100, 3))).unwrap();
        callback.begin_block();
        callback.render(&mut output).unwrap();
        for _ in 0..COMMANDS {
            control.command(PlaybackCommand::Pause).unwrap();
        }
        assert!(control.command(PlaybackCommand::Pause).is_err());
        control.stop();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        assert!(input.is_cancelled());
        assert_eq!(output, [[0.0; 2]; 256]);
        control.publish(Playback::from_prepared(revision)).unwrap();
        let measure = allocation_counter::measure(|| {
            callback.begin_block();
            callback.render(&mut output).unwrap();
        });
        assert_eq!(
            (
                measure.count_total,
                measure.bytes_total,
                measure.count_current,
                measure.bytes_current
            ),
            (0, 0, 0, 0)
        );
        control.reclaim();
        control.request_shutdown();
        callback.begin_block();
        control.reclaim();
    }
}

#[test]
fn native_epoch_successor_accepts_same_or_newer_revision_in_same_generation() {
    let ctx = |project_generation, editor_revision| {
        Some(hex_engine::AudioSourceContext {
            project_generation,
            editor_revision,
        })
    };
    assert!(native_epoch_accepts_successor(ctx(1, 15), ctx(1, 15)));
    assert!(native_epoch_accepts_successor(ctx(1, 15), ctx(1, 16)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), ctx(1, 14)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), ctx(2, 16)));
    assert!(!native_epoch_accepts_successor(None, ctx(1, 15)));
    assert!(!native_epoch_accepts_successor(ctx(1, 15), None));
}
