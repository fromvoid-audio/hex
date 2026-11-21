//! A device clipboard copies authored instrument state without copying its live notes.
use super::*;
use hex_devices::fm4::device::{FM4_DEFINITION, parameter};
use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
use hex_project::{DeviceChainOwner, DeviceSelectionIds, TrackDeviceSlot};

fn play(revision: Arc<PreparedRevision>) -> SessionPlayback {
    let mut playback = SessionPlayback::new(revision).unwrap();
    playback.apply(SessionPlaybackCommand::Play).unwrap();
    playback
}

fn render(playback: &mut SessionPlayback, frames: usize) -> Vec<Frame> {
    let mut output = vec![[0.0; 2]; frames];
    for part in output.chunks_mut(127) {
        playback.render(part).unwrap();
    }
    output
}

fn render_in_cut_partitions(playback: &mut SessionPlayback, cut: usize, end: usize) -> Vec<Frame> {
    let mut output = render(playback, cut);
    output.extend(render(playback, end - cut));
    output
}

#[test]
fn pasted_native_instrument_waits_for_its_own_note_while_the_original_keeps_playing() {
    const CUT: usize = 128;
    const END: usize = 1_024;
    // Default 120 BPM at 48 kHz places one thirty-second of a beat at frame 750.
    const TARGET_ONSET: usize = 750;
    let target_beat = BeatTime::from_raw(1 << 27);
    let default_state = FM4_DEFINITION.default_state().unwrap();
    let state = FM4_DEFINITION
        .patch_parameter(
            &default_state,
            parameter::oscillator_ratio(1),
            ParameterValue::Float(1.5),
        )
        .unwrap();
    assert_ne!(state, default_state);
    let mut editor = Editor::new(Project::new(48_000).unwrap(), 8).unwrap();
    let Created::Track(source_track) = editor
        .execute(Command::InsertTrackWithDevice {
            name: "sounding instrument".into(),
            before_track: None,
            slot: TrackDeviceSlot::Instrument,
            state: state.clone(),
        })
        .unwrap()
    else {
        panic!("source instrument track");
    };
    let Created::Track(target_track) = editor
        .execute(Command::AddMidiTrack {
            name: "clipboard destination".into(),
        })
        .unwrap()
    else {
        panic!("target midi track");
    };
    // Separate the channels so moving a voice cannot look like preserving it.
    for (track, onset, pitch, balance) in [
        (source_track, BeatTime::ZERO, 60, -1.0),
        (target_track, target_beat, 67, 1.0),
    ] {
        editor.execute(Command::Balance { track, balance }).unwrap();
        let Created::Clip(clip) = editor
            .execute(Command::PlaceMidiClip {
                track,
                start: BeatTime::ZERO,
                length: BeatTime::ONE,
            })
            .unwrap()
        else {
            panic!("arrangement midi clip");
        };
        editor
            .execute(Command::AddMidiNote {
                clip,
                start: onset,
                duration: BeatTime::from_raw(1 << 31),
                pitch,
                channel: 0,
                velocity: 100,
            })
            .unwrap();
    }
    let original = editor
        .project()
        .track(source_track)
        .unwrap()
        .instrument()
        .unwrap()
        .id();
    let table = Arc::new(hex_devices::SineTable::new());
    let prepare = |project: &Project, prior: Option<&PreparedSessionInventory>| {
        Arc::new(
            PreparedRevision::prepare(project, &BTreeMap::new(), Arc::clone(&table), prior)
                .unwrap(),
        )
    };
    let source_revision = prepare(editor.project(), None);
    let source_only = render_in_cut_partitions(&mut play(Arc::clone(&source_revision)), CUT, END);
    let mut previous = play(source_revision);
    assert_eq!(render(&mut previous, CUT), source_only[..CUT]);
    assert!(source_only[..CUT].iter().any(|frame| frame[0] != 0.0));
    assert!(source_only.iter().all(|frame| frame[1] == 0.0));

    let clipboard = Arc::new(
        editor
            .project()
            .capture_devices(
                DeviceChainOwner::Track(source_track),
                &DeviceSelectionIds::new(&[original]).unwrap(),
            )
            .unwrap(),
    );
    let Created::Device(copy) = editor
        .execute(Command::PasteDevices {
            clipboard,
            owner: DeviceChainOwner::Track(target_track),
            before: None,
        })
        .unwrap()
    else {
        panic!("pasted instrument");
    };
    let source = editor
        .project()
        .track(source_track)
        .unwrap()
        .instrument()
        .unwrap();
    let target = editor
        .project()
        .track(target_track)
        .unwrap()
        .instrument()
        .unwrap();
    assert_eq!(source.id(), original);
    assert_eq!(target.id(), copy);
    assert_ne!(copy, original);
    assert_eq!(source.state(), &state);
    assert_eq!(target.state(), &state);

    // Independent playback of the pasted project has no live source to inherit.
    let expected = render_in_cut_partitions(&mut play(prepare(editor.project(), None)), CUT, END);
    assert!(
        expected
            .iter()
            .zip(&source_only)
            .all(|(both, original)| both[0] == original[0]),
        "the destination must leave the original channel untouched",
    );
    assert!(expected[..TARGET_ONSET].iter().all(|frame| frame[1] == 0.0));
    assert!(
        expected[TARGET_ONSET..TARGET_ONSET + 8]
            .iter()
            .any(|frame| frame[1] != 0.0),
        "the destination must actually sound after its own note-on",
    );

    let mut candidate =
        SessionPlayback::new(prepare(editor.project(), Some(previous.revision.session()))).unwrap();
    candidate.try_inherit_state_from(&mut previous).unwrap();
    assert_eq!(candidate.arrangement_frame(), CUT as u64);
    assert_eq!(render(&mut candidate, END - CUT), expected[CUT..]);
    assert_eq!(candidate.arrangement_frame(), END as u64);
}
