use super::*;
use crate::midi_clip_volume::VolumePlacement;
use hex_project::{MidiClipGain, MidiClipVolume, MidiClipVolumePoint};

fn config(project: &Project, old: &PreparedSessionMidiClip) -> PreparedTrackInstrumentConfig {
    let device = project.track(old.track).unwrap().instrument().unwrap();
    PreparedTrackInstrumentConfig {
        track: old.track,
        config: Some(
            hex_devices::prepare_legacy_sine_instrument(project.sample_rate())
                .unwrap()
                .into(),
        ),
        mutable_limits: None,
        device_id: Some(device.id()),
        tuning_identity: old.tuning_identity,
        state: Some(device.state().clone().without_host_gains()),
        bypassed: false,
        note_bank: Some(Arc::clone(old.instrument.note_bank())),
        mutable_runtime: None,
        resource_factory_inline_bound: None,
        history_retarget_target: None,
        history_retarget_requires_quiescence: false,
    }
}
fn curve() -> MidiClipVolume {
    MidiClipVolume::new(
        vec![
            MidiClipVolumePoint::linear(beat(0), MidiClipGain::new(0.0).unwrap()),
            MidiClipVolumePoint::linear(beat(4), MidiClipGain::new(2.0).unwrap()),
        ],
        beat(4),
    )
    .unwrap()
}
fn prepare(
    project: &Project,
    inventory: &PreparedSessionInventory,
    config: &PreparedTrackInstrumentConfig,
    old: &PreparedSessionMidiClip,
    timing: &SessionTimingView<'_>,
) -> PreparedSessionMidiClip {
    let cell = project
        .session_cells()
        .iter()
        .find(
            |cell| matches!(cell.content(), SessionCellContent::Midi(clip) if clip.id() == old.id),
        )
        .unwrap();
    let SessionCellContent::Midi(clip) = cell.content() else {
        panic!("MIDI fixture");
    };
    prepare_clip(
        old.track,
        cell,
        clip,
        config,
        timing,
        inventory.shared_contexts(),
        Some(old),
    )
    .unwrap()
}

#[test]
fn volume_revision_replaces_only_program_and_charges_each_prepared_owner() {
    let (mut editor, [track, _, _], id, _) = fixture();
    let table = Arc::new(SineTable::new());
    let inventory =
        PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), None).unwrap();
    let old = inventory.midi_clip(id).unwrap();
    assert!(old.volume_program().is_none());
    let config = config(editor.project(), old);
    let timing = SessionTimingView::Canonical(inventory.timing());
    let volume = curve();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: id,
            volume: volume.clone(),
        })
        .unwrap();
    let public =
        PreparedSessionInventory::prepare(editor.project(), Arc::clone(&table), Some(&inventory))
            .unwrap();
    let admitted = public.midi_clip(id).unwrap();
    assert!(
        admitted
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    assert!(Arc::ptr_eq(&admitted.source, &old.source));
    assert!(Arc::ptr_eq(&admitted.instrument, &old.instrument));
    let first = prepare(editor.project(), &inventory, &config, old, &timing);
    assert!(first.source_content_matches(old));
    assert!(first.instrument_config_matches(old));
    assert!(first.instrument_retarget_compatible(old));
    assert!(Arc::ptr_eq(&first.source, &old.source));
    assert!(Arc::ptr_eq(&first.instrument, &old.instrument));
    assert_eq!(pcm(&inventory, &first), pcm(&inventory, old));
    assert!(
        first
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    assert_eq!(first.track, track);
    let unchanged = prepare(editor.project(), &inventory, &config, &first, &timing);
    assert!(
        unchanged
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    let changed = MidiClipVolume::new(
        vec![MidiClipVolumePoint::linear(
            beat(0),
            MidiClipGain::new(0.5).unwrap(),
        )],
        beat(4),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: id,
            volume: changed.clone(),
        })
        .unwrap();
    let second = prepare(editor.project(), &inventory, &config, &first, &timing);
    assert!(
        second
            .volume_program()
            .unwrap()
            .source_revision_matches(&changed)
    );
    assert!(
        !second
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    assert!(second.source_content_matches(&first));
    assert!(Arc::ptr_eq(&second.instrument, &first.instrument));
    let mut bytes = 0;
    let mut references = 0;
    account_template(&first, &mut bytes, &mut references).unwrap();
    assert_eq!(
        bytes,
        std::mem::size_of::<Arc<crate::midi_note_bank::PreparedMidiNoteBank>>()
            + volume.retained_control_metadata_bytes().unwrap()
    );
    let once = bytes;
    account_template(&unchanged, &mut bytes, &mut references).unwrap();
    assert_eq!(
        bytes,
        2 * once,
        "shared Arc is conservatively charged per owner"
    );
    let mut overflowing = usize::MAX;
    assert!(account_template(&first, &mut overflowing, &mut 0).is_err());
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: id,
            volume: MidiClipVolume::default(),
        })
        .unwrap();
    let unity = prepare(editor.project(), &inventory, &config, &second, &timing);
    assert!(unity.volume_program().is_none());
    assert!(unity.source_content_matches(&second));
    assert!(Arc::ptr_eq(&unity.instrument, &second.instrument));
}

#[test]
fn console_volume_program_uses_actual_periodic_timing_not_canonical_fallback() {
    let (mut editor, _, id, _) = fixture();
    let inventory =
        PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
            .unwrap();
    let old = inventory.midi_clip(id).unwrap();
    let config = config(editor.project(), old);
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: id,
            volume: curve(),
        })
        .unwrap();
    let periodic = PreparedPeriodicSessionTempo::prepare(
        inventory.timing(),
        hex_project::LoopRegion::new(0, 48000, true).unwrap(),
    )
    .unwrap();
    let timing = SessionTimingView::Periodic(periodic.view(inventory.timing()));
    let prepared = prepare(editor.project(), &inventory, &config, old, &timing);
    let ramp = prepared
        .volume_program()
        .unwrap()
        .segment_at(
            &timing,
            VolumePlacement::Session {
                origin: crate::SessionMidiOrigin::launch(beat(0)),
            },
            48050,
            1,
        )
        .unwrap()
        .unwrap();
    assert_eq!((ramp.start_frame, ramp.end_frame), (48000, 96000));
    assert_eq!((ramp.start_gain, ramp.end_gain), (1.0, 2.0));
    assert!(prepared.source_content_matches(old));
}

#[test]
fn changed_effective_tempo_density_is_readmitted_even_with_same_curve_and_source() {
    let (mut editor, _, id, _) = fixture();
    // Exactly 16,384 frames/beat at 48 kHz: the one-frame global loop
    // changes boundary density without changing the admitted maximum BPM.
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(175_781_250).unwrap(),
        })
        .unwrap();
    let loop_end = BeatTime::from_raw(BeatTime::ONE.raw() / 64);
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip: id,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end,
            looping: true,
        })
        .unwrap();
    let inventory =
        PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
            .unwrap();
    let old = inventory.midi_clip(id).unwrap();
    let config = config(editor.project(), old);
    let volume = MidiClipVolume::new(
        (0..hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP)
            .map(|index| {
                MidiClipVolumePoint::linear(
                    BeatTime::from_raw(
                        loop_end.raw() * index as u64
                            / hex_project::MAX_MIDI_VOLUME_POINTS_PER_CLIP as u64,
                    ),
                    MidiClipGain::new((index % 2) as f64).unwrap(),
                )
            })
            .collect(),
        beat(4),
    )
    .unwrap();
    editor
        .execute(Command::ReplaceMidiClipVolume {
            clip: id,
            volume: volume.clone(),
        })
        .unwrap();
    let canonical = SessionTimingView::Canonical(inventory.timing());
    let first = prepare(editor.project(), &inventory, &config, old, &canonical);
    assert!(
        first
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    let periodic = PreparedPeriodicSessionTempo::prepare(
        inventory.timing(),
        hex_project::LoopRegion::new(0, 1, true).unwrap(),
    )
    .unwrap();
    let timing = SessionTimingView::Periodic(periodic.view(inventory.timing()));
    assert_eq!(timing.max_micro_bpm(), canonical.max_micro_bpm());
    let cell = editor
        .project()
        .session_cells()
        .iter()
        .find(|cell| matches!(cell.content(), SessionCellContent::Midi(clip) if clip.id() == id))
        .unwrap();
    let SessionCellContent::Midi(clip) = cell.content() else {
        panic!("MIDI fixture");
    };
    assert!(first.source_identity == clip.source_identity());
    assert!(matches!(
        prepare_clip(
            first.track,
            cell,
            clip,
            &config,
            &timing,
            inventory.shared_contexts(),
            Some(&first)
        ),
        Err(crate::Invalid("clip volume density exceeds capacity"))
    ));
    // Refusal does not mutate or replace the previous prepared owner.
    assert!(
        first
            .volume_program()
            .unwrap()
            .source_revision_matches(&volume)
    );
    assert!(first.source_content_matches(old));
    assert!(Arc::ptr_eq(&first.instrument, &old.instrument));
}

#[test]
fn voice_gain_preparation_rechecks_effective_tempo_density() {
    let (mut editor, _, id, _) = fixture();
    editor
        .execute(Command::SetTempoPoint {
            beat: BeatTime::ZERO,
            tempo: hex_project::Tempo::new(175_781_250).unwrap(),
        })
        .unwrap();
    let loop_end = BeatTime::from_raw(BeatTime::ONE.raw() / 64);
    editor
        .execute(Command::SetSessionMidiClipSource {
            clip: id,
            source_length: beat(4),
            source_start: beat(0),
            loop_start: beat(0),
            loop_end,
            looping: true,
        })
        .unwrap();
    let mut inventory =
        PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
            .unwrap();
    let cell = editor
        .project()
        .session_cells()
        .iter()
        .find(|cell| matches!(cell.content(), SessionCellContent::Midi(clip) if clip.id() == id))
        .unwrap();
    let SessionCellContent::Midi(clip) = cell.content() else {
        panic!("MIDI fixture")
    };
    let source = clip
        .source()
        .playback_for_duration(beat(16))
        .unwrap()
        .with_playback_offset(clip.playback_offset())
        .unwrap();
    let curve = MidiClipVolume::new(
        (0..1023)
            .map(|index| {
                MidiClipVolumePoint::linear(
                    BeatTime::from_raw(loop_end.raw() * index as u64 / (2 * 1023)),
                    MidiClipGain::new((index % 2) as f64).unwrap(),
                )
            })
            .collect(),
        beat(4),
    )
    .unwrap();
    let gain = hex_project::MidiVoiceGain::new(vec![
        hex_project::MidiVoiceGainLayer::new(
            beat(0),
            BeatTime::from_raw(loop_end.raw() / 2),
            beat(4),
            curve,
        )
        .unwrap(),
    ])
    .unwrap();
    let note = clip.source().notes()[0].id();
    let voice = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        note,
        source,
        &gain,
        inventory.timing(),
    )
    .unwrap();
    inventory
        .midi_clips
        .iter_mut()
        .find(|clip| clip.id == id)
        .unwrap()
        .voice_gains = Some(voice);
    let canonical = SessionTimingView::Canonical(inventory.timing());
    let first = crate::midi_clip_volume::voice::PreparedVoiceGain::prepare_layers(
        gain.layers(),
        source,
        &canonical,
    )
    .unwrap();
    let witness = first.preparation_identity().unwrap();
    let periodic = PreparedPeriodicSessionTempo::prepare(
        inventory.timing(),
        hex_project::LoopRegion::new(0, 1, true).unwrap(),
    )
    .unwrap();
    let timing = SessionTimingView::Periodic(periodic.view(inventory.timing()));
    assert_eq!(timing.max_micro_bpm(), canonical.max_micro_bpm());
    assert!(matches!(
        crate::midi_clip_volume::voice::PreparedVoiceGain::prepare_layers(
            gain.layers(),
            source,
            &timing,
        ),
        Err(crate::Invalid("clip volume density exceeds capacity"))
    ));
    assert!(witness.matches(&first));
    let old = inventory.midi_clip(id).unwrap().voice_gains().unwrap();
    let old_identity = old.preparation_identity().unwrap();
    let quote = inventory
        .section_admission_quote(timing.max_micro_bpm())
        .unwrap();
    assert!(matches!(
        inventory.prepare_section_admission(&timing, quote.0 + quote.1),
        Err(crate::Invalid("clip volume density exceeds capacity"))
    ));
    assert!(old_identity.matches(inventory.midi_clip(id).unwrap().voice_gains().unwrap()));
}

#[test]
fn section_readmission_selects_a_new_voice_owner_even_at_the_same_maximum_tempo() {
    let (editor, _, id, _) = fixture();
    let mut inventory =
        PreparedSessionInventory::prepare(editor.project(), Arc::new(SineTable::new()), None)
            .unwrap();
    let cell = editor
        .project()
        .session_cells()
        .iter()
        .find(|cell| matches!(cell.content(), SessionCellContent::Midi(clip) if clip.id() == id))
        .unwrap();
    let SessionCellContent::Midi(clip) = cell.content() else {
        panic!("MIDI fixture")
    };
    let note = clip.source().notes()[0].id();
    let source = clip.source().playback_for_duration(beat(16)).unwrap();
    let gain = hex_project::MidiVoiceGain::new(vec![
        hex_project::MidiVoiceGainLayer::new(beat(0), beat(4), beat(4), curve()).unwrap(),
    ])
    .unwrap();
    let voice = crate::midi_clip_volume::voice::PreparedSessionVoiceGains::test_voice(
        note,
        source,
        &gain,
        inventory.timing(),
    )
    .unwrap();
    inventory
        .midi_clips
        .iter_mut()
        .find(|clip| clip.id == id)
        .unwrap()
        .voice_gains = Some(voice);
    let periodic = PreparedPeriodicSessionTempo::prepare(
        inventory.timing(),
        hex_project::LoopRegion::new(0, 12000, true).unwrap(),
    )
    .unwrap();
    let timing = SessionTimingView::Periodic(periodic.view(inventory.timing()));
    assert_eq!(
        timing.max_micro_bpm(),
        inventory.source_admission_max_micro_bpm
    );
    let quote = inventory
        .section_admission_quote(timing.max_micro_bpm())
        .unwrap();
    assert!(quote.0 > 0);
    let admission = inventory
        .prepare_section_admission(&timing, quote.0 + quote.1)
        .unwrap()
        .unwrap();
    assert!(admission.retained_bytes().unwrap() <= quote.0);
    let base = inventory.midi_clip(id).unwrap().voice_gains().unwrap();
    let effective =
        super::super::super::readmission::EffectiveSessionMidi::new(&inventory, Some(&admission))
            .midi_clip(id)
            .unwrap();
    assert!(!std::ptr::eq(base, effective.voice_gains().unwrap()));
    assert!(effective.work().voice_gain_factor_work > 0);
    assert_eq!(
        effective.work().voice_gain_factor_work,
        effective.work().span_hits * crate::midi_clip_volume::VOLUME_QUERY_FRAMES * 6
    );
}
