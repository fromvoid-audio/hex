use super::*;
use std::sync::Arc;

fn audio_fixture() -> Clip {
    Clip {
        name: "té\0st".into(),
        flavor: ClipFlavor::default(),
        id: ClipId(1),
        asset: AssetId(2),
        start: 10,
        duration: 100,
        source_offset: 0,
        source_length: 100,
        looping: false,
        source_phase: 0,
        fade: ClipFade::neutral(100).unwrap(),
        gain: ClipGain::UNITY,
        edit: None,
    }
}
fn token(clip: &Clip) -> ClipContentToken {
    let mut result =
        ClipContentToken::reserve_on_control(clip.content_token_capacity().unwrap()).unwrap();
    clip.capture_content(&mut result).unwrap();
    result
}
fn edit(clip: &mut Clip) -> &mut AudioClipEdit {
    if clip.edit.is_none() {
        clip.edit = Some(Arc::new(clip.audio_event_root()));
    }
    Arc::make_mut(clip.edit.as_mut().unwrap())
}
#[test]
fn audio_token_matches_derived_eq_for_every_owning_field_and_equal_new_allocation() {
    let original = audio_fixture();
    let saved = token(&original);
    let mut variants = vec![original.clone()];
    macro_rules! change {
        ($body:expr) => {{
            let mut c = original.clone();
            ($body)(&mut c);
            variants.push(c);
        }};
    }
    change!(|c: &mut Clip| c.name.push('x'));
    change!(|c: &mut Clip| c.flavor = c.flavor.toggled());
    change!(|c: &mut Clip| c.id = ClipId(99));
    change!(|c: &mut Clip| c.asset = AssetId(99));
    change!(|c: &mut Clip| c.start += 1);
    change!(|c: &mut Clip| c.duration += 1);
    change!(|c: &mut Clip| c.source_offset += 1);
    change!(|c: &mut Clip| c.source_length += 1);
    change!(|c: &mut Clip| c.looping = true);
    change!(|c: &mut Clip| c.source_phase += 1);
    change!(|c: &mut Clip| c.fade = ClipFade::new(100, 0, 1, 2).unwrap());
    change!(|c: &mut Clip| c.gain = ClipGain::new(-100).unwrap());
    change!(|c: &mut Clip| {
        edit(c);
    });
    for c in variants {
        assert_eq!(c.matches_content(&saved), c == original);
    }
    let mut retained = original.clone();
    edit(&mut retained);
    let saved = token(&retained);
    let equal = retained.clone();
    let mut detached = equal.clone();
    let _ = Arc::make_mut(&mut Arc::make_mut(detached.edit.as_mut().unwrap()).domain);
    assert!(detached.matches_content(&saved));
    assert_eq!(detached, retained);
}
#[test]
fn exact_stored_events_optional_domains_and_frozen_projection_are_not_editable_views() {
    let mut original = audio_fixture();
    let root = edit(&mut original);
    root.view_start = 20;
    root.view_end = 40;
    let domain = Arc::make_mut(&mut root.domain);
    domain.events = Some(Arc::new(AudioEvents {
        next_id: 5,
        events: vec![
            AudioEvent {
                id: 1,
                start: 0,
                end: 10,
                source_start: 0,
            },
            AudioEvent {
                id: 2,
                start: 50,
                end: 60,
                source_start: 10,
            },
        ],
    }));
    domain.fade_projection = Some(AudioFadeProjection {
        tempo: TempoMap::constant(48000, Tempo::new(120_000_000).unwrap()).unwrap(),
        origin_beat_raw: -17,
        local_start_raw: 5,
    });
    let saved = token(&original);
    let mut variants = vec![original.clone()];
    macro_rules! change {
        ($body:expr) => {{
            let mut c = original.clone();
            ($body)(edit(&mut c));
            variants.push(c);
        }};
    }
    change!(|e: &mut AudioClipEdit| e.view_start += 1);
    change!(|e: &mut AudioClipEdit| e.view_end += 1);
    change!(|e: &mut AudioClipEdit| e.musical_start = Some(BeatTime::ZERO));
    change!(
        |e: &mut AudioClipEdit| e.placement = Some(AudioViewPlacement {
            mode: AudioViewMode::Once,
            outer_length: AudioDomainClock::Frames(7),
            cycle_phase: 1
        })
    );
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).events = None);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(
        Arc::make_mut(&mut e.domain).events.as_mut().unwrap()
    )
    .next_id += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(
        Arc::make_mut(&mut e.domain).events.as_mut().unwrap()
    )
    .events[1]
        .source_start += 1);
    change!(
        |e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain)
            .source_gain
            .push(SourceGainPoint {
                frame: 0,
                gain_q16: 42
            })
    );
    change!(
        |e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).warp = Some(WarpSettings {
            mode: WarpMode::RePitch,
            markers: vec![]
        })
    );
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).warp_enabled = true);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).pitch_cents = 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).source_offset += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).source_length += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).source_phase += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).looping = true);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).clock =
        AudioDomainClock::Beats(BeatTime::ONE));
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).sample_bpm = Some(Tempo::DEFAULT));
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).fade_start += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain).fade =
        ClipFade::new(100, 0, 3, 4).unwrap());
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain)
        .fade_projection
        .as_mut()
        .unwrap()
        .tempo =
        TempoMap::constant(44_100, Tempo::DEFAULT).unwrap());
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain)
        .fade_projection
        .as_mut()
        .unwrap()
        .local_start_raw += 1);
    change!(|e: &mut AudioClipEdit| Arc::make_mut(&mut e.domain)
        .fade_projection
        .as_mut()
        .unwrap()
        .origin_beat_raw += 1);
    for c in variants {
        assert_eq!(c.matches_content(&saved), c == original);
    }
    let mut none = original.clone();
    Arc::make_mut(&mut edit(&mut none).domain).events = None;
    let saved_none = token(&none);
    Arc::make_mut(&mut edit(&mut none).domain).events = Some(Arc::new(AudioEvents {
        next_id: 1,
        events: vec![],
    }));
    assert!(!none.matches_content(&saved_none));
    let mut warped = original.clone();
    Arc::make_mut(&mut edit(&mut warped).domain).warp = Some(WarpSettings {
        mode: WarpMode::RePitch,
        markers: vec![
            WarpMarker {
                source_frame: 0,
                beat: BeatTime::ZERO,
            },
            WarpMarker {
                source_frame: 100,
                beat: BeatTime::ONE,
            },
        ],
    });
    let saved_warp = token(&warped);
    let mut marker_change = warped.clone();
    Arc::make_mut(&mut edit(&mut marker_change).domain)
        .warp
        .as_mut()
        .unwrap()
        .markers[1]
        .source_frame += 1;
    assert!(!marker_change.matches_content(&saved_warp));
    Arc::make_mut(&mut edit(&mut warped).domain)
        .warp
        .as_mut()
        .unwrap()
        .mode = WarpMode::Complex;
    assert!(!warped.matches_content(&saved_warp));
}
#[test]
fn midi_token_preserves_every_note_field_source_order_and_capacity_refusal_is_atomic() {
    let original = MidiClip {
        name: "midi".into(),
        flavor: ClipFlavor::default(),
        id: ClipId(5),
        start: BeatTime::ZERO,
        length: BeatTime::ONE,
        source: Arc::new(MidiSource {
            source_length: BeatTime::ONE,
            source_start: BeatTime::ZERO,
            loop_start: BeatTime::ZERO,
            loop_end: BeatTime::ONE,
            looping: false,
            notes: vec![MidiNote {
                id: NoteId(7),
                start: BeatTime::ZERO,
                duration: BeatTime::ONE,
                pitch: 60,
                channel: 0,
                velocity: 100,
            }],
        }),
    };
    let mut saved =
        ClipContentToken::reserve_on_control(original.content_token_capacity().unwrap()).unwrap();
    original.capture_content(&mut saved).unwrap();
    let mut variants = vec![original.clone()];
    macro_rules! change {
        ($body:expr) => {{
            let mut c = original.clone();
            ($body)(&mut c);
            variants.push(c);
        }};
    }
    change!(|c: &mut MidiClip| c.name.push('x'));
    change!(|c: &mut MidiClip| c.flavor = c.flavor.toggled());
    change!(|c: &mut MidiClip| c.id = ClipId(6));
    change!(|c: &mut MidiClip| c.start = BeatTime::ONE);
    change!(|c: &mut MidiClip| c.length = BeatTime::ZERO);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).source_length = BeatTime::ZERO);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).source_start = BeatTime::ONE);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).loop_start = BeatTime::ONE);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).loop_end = BeatTime::ZERO);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).looping = true);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].id = NoteId(8));
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].start = BeatTime::ONE);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].duration = BeatTime::ZERO);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].pitch = 61);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].channel = 1);
    change!(|c: &mut MidiClip| Arc::make_mut(&mut c.source).notes[0].velocity = 99);
    for c in variants {
        assert_eq!(c.matches_content(&saved), c == original);
    }
    let mut too_large = original.clone();
    too_large.name.push_str(&"x".repeat(100));
    assert!(too_large.capture_content(&mut saved).is_err());
    assert!(original.matches_content(&saved));
    assert!(!audio_fixture().matches_content(&saved));
    let mut ordered = original.clone();
    let source = Arc::make_mut(&mut ordered.source);
    let mut next = source.notes[0].clone();
    next.id = NoteId(8);
    source.notes.push(next);
    let mut ordered_token =
        ClipContentToken::reserve_on_control(ordered.content_token_capacity().unwrap()).unwrap();
    ordered.capture_content(&mut ordered_token).unwrap();
    Arc::make_mut(&mut ordered.source).notes.reverse();
    assert!(!ordered.matches_content(&ordered_token));
}
