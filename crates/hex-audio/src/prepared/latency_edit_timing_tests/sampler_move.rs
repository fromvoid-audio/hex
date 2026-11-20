//! Move a playing Sampler without restarting its original sample read cursor.
use super::*;
use hex_devices::sampler::{Envelope, state::SamplerState};
use hex_engine::{
    AudioStorageLedgerReservation, OwnedAudioSource, PreparedRevision, SineTable,
    native_device_resources::{
        PreparedDeviceResources, ResidentDeviceAssets, ResourcePreparationError,
    },
};
use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
use hex_project::{BeatTime, DeviceSelectionIds, TrackId};
use std::sync::atomic::AtomicBool;

#[test]
fn direct_sampler_move_carries_the_original_sample_and_held_gate() {
    const REQUEST: usize = 512;
    const FADE: usize = 240;
    const CUT: usize = REQUEST + FADE;
    const FRAMES: usize = 8_192;
    let mut editor = Editor::new(Project::new(RATE).unwrap(), 16).unwrap();
    let Created::Asset(asset) = editor
        .execute(Command::ImportDeviceAsset {
            path: "memory/sampler-move.wav".into(),
            sample_rate: RATE,
            frames: FRAMES as u64,
        })
        .unwrap()
    else {
        panic!("original asset");
    };

    let add_track = |editor: &mut Editor, name: &str| {
        let Created::Track(id) = editor
            .execute(Command::AddMidiTrack { name: name.into() })
            .unwrap()
        else {
            panic!("midi track");
        };
        id
    };
    let donor = add_track(&mut editor, "donor");
    let receiver = add_track(&mut editor, "receiver");
    let mut sampler = SamplerState::for_source_frames(FRAMES).unwrap();
    sampler.envelope = Envelope {
        attack_seconds: 0.0,
        decay_seconds: 0.0,
        sustain: 1.0,
        release_seconds: 0.012,
    };
    let state = sampler
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    editor
        .execute(Command::InsertInstrument {
            track: donor,
            state,
        })
        .unwrap();
    let device = editor
        .project()
        .track(donor)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let place = |editor: &mut Editor, track: TrackId| {
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("midi clip");
        };
        clip
    };
    let donor_clip = place(&mut editor, donor);
    let receiver_clip = place(&mut editor, receiver);
    let add_note = |editor: &mut Editor, clip, frame, frames, pitch| {
        let clock = editor.project().tempo_map();
        let command = Command::AddMidiNote {
            clip,
            start: clock.frame_to_beat(frame).unwrap(),
            duration: clock.frame_to_beat(frames).unwrap(),
            pitch,
            channel: 0,
            velocity: 127,
        };
        editor.execute(command).unwrap();
    };
    add_note(&mut editor, donor_clip, 0, 1950, 60);
    let mut reference = Editor::new(editor.project().clone(), 8).unwrap();
    for (frame, duration, pitch) in [(CUT as u64, 660, 69), (2600, 700, 74)] {
        add_note(&mut editor, receiver_clip, frame, duration, pitch);
        add_note(&mut reference, donor_clip, frame, duration, pitch);
    }
    add_note(&mut editor, receiver_clip, 128, 5600, 84);
    editor
        .execute(Command::SetTrackGains {
            gains: vec![(donor, 0.25), (receiver, 0.75)],
        })
        .unwrap();

    let cancel = AtomicBool::new(false);
    let store = || {
        let mut assets = ResidentDeviceAssets::default();
        let input = (0..FRAMES)
            .map(|frame| {
                let sample = ((frame % 509) as f32 - 254.0) / 1024.0;
                [sample, -sample * 0.37]
            })
            .collect();
        assets
            .insert_on_control(
                (
                    asset,
                    OwnedAudioSource::try_resident(RATE, input).ok().unwrap(),
                ),
                0,
                AudioStorageLedgerReservation {
                    data_rows: 512,
                    handle_rows: 256,
                    root_rows: 128,
                    max_retained_bytes: 20_000_000,
                    max_construction_peak_bytes: 20_000_000,
                },
                FRAMES as u64,
                &cancel,
                || Ok::<_, ResourcePreparationError>(()),
            )
            .unwrap();
        assets
    };
    let assets = store();

    let sine = Arc::new(SineTable::new());
    let prepare = |project: &Project, assets: &ResidentDeviceAssets| {
        let resources =
            PreparedDeviceResources::prepare_on_control(assets, project, 1_000_000, &cancel)
                .unwrap();
        let revision = PreparedRevision::prepare_with_device_resources(
            project,
            &BTreeMap::new(),
            Arc::clone(&sine),
            None,
            &hex_engine::audio_transform::AudioTransformPreparation::default(),
            &resources,
            1_000_000,
            &cancel,
        )
        .unwrap();
        (Arc::new(revision), resources)
    };
    let playback = |(revision, resources): &(Arc<PreparedRevision>, PreparedDeviceResources)| {
        Playback::try_from_prepared_with_device_resources(Arc::clone(revision), resources).unwrap()
    };
    let before = prepare(editor.project(), &assets);
    editor
        .execute(Command::MoveTrackDevices {
            source: donor,
            destination: receiver,
            selected: DeviceSelectionIds::new(&[device]).unwrap(),
        })
        .unwrap();
    let after = prepare(editor.project(), &assets);
    let mut dry = playback(&prepare(reference.project(), &assets));
    dry.apply(PlaybackCommand::Play);

    let mut expected = vec![[0.0; 2]; FRAMES];
    dry.render(&mut expected[..REQUEST]).unwrap();
    dry.render(&mut expected[REQUEST..CUT]).unwrap();
    for block in expected[CUT..].chunks_mut(BLOCK) {
        dry.render(block).unwrap();
    }
    assert!(
        expected[REQUEST..CUT]
            .iter()
            .flatten()
            .any(|sample| sample.abs() > 1e-4)
    );
    assert!(
        expected[FRAMES - 64..]
            .iter()
            .all(|frame| *frame == [0.0; 2])
    );

    let (control, mut callback, donor_stamp, receipt) = begin_quantized_playback_insert(
        playback(&before),
        playback(&after),
        None,
        REQUEST,
        LaunchQuantization::ThirtySecond,
    );
    let mut pcm = vec![[0.0; 2]; FRAMES - REQUEST];
    let (fade, following) = pcm.split_at_mut(FADE);
    render_without_allocation(&mut callback, fade);
    let during_fade = (receipt.state(), control.status.recording_source());
    let (cut, remaining) = following.split_at_mut(BLOCK);
    render_without_allocation(&mut callback, cut);
    let at_cut = receipt.state();
    for block in remaining.chunks_mut(BLOCK) {
        render_without_allocation(&mut callback, block);
    }

    let expected = &expected[REQUEST..];
    let mismatch = |faded: bool| {
        pcm.iter()
            .zip(expected)
            .enumerate()
            .find_map(|(offset, (actual, want))| {
                let gain = if !faded {
                    0.75
                } else if offset < FADE {
                    0.25 * (1.0 - (offset + 1) as f32 / FADE as f32)
                } else {
                    0.75 * (offset - FADE + 1).min(FADE) as f32 / FADE as f32
                };
                (0..2).find_map(|lane| {
                    let expected = want[lane] * gain;
                    let error = (actual[lane] - expected).abs();
                    (!error.is_finite() || error >= 2e-6).then_some((
                        REQUEST + offset,
                        lane,
                        actual[lane],
                        expected,
                    ))
                })
            })
    };

    assert_eq!(
        (during_fade, at_cut, mismatch(true)),
        (
            (PublicationReceiptState::Pending, Some(donor_stamp)),
            PublicationReceiptState::Installed,
            None
        ),
        "first mismatch against an immediate full-gain destination cut: {:?}",
        mismatch(false),
    );
    assert_eq!(
        control.status.recording_source().unwrap().audio_revision,
        receipt.revision()
    );

    // Identical contents and asset ID in a separate retained owner cannot
    // authenticate the running sampler's resource lease.
    let unrelated_assets = store();
    let unrelated = prepare(editor.project(), &unrelated_assets);
    let (control, mut callback, predecessor, receipt) = begin_quantized_playback_insert(
        playback(&before),
        playback(&unrelated),
        None,
        REQUEST,
        LaunchQuantization::ThirtySecond,
    );
    let mut output = [[0.0; 2]; 64];
    render_without_allocation(&mut callback, &mut output);
    assert_eq!(
        receipt.state(),
        PublicationReceiptState::Refused(PlaybackHandoffRejection::SessionRuntime)
    );
    assert_eq!(control.status.recording_source(), Some(predecessor));
    for (actual, dry) in output.into_iter().zip(expected) {
        for (actual, dry) in actual.into_iter().zip(dry) {
            assert!((actual - dry * 0.25).abs() < 2e-6);
        }
    }
}

fn render_without_allocation(callback: &mut Callback, block: &mut [Frame]) {
    let counts = allocation_counter::measure(|| {
        callback.begin_block();
        callback.render(block).unwrap();
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
