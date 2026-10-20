use super::*;
use crate::{AudioRootStorageLedger, AudioStorageLedgerReservation};
use hex_project::{BeatTime, Command, Created, Editor, Project};
use std::{alloc::Layout, collections::BTreeMap, sync::atomic::AtomicUsize};

fn actual_model() -> Editor {
    let mut editor = Editor::new(Project::new(48000).unwrap(), 8).unwrap();
    let Created::Track(track) = editor
        .execute(Command::AddMidiTrack {
            name: "preview".into(),
        })
        .unwrap()
    else {
        panic!("track")
    };
    let Created::Clip(clip) = editor
        .execute(Command::PlaceMidiClip {
            track,
            start: BeatTime::ZERO,
            length: BeatTime::ONE,
        })
        .unwrap()
    else {
        panic!("clip")
    };
    editor
        .execute(Command::AddMidiNote {
            clip,
            start: BeatTime::ZERO,
            duration: BeatTime::ONE,
            pitch: 60,
            channel: 0,
            velocity: 100,
        })
        .unwrap();
    editor
}
fn actual_preview() -> MidiArrangementPreview {
    let editor = actual_model();
    let (_, preview) =
        crate::compile::project_with_midi_preview(editor.project(), &BTreeMap::new()).unwrap();
    preview
}

fn custody(preview: MidiArrangementPreview) -> PreviewCustody {
    // Direct test placement in the actual existing persistent field exercises
    // inventory/transfer/restore/disposal only. Production reserve remains
    // audio-only until the separately prequoted closed compiler entry exists.
    PreviewCustody {
        preview: Some(preview),
        handle: crate::audio_only_inputs::handle().unwrap(),
        phase: Phase::Installed,
        ..Default::default()
    }
}
fn budget() -> AudioStorageLedgerReservation {
    AudioStorageLedgerReservation {
        data_rows: 16,
        handle_rows: 8,
        root_rows: 4,
        max_retained_bytes: 1_000_000,
        max_construction_peak_bytes: 1_000_000,
    }
}
fn requested_slice(rows: usize) -> usize {
    Layout::array::<AtomicUsize>(2)
        .unwrap()
        .extend(Layout::array::<MidiClipPreview>(rows).unwrap())
        .unwrap()
        .0
        .pad_to_align()
        .size()
}
#[test]
fn nonempty_preview_actual_compiler_transfer_restore_inventory_and_disposal() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported nonempty preview constructor; no positive evidence");
        return;
    }
    eprintln!(
        "SUPPORTED actual compiler preview persistent transfer/restore/inventory/control disposal"
    );
    let preview = actual_preview();
    assert_eq!(preview.clips().len(), 1);
    assert_eq!(preview.clips()[0].samples.len(), 1);
    assert_eq!(preview.clips()[0].samples[0].pitch, 60);
    let expected = requested_slice(1)
        + preview.clips()[0].samples.capacity() * size_of::<super::super::MidiPreviewSample>();
    let pointer = preview.clips.arc().as_ptr();
    let mut slot = custody(preview);
    let initial = slot.capacity().unwrap();
    assert_eq!(initial.bytes().unwrap(), expected);
    assert_eq!(initial.unresolved_allocations, 2);
    let collect = |slot: &PreviewCustody, external: Option<&MidiArrangementPreview>, limit| {
        let mut ledger = AudioRootStorageLedger::try_new_on_control(limit)?;
        slot.visit_audio_storage(&mut ledger, external)?;
        slot.visit_audio_storage(&mut ledger, external)?;
        ledger.validate_selected_shared_handles_on_control()?;
        ledger.summary()
    };
    let summary = collect(&slot, None, budget()).unwrap();
    assert_eq!(summary.data_bytes, expected);
    assert_eq!(summary.data_allocations, 2);
    assert!(summary.require_complete().is_err()); // no retirement/publication proof
    let mut duplicate = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    slot.visit_audio_storage(&mut duplicate, None).unwrap();
    let owner = &slot.preview.as_ref().unwrap().clips;
    assert_eq!(
        duplicate.shared(
            owner.arc(),
            owner.parts(),
            crate::audio_storage::Owner::exclusive(owner),
            slot.handle,
            None
        ),
        Err(Error::SourceMismatch)
    );
    drop(duplicate);
    let mut exact = budget();
    exact.max_retained_bytes = summary.retained_bytes;
    exact.max_construction_peak_bytes = summary.construction_peak_bytes;
    assert_eq!(collect(&slot, None, exact).unwrap(), summary);
    exact.max_retained_bytes -= 1;
    assert_eq!(collect(&slot, None, exact), Err(Error::ReservationExceeded));
    let mut target = None;
    assert!(slot.can_transfer_revision());
    slot.transfer_revision(&mut target);
    assert_eq!(slot.capacity_at(target.as_ref()).unwrap(), initial);
    assert_eq!(
        collect(&slot, target.as_ref(), budget())
            .unwrap()
            .data_bytes,
        expected
    );
    assert!(std::ptr::eq(
        target.as_ref().unwrap().clips.arc().as_ptr(),
        pointer
    ));
    assert!(slot.can_restore_revision());
    slot.restore_revision(&mut target);
    assert!(target.is_none());
    assert_eq!(slot.capacity().unwrap(), initial);
    slot.dispose_next().unwrap(); // preview -> same persistent slice destination
    assert!(slot.preview.is_none());
    assert_eq!(slot.capacity().unwrap(), initial);
    slot.dispose_next().unwrap(); // exact closed scalar payload released on control
    assert!(!slot.has_next());
    assert_eq!(slot.capacity().unwrap().bytes().unwrap(), 0);
    slot.clear_disposed().unwrap();
    assert_eq!(slot.handle, 0);
}
#[test]
fn nonempty_preview_reserved_sample_backing_and_unknown_outer_remain_distinct() {
    if !crate::audio_storage_constructor_layout_supported() {
        eprintln!("unsupported nonempty preview constructor; no positive evidence");
        return;
    }
    eprintln!("SUPPORTED nonempty preview reserved empty samples and untracked outer refusal");
    let preview = actual_preview();
    let mut rows = preview.clips().to_vec();
    rows[0].samples = Vec::with_capacity(19); // empty live backing still counts
    let slot = custody(MidiArrangementPreview::from_sorted_clips(rows));
    let expected_samples = 19 * size_of::<super::super::MidiPreviewSample>();
    assert_eq!(
        slot.capacity().unwrap().samples_backing_bytes,
        expected_samples
    );
    let mut alias = custody(slot.preview.as_ref().unwrap().clone());
    let mut ledger = AudioRootStorageLedger::try_new_on_control(budget()).unwrap();
    slot.visit_audio_storage(&mut ledger, None).unwrap();
    assert!(
        ledger
            .validate_selected_shared_handles_on_control()
            .is_err()
    );
    alias.visit_audio_storage(&mut ledger, None).unwrap();
    alias.visit_audio_storage(&mut ledger, None).unwrap();
    ledger
        .validate_selected_shared_handles_on_control()
        .unwrap();
    assert_eq!(
        ledger.summary().unwrap().data_bytes,
        requested_slice(1) + expected_samples
    );
    assert_eq!(ledger.summary().unwrap().data_allocations, 2);
    assert_eq!(ledger.selected_shared_counts_on_control(), (1, 2));
    drop(ledger);
    drop(slot);
    assert_eq!(
        alias.capacity().unwrap().samples_backing_bytes,
        expected_samples
    );
    alias.dispose_next().unwrap();
    alias.dispose_next().unwrap();
    alias.clear_disposed().unwrap();
    let unknown = custody(MidiArrangementPreview::default());
    assert_eq!(unknown.capacity(), Err(Error::UnsupportedConstructor));
    // Typed inspection does not authorize a new nonempty constructor path.
    let mut empty = PreviewCustody::default();
    assert_eq!(
        empty.reserve(actual_model().project()),
        Err(Error::UnsupportedGraph)
    );
}
