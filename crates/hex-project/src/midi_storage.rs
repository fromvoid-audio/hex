//! Borrowed observations of the closed canonical sized-Arc MIDI family.
//! All crate-private constructors and COW paths must preserve this invariant:
//! sized global Arc::new, Arc::clone, Arc::make_mut/get_mut only. No raw/custom
//! allocator/unsized/Weak ingress. See docs/native-midi-storage.md for the audit.
use crate::{BeatTime, MidiClip, MidiNote, MidiSource, MidiSourceIdentity, SessionMidiClip};
use std::{alloc::Layout, sync::Arc};

/// An observation of an ACTUAL canonical owning field, never an owning handle
/// or a constructor receipt. No public constructor, pointer, Arc, Weak or mutable
/// reference escapes. The borrow prevents retaining observations across mutation
/// or movement of that field. Counts alone do not freeze other owning fields,
/// prove provenance, construction peak, publication or retirement.
///
/// ```compile_fail
/// use hex_project::{MidiSource, MidiSourceStorageObservation};
/// fn raw_ingress(raw: std::sync::Arc<MidiSource>) {
///     let _ = MidiSourceStorageObservation::from(raw);
/// }
/// ```
/// ```compile_fail
/// fn stale_view(mut clip: hex_project::MidiClip) {
///     let view = clip.source_storage_on_control();
///     let replacement = (*clip).clone();
///     *clip = replacement;
///     let _ = view.notes_capacity();
/// }
/// ```
pub struct MidiSourceStorageObservation<'a> {
    owner: &'a Arc<MidiSource>,
}
impl MidiSourceStorageObservation<'_> {
    /// Allocation identity, independent of equal contents and physical fields.
    pub fn shares_source_with(&self, other: &Self) -> bool {
        Arc::ptr_eq(self.owner, other.owner)
    }
    /// One physical Arc field, even if reached through two enclosing views.
    pub fn is_same_owning_field_as(&self, other: &Self) -> bool {
        std::ptr::eq(self.owner, other.owner)
    }
    /// Fixed body including its embedded Vec header, NOT the notes backing.
    /// This is not an Arc header/layout declaration; the consumer must separately
    /// qualify BOTH sized Arc::new and make_mut for its native standard library.
    pub fn body_layout(&self) -> Layout {
        // Exhaustive fields and scalar types guard against unnoticed new owners.
        let MidiSource {
            source_length,
            source_start,
            loop_start,
            loop_end,
            looping,
            notes,
        } = self.owner.as_ref();
        let _: (&BeatTime, &BeatTime, &BeatTime, &BeatTime, &bool) =
            (source_length, source_start, loop_start, loop_end, looping);
        let _: &Vec<MidiNote> = notes;
        Layout::new::<MidiSource>()
    }
    pub fn notes_capacity(&self) -> usize {
        self.owner.notes.capacity()
    }
    pub fn note_element_layout(&self) -> Layout {
        // Exhaustive scalar check also compiles for an empty notes vector.
        if let Some(MidiNote {
            id,
            start,
            duration,
            pitch,
            channel,
            velocity,
        }) = self.owner.notes.first()
        {
            let _: (&crate::NoteId, &BeatTime, &BeatTime, &u8, &u8, &u8) =
                (id, start, duration, pitch, channel, velocity);
        }
        Layout::new::<MidiNote>()
    }
    /// Closure observations only. These neither mint nor upgrade provenance.
    pub fn owning_counts(&self) -> (usize, usize) {
        (Arc::strong_count(self.owner), Arc::weak_count(self.owner))
    }
}
impl MidiClip {
    pub fn source_storage_on_control(&self) -> MidiSourceStorageObservation<'_> {
        MidiSourceStorageObservation {
            owner: &self.source,
        }
    }
}
impl SessionMidiClip {
    pub fn source_storage_on_control(&self) -> MidiSourceStorageObservation<'_> {
        MidiSourceStorageObservation {
            owner: &self.source,
        }
    }
}
impl MidiSourceIdentity {
    pub fn source_storage_on_control(&self) -> MidiSourceStorageObservation<'_> {
        MidiSourceStorageObservation { owner: &self.0 }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unique_deref_mut_and_public_value_replacement_reobserve_actual_backing() {
        let mut clip = MidiClip {
            name: String::new(),
            flavor: crate::ClipFlavor::default(),
            id: crate::ClipId(1),
            start: BeatTime::ZERO,
            length: BeatTime::from_raw(4),
            source: Arc::new(MidiSource::defaults(BeatTime::from_raw(4))),
        };
        let original = Arc::as_ptr(&clip.source);
        let mut replacement = (*clip).clone();
        replacement.notes = Vec::with_capacity(17);
        *clip = replacement;
        assert_eq!(original, Arc::as_ptr(&clip.source)); // unique make_mut retains outer
        assert_eq!(clip.source_storage_on_control().notes_capacity(), 17);
        let identity = clip.source_identity();
        let mut replacement = (*clip).clone();
        replacement.notes = Vec::with_capacity(23);
        *clip = replacement; // shared make_mut clones outer before value replacement
        assert!(
            !clip
                .source_storage_on_control()
                .shares_source_with(&identity.source_storage_on_control())
        );
        assert_eq!(identity.source_storage_on_control().notes_capacity(), 17);
        assert_eq!(clip.source_storage_on_control().notes_capacity(), 23);
        assert!(Arc::get_mut(&mut clip.source).is_some());
        let session = SessionMidiClip {
            name: String::new(),
            flavor: clip.flavor,
            id: crate::ClipId(2),
            source: Arc::clone(&clip.source),
        };
        assert!(
            clip.source_storage_on_control()
                .shares_source_with(&session.source_storage_on_control())
        );
        assert!(
            !clip
                .source_storage_on_control()
                .is_same_owning_field_as(&session.source_storage_on_control())
        );
    }
}
