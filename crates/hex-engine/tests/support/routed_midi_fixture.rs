//! Preserve independent audio/instrument DSP summing through valid authored MIDI routing.
//! The source is silent and instrumentless; the original owner retains its complete
//! gain, effects, capture, release and hardware-independent preparation contracts.
pub fn separate_midi(snapshot: &mut hex_project::Snapshot, owner: usize) {
    let source_id = snapshot.next_id;
    let route_id = source_id.checked_add(1).expect("fixture route id");
    snapshot.next_id = route_id.checked_add(1).expect("fixture next id");
    let mut source = snapshot.tracks[owner].clone();
    let hex_project::TrackContentSnapshot::Hybrid { midi_clips, .. } =
        &mut snapshot.tracks[owner].content
    else {
        panic!("hybrid fixture owner")
    };
    source.id = source_id;
    source.name = "silent midi source".into();
    source.content = hex_project::TrackContentSnapshot::Hybrid {
        audio_clips: Vec::new(),
        midi_clips: std::mem::take(midi_clips),
    };
    source.instrument = None;
    source.instrument_position = None;
    source.devices.clear();
    source.gain = 1.0;
    source.balance = 0.0;
    source.gain_envelope.clear();
    source.gain_touch_spans.clear();
    source.gain_touch_epoch = 0;
    source.sends.clear();
    source.muted = false;
    source.soloed = false;
    snapshot.midi_routes.push(hex_project::MidiRouteSnapshot {
        id: route_id,
        source: source_id,
        destination: snapshot.tracks[owner].id,
    });
    snapshot.tracks.push(source);
}
