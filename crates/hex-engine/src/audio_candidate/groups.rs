//! Original177 owner_groups.rs provenance for actual closed runtime owners.
//! Planning changes scalar scratch only. Ownership and origins commit together
//! at the existing Session chain/tail and compensation move frontiers.
use super::{AudioCandidateIdentity, AudioStorageError, Group};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Location {
    TrackChain(usize),
    TailChain(usize),
    ReturnChain(usize),
    ReturnTail(usize),
    MainChain,
    Compensation,
}
impl Location {
    fn tuple(self) -> (u8, usize) {
        match self {
            Self::TrackChain(i) => (0, i),
            Self::TailChain(i) => (1, i),
            Self::ReturnChain(i) => (4, i),
            Self::ReturnTail(i) => (5, i),
            Self::MainChain => (14, 0),
            Self::Compensation => (8, 0),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Origin {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) member: usize,
}
// A row is a scalar association, never a copied musical owner.
#[derive(Clone, Copy)]
pub(super) struct TransferRow {
    source: usize,
    destination: usize,
    origin: Origin,
    swap: bool,
    displaced: Option<Origin>,
}
pub(crate) struct Transfer<'a> {
    candidate: &'a mut [Group],
    previous: &'a mut [Group],
    journal: &'a mut Vec<TransferRow>,
    runtime_bytes: &'a mut usize,
    planned_runtime_bytes: usize,
}
impl<'a> Transfer<'a> {
    pub(super) fn new(
        candidate: &'a mut [Group],
        previous: &'a mut [Group],
        journal: &'a mut Vec<TransferRow>,
        runtime_bytes: &'a mut usize,
    ) -> Self {
        journal.clear();
        let planned_runtime_bytes = *runtime_bytes;
        Self {
            candidate,
            previous,
            journal,
            runtime_bytes,
            planned_runtime_bytes,
        }
    }
    pub(crate) fn plan(
        &mut self,
        source: Location,
        destination: Location,
        actual_chain: &hex_lattice::NativeRuntimeChain,
    ) -> Result<(), crate::SessionPlaybackError> {
        let invalid = crate::SessionPlaybackError::ReleaseBankFull;
        let source = self
            .previous
            .iter()
            .position(|g| g.location == source.tuple())
            .ok_or(invalid)?;
        let destination = self
            .candidate
            .iter()
            .position(|g| g.location == destination.tuple())
            .ok_or(invalid)?;
        let src = &self.previous[source];
        let dst = &self.candidate[destination];
        let origin = src.origin.ok_or(invalid)?;
        if dst.origin.is_some()
            || dst.present
            || src.closed
            || dst.closed
            || self.journal.len() == self.journal.capacity()
            || self
                .journal
                .iter()
                .any(|r| r.source == source || r.destination == destination)
        {
            return Err(invalid);
        }
        // Both destination inline slots were reserved at construction. Only
        // the real moved entry backing and its concrete twin Boxes add to the
        // current runtime; shared prepared DATA remains in original anchors.
        let view = actual_chain.storage_view();
        let mut bytes = view.entry_capacity_bytes().ok_or(invalid)?;
        for entry in view.entries() {
            bytes = bytes
                .checked_add(entry.automation().storage_bytes().ok_or(invalid)?)
                .and_then(|bytes| bytes.checked_add(entry.modulation_storage_bytes()?))
                .ok_or(invalid)?;
            if let Some((a, b)) =
                crate::native_effect_storage::pair_bytes(&entry).map_err(|_| invalid)?
            {
                bytes = bytes
                    .checked_add(a)
                    .and_then(|n| n.checked_add(b))
                    .ok_or(invalid)?;
            }
        }
        let next_bytes = self
            .planned_runtime_bytes
            .checked_add(bytes)
            .ok_or(invalid)?;
        if next_bytes > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(invalid);
        }
        self.journal.push(TransferRow {
            source,
            destination,
            origin,
            swap: false,
            displaced: None,
        });
        self.planned_runtime_bytes = next_bytes;
        Ok(())
    }
    /// The existing clock path swaps whole original owners, including a
    /// possibly occupied recipient. Record the displaced origin as well; both
    /// anchor books already participate in normal disposal fanout.
    pub(crate) fn plan_compensation_swap(
        &mut self,
        old: &crate::signal_graph::SignalRuntime,
        current: Option<&crate::signal_graph::SignalRuntime>,
        graph: &std::sync::Arc<crate::signal_graph::SignalGraph>,
    ) -> Result<(), crate::SessionPlaybackError> {
        let invalid = crate::SessionPlaybackError::ReleaseBankFull;
        let location = Location::Compensation.tuple();
        let source = self
            .previous
            .iter()
            .position(|g| g.location == location)
            .ok_or(invalid)?;
        let destination = self
            .candidate
            .iter()
            .position(|g| g.location == location)
            .ok_or(invalid)?;
        let src = &self.previous[source];
        let dst = &self.candidate[destination];
        if src.closed
            || dst.closed
            || !src.present
            || dst.present != current.is_some()
            || dst.present != dst.origin.is_some()
            || self.journal.len() == self.journal.capacity()
            || self
                .journal
                .iter()
                .any(|r| r.source == source || r.destination == destination)
        {
            return Err(invalid);
        }
        let origin = src.origin.ok_or(invalid)?;
        // Charge the actual adopted storage conservatively on top of the
        // constructor's reservation. No allocation accompanies this transfer;
        // the retired runtime keeps the displaced original storage.
        let bytes = old.storage_bytes_with_graph(graph).ok_or(invalid)?;
        let next_bytes = self
            .planned_runtime_bytes
            .checked_add(bytes)
            .ok_or(invalid)?;
        if next_bytes > crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES {
            return Err(invalid);
        }
        self.journal.push(TransferRow {
            source,
            destination,
            origin,
            swap: true,
            displaced: dst.origin,
        });
        self.planned_runtime_bytes = next_bytes;
        Ok(())
    }
    pub(crate) fn commit(self) {
        for row in self.journal.iter() {
            let source = &mut self.previous[row.source];
            let destination = &mut self.candidate[row.destination];
            if row.swap {
                std::mem::swap(&mut destination.origin, &mut source.origin);
                std::mem::swap(&mut destination.present, &mut source.present);
            } else {
                destination.origin = source.origin.take();
                destination.present = true;
                // Track/Return vector cells retain empty inline shells. Main
                // backing and tail Options become vacant; none retains an origin.
                source.present = matches!(source.location.0, 0 | 4);
            }
        }
        *self.runtime_bytes = self.planned_runtime_bytes;
    }
}
/// A move-only journal from one real coupled handoff. No scalar factory.
pub struct AudioCandidateTransfer {
    pub(super) predecessor: AudioCandidateIdentity,
    pub(super) successor: AudioCandidateIdentity,
    pub(super) rows: Vec<TransferRow>,
}
impl AudioCandidateTransfer {
    pub fn predecessor(&self) -> AudioCandidateIdentity {
        self.predecessor
    }
    pub fn successor(&self) -> AudioCandidateIdentity {
        self.successor
    }
    pub fn moved_groups(&self) -> usize {
        self.rows
            .iter()
            .map(|row| 1 + usize::from(row.displaced.is_some()))
            .sum()
    }
    pub fn moved_groups_from(&self, origin: AudioCandidateIdentity) -> usize {
        self.rows
            .iter()
            .map(|row| {
                usize::from(row.origin.runtime == origin)
                    + usize::from(
                        row.displaced
                            .is_some_and(|displaced| displaced.runtime == origin),
                    )
            })
            .sum()
    }
}
#[derive(Clone, Copy)]
pub(super) struct ArchivedOrigin {
    pub(super) origin: Origin,
    pub(super) closed: bool,
}
pub(super) struct Archive {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) groups: Vec<ArchivedOrigin>,
    pub(super) original_disposed: bool,
}
impl Archive {
    pub(super) fn prepare(
        runtime: AudioCandidateIdentity,
        groups: &[Group],
    ) -> Result<Self, AudioStorageError> {
        let mut rows = Vec::new();
        rows.try_reserve_exact(groups.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        for group in groups {
            if let Some(origin) = group.origin {
                rows.push(ArchivedOrigin {
                    origin,
                    closed: false,
                });
            }
        }
        Ok(Self {
            runtime,
            groups: rows,
            original_disposed: false,
        })
    }
    pub(super) fn all_closed(&self) -> bool {
        self.original_disposed && self.groups.iter().all(|g| g.closed)
    }
}
impl Archive {
    pub(super) fn metadata_bytes(&self) -> Result<usize, AudioStorageError> {
        self.groups
            .capacity()
            .checked_mul(std::mem::size_of::<ArchivedOrigin>())
            .ok_or(AudioStorageError::Overflow)
    }
    pub(super) fn preflight(
        &self,
        proof: &AudioCandidateRuntimeDisposed,
    ) -> Result<(), AudioStorageError> {
        if self.runtime == proof.runtime && self.original_disposed {
            return Err(AudioStorageError::SourceMismatch);
        }
        for group in &proof.groups {
            if let Some(origin) = group.origin.filter(|o| o.runtime == self.runtime)
                && !self.groups.iter().any(|g| g.origin == origin && !g.closed)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        Ok(())
    }
    pub(super) fn commit(&mut self, proof: &AudioCandidateRuntimeDisposed) {
        if self.runtime == proof.runtime {
            self.original_disposed = true;
        }
        for row in &mut self.groups {
            if proof.groups.iter().any(|g| g.origin == Some(row.origin)) {
                row.closed = true;
            }
        }
    }
}
/// Normal actual Runtime Box destructor return. This book retains every
/// remaining constructor origin and its actual backing until atomic ingestion.
/// It closes no descendant and provides no admission/catalog/native authority.
pub struct AudioCandidateRuntimeDisposed {
    pub(super) runtime: AudioCandidateIdentity,
    pub(super) groups: Vec<Group>,
}
impl AudioCandidateRuntimeDisposed {
    pub fn identity(&self) -> AudioCandidateIdentity {
        self.runtime
    }
    pub fn record_anchor_cells_on_control(
        &self,
        anchors: &mut [Option<super::AudioCandidateAnchors>],
    ) -> Result<(), AudioStorageError> {
        self.preflight_archives(anchors.iter().flatten().map(|a| &a.owner.inner.archive))?;
        for anchor in anchors.iter_mut().flatten() {
            anchor.owner.inner.archive.commit(self);
        }
        Ok(())
    }
    fn preflight_archives<'a>(
        &self,
        archives: impl Iterator<Item = &'a Archive> + Clone,
    ) -> Result<(), AudioStorageError> {
        if archives
            .clone()
            .filter(|a| a.runtime == self.runtime)
            .count()
            != 1
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (i, archive) in archives.clone().enumerate() {
            if archives
                .clone()
                .take(i)
                .any(|a| a.runtime == archive.runtime)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for group in &self.groups {
            if let Some(origin) = group.origin
                && !archives.clone().any(|a| a.runtime == origin.runtime)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for archive in archives {
            if archive.runtime == self.runtime
                || self
                    .groups
                    .iter()
                    .any(|g| g.origin.is_some_and(|o| o.runtime == archive.runtime))
            {
                archive.preflight(self)?;
            }
        }
        Ok(())
    }
    /// Atomic exact-origin fanout over existing reserved custody cells. No
    /// temporary reference Vec, owner clone or public scalar closure factory.
    pub fn record_fanout_on_control(
        &self,
        owners: &mut [Option<super::AudioCandidateOwner>],
    ) -> Result<(), AudioStorageError> {
        if owners
            .iter()
            .flatten()
            .filter(|o| o.identity() == self.runtime)
            .count()
            != 1
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        for (i, owner) in owners
            .iter()
            .enumerate()
            .filter_map(|(i, o)| o.as_ref().map(|o| (i, o)))
        {
            if owners[..i]
                .iter()
                .flatten()
                .any(|o| o.identity() == owner.identity())
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for group in &self.groups {
            if let Some(origin) = group.origin
                && !owners
                    .iter()
                    .flatten()
                    .any(|o| o.identity() == origin.runtime)
            {
                return Err(AudioStorageError::SourceMismatch);
            }
        }
        for owner in owners.iter().flatten() {
            if owner.identity() == self.runtime
                || self
                    .groups
                    .iter()
                    .any(|g| g.origin.is_some_and(|o| o.runtime == owner.identity()))
            {
                owner.inner.archive.preflight(self)?;
            }
        }
        for owner in owners.iter_mut().flatten() {
            owner.inner.archive.commit(self);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        AudioArrangementInput, AudioCandidateOwner, AudioPreparationCustody,
        AudioStorageReservation, OwnedAudioInputs, OwnedAudioSource,
    };
    use hex_lattice_contract::{NativeDeviceMetadata, ParameterValue};
    use hex_project::{Command, Created, Editor};
    fn owner(
        project: &hex_project::Project,
    ) -> (
        crate::ClaimedAudioCandidateRuntime,
        crate::AudioCandidateAnchors,
        usize,
        usize,
    ) {
        let source = OwnedAudioSource::try_resident(48000, vec![[0.25, -0.1]; 8]);
        owner_with_source(project, source.ok().unwrap())
    }

    fn owner_with_source(
        project: &hex_project::Project,
        source: OwnedAudioSource,
    ) -> (
        crate::ClaimedAudioCandidateRuntime,
        crate::AudioCandidateAnchors,
        usize,
        usize,
    ) {
        let asset = project.assets().next().unwrap().id();
        let bank = OwnedAudioInputs::try_new(
            vec![(asset, source)],
            AudioStorageReservation {
                allocation_items: 64,
                handle_obligations: 128,
                max_prepared_data_bytes: 64 * 1024 * 1024,
                max_construction_data_bytes: 64 * 1024 * 1024,
                caller_retained_bytes: 0,
                caller_construction_bytes: 0,
            },
        )
        .ok()
        .unwrap();
        let pointer = bank.sources.as_ptr() as usize;
        let capacity = bank.sources.capacity();
        let mut custody = AudioPreparationCustody::try_new_on_control().unwrap();
        let effects = hex_devices::native_registry().unwrap();
        let input = AudioArrangementInput::new(project, 11, bank).with_audio_prepared_revision();
        let input = if project.main_devices().is_empty() {
            input
        } else {
            input.with_effect_registry(&effects)
        };
        let token = custody
            .begin_input_on_control(input)
            .unwrap_or_else(|error| panic!("constructor: {:?}", error.reason));
        let mut owner = AudioCandidateOwner::try_new_on_control(custody, token)
            .ok()
            .unwrap();
        let summary = owner
            .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                data_rows: 4096,
                handle_rows: 2048,
                root_rows: 4,
                max_retained_bytes: 64 * 1024 * 1024,
                max_construction_peak_bytes: 64 * 1024 * 1024,
            })
            .unwrap();
        assert_eq!(summary.data.unknown_storage, 0);
        assert!(summary.data.require_complete().is_err());
        let (runtime, anchors) = owner.split_constructor_on_control().ok().unwrap();
        (runtime, anchors, pointer, capacity)
    }
    #[test]
    fn claimed_preview_reports_continuation_without_consuming_the_predecessor() {
        if !crate::audio_storage_constructor_layout_supported() {
            eprintln!("claimed constructor layout unsupported; fixture not exercised");
            return;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let (mut old, _old_anchors, _, _) = owner(&project);
        let (mut next, _next_anchors, _, _) = owner(&project);
        old.apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
        old.render(&mut [[0.0; 2]; 2]).unwrap();
        let before = old.status().unwrap();
        let (window, report) = next
            .stream_playback()
            .preview_stream_handoff_with_report(
                &mut old.stream_playback(),
                4,
                None,
                &mut |_| Ok(()),
            )
            .unwrap();
        assert!(!report.running_audio_restart());
        assert_eq!(window.frames, 4);
        assert_eq!(old.status().unwrap().position, before.position);
        let mut previous = Some(old);
        let mut retired = None;
        let mut manifest = None;
        next.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut manifest)
            .unwrap();
        assert!(previous.is_none());
        assert!(retired.is_some());
        assert!(manifest.is_some());
    }

    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
    ))]
    #[test]
    fn claimed_device_insert_preserves_the_actual_loop_clocks_and_retirement() {
        assert!(crate::audio_storage_constructor_layout_supported());
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        let key = crate::StreamSourceKey::new(11, asset);
        struct Pages(crate::StreamSourceKey, [crate::Frame; 8]);
        impl crate::MediaPages for Pages {
            fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {}

            fn page(
                &mut self,
                key: crate::StreamSourceKey,
                page_index: u64,
            ) -> Option<crate::BorrowedMediaPage<'_>> {
                (key == self.0 && page_index == 0).then_some(crate::BorrowedMediaPage {
                    page_index,
                    frames: &self.1,
                })
            }
        }
        let mut pages = Pages(key, [[0.25, -0.1]; 8]);
        let stream_owner = |project: &hex_project::Project| {
            let metadata = crate::StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                8,
                8,
                hex_project::IDENTITY_CONVERSION,
                [7; 32],
            )
            .unwrap();
            owner_with_source(
                project,
                OwnedAudioSource::try_streamed(metadata).ok().unwrap(),
            )
        };
        let mut editor = Editor::new(project, 8).unwrap();
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: 8,
                enabled: true,
            })
            .unwrap();
        let (mut old, _old_anchors, _, _) = stream_owner(editor.project());
        let state = crate::default_native_device_state_for_class(
            crate::NativeDeviceClass::Effect,
            crate::DeviceTypeId::from_static("hex.limiter"),
        )
        .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state,
            })
            .unwrap();
        let (mut unapproved, _unapproved_anchors, _, _) = stream_owner(editor.project());
        let (mut approved, _approved_anchors, _, _) = stream_owner(editor.project());
        old.apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
        old.stream_playback()
            .render_with_pages_and_signal_observers(&mut [[0.0; 2]; 3], &mut pages, None, None)
            .unwrap();
        let running = old.status().unwrap();
        assert!(unapproved.require_continuous_device_insert());
        assert_eq!(
            unapproved.preview_stream_handoff(&mut old, 4, None, &mut |_| Ok(())),
            Err(crate::PlaybackHandoffError::Session(
                crate::SessionPlaybackError::DeviceInsertNeedsConfirmation
            ))
        );
        let mut predecessor = Some(old);
        let mut retired = None;
        let mut manifest = None;
        assert_eq!(
            unapproved.try_inherit_and_retire_in_place(
                &mut predecessor,
                &mut retired,
                &mut manifest
            ),
            Err(crate::PlaybackHandoffError::Session(
                crate::SessionPlaybackError::DeviceInsertNeedsConfirmation
            ))
        );
        assert!(retired.is_none() && manifest.is_none());
        let old = predecessor.as_mut().unwrap();
        assert_eq!(old.status(), Some(running));
        let fade = old.device_insert_fade_frames().unwrap();
        let mut projected_extents = Vec::new();
        let projected = approved
            .stream_playback()
            .preview_device_insert(&mut old.stream_playback(), fade, 4, &mut |extent| {
                projected_extents.push(extent);
                Ok(())
            })
            .unwrap();
        assert!(!projected_extents.is_empty());
        assert_eq!(old.status(), Some(running));
        assert!(old.begin_device_insert_cut());
        let mut pcm = [[0.0; 2]; 128];
        assert_eq!(fade as usize, pcm.len());
        old.stream_playback()
            .render_with_pages_and_signal_observers(&mut pcm[..64], &mut pages, None, None)
            .unwrap();
        assert_eq!(
            unapproved.try_inherit_and_retire_in_place(
                &mut predecessor,
                &mut retired,
                &mut manifest
            ),
            Err(crate::PlaybackHandoffError::RuntimeFenceChanged)
        );
        let old = predecessor.as_mut().unwrap();
        assert!(old.device_insert_cut_pending());
        assert!(!old.device_insert_held());
        old.stream_playback()
            .render_with_pages_and_signal_observers(&mut pcm[64..], &mut pages, None, None)
            .unwrap();
        assert_eq!(pcm[0], [0.25 * 127.0 / 128.0, -0.1 * 127.0 / 128.0]);
        assert_eq!(pcm[127], [0.0; 2]);
        assert!(old.device_insert_held());
        let held = old.status().unwrap();
        assert_eq!(held.position, running.position);
        assert_eq!(
            held.performance_frame,
            running.performance_frame + u64::from(fade)
        );
        let mut exact_extents = Vec::new();
        let exact = approved
            .stream_playback()
            .preview_device_insert(&mut old.stream_playback(), 0, 4, &mut |extent| {
                exact_extents.push(extent);
                Ok(())
            })
            .unwrap();
        assert_eq!(projected, exact);
        assert_eq!(projected_extents, exact_extents);
        assert_eq!(
            unapproved.try_inherit_and_retire_in_place(
                &mut predecessor,
                &mut retired,
                &mut manifest
            ),
            Err(crate::PlaybackHandoffError::RuntimeFenceChanged)
        );
        let predecessor_id = predecessor.as_ref().unwrap().identity();
        approved
            .try_inherit_and_retire_at_device_insert_cut(
                &mut predecessor,
                &mut retired,
                &mut manifest,
            )
            .unwrap();
        assert!(predecessor.is_none());
        assert_eq!(retired.as_ref().unwrap().identity(), predecessor_id);
        let manifest = manifest.unwrap();
        assert_eq!(manifest.predecessor(), predecessor_id);
        assert_eq!(manifest.successor(), approved.identity());
        let resumed = approved.status().unwrap();
        assert!(resumed.playing && !approved.device_insert_cut_pending());
        assert_eq!(resumed.position, held.position);
        assert_eq!(resumed.performance_frame, held.performance_frame);
        assert_eq!(resumed.arrangement_loop_state, held.arrangement_loop_state);
        assert!(resumed.main_output_latency_frames > 0);
    }

    #[cfg(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64", target_env = "gnu")
    ))]
    #[test]
    fn claimed_streamed_tempo_cut_preserves_pcm_and_original_retirement() {
        assert!(crate::audio_storage_constructor_layout_supported());
        const FRAMES: usize = 2048;
        const LOOP: usize = 128;
        let fixture = crate::signal_graph::tests::fixture(&[&[]], FRAMES);
        let asset = fixture.editor.project().assets().next().unwrap().id();
        let key = crate::StreamSourceKey::new(11, asset);
        struct Pages(crate::StreamSourceKey, Vec<crate::Frame>);
        impl crate::MediaPages for Pages {
            fn request_page(&mut self, _: crate::StreamSourceKey, _: u64) {
                panic!("the only source page is present");
            }
            fn page(
                &mut self,
                key: crate::StreamSourceKey,
                page_index: u64,
            ) -> Option<crate::BorrowedMediaPage<'_>> {
                (key == self.0 && page_index == 0).then_some(crate::BorrowedMediaPage {
                    page_index,
                    frames: &self.1,
                })
            }
        }
        let mut pages = Pages(key, vec![[0.25, -0.1]; FRAMES]);
        let stream_owner = |project: &hex_project::Project| {
            let metadata = crate::StreamedSourceMetadata::new(
                key,
                48_000,
                48_000,
                FRAMES as u64,
                FRAMES as u64,
                hex_project::IDENTITY_CONVERSION,
                [7; 32],
            )
            .unwrap();
            owner_with_source(
                project,
                OwnedAudioSource::try_streamed(metadata).ok().unwrap(),
            )
        };
        let mut editor = fixture.editor;
        editor
            .execute(Command::SetLoopRegion {
                start: 0,
                end: LOOP as u64,
                enabled: true,
            })
            .unwrap();
        editor
            .execute(Command::InsertMainDevice {
                before: None,
                state: crate::default_native_device_state_for_class(
                    crate::NativeDeviceClass::Effect,
                    crate::DeviceTypeId::from_static("hex.limiter"),
                )
                .unwrap(),
            })
            .unwrap();
        let (mut old, old_anchors, _, _) = stream_owner(editor.project());
        let latency = old.status().unwrap().main_output_latency_frames;
        assert!(latency > 0);
        editor
            .execute(Command::SetProjectTempo {
                tempo: hex_project::Tempo::new(60_000_000).unwrap(),
            })
            .unwrap();
        let (mut next, next_anchors, _, _) = stream_owner(editor.project());
        old.apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
        let mut warm = [[0.0; 2]; LOOP];
        for _ in 0..latency.div_ceil(LOOP as u32) + 2 {
            old.stream_playback()
                .render_with_pages_and_signal_observers(&mut warm, &mut pages, None, None)
                .unwrap();
        }
        let before = old.status().unwrap();
        assert_eq!(before.position, 0);
        assert!(
            warm.iter().any(|frame| frame[0].abs() > 0.01),
            "lost predecessor PCM before the edit"
        );
        let due = before.performance_frame + LOOP as u64 + 3;
        let anchor = old_anchors
            .prepare_tempo_successor_on_control(
                &next_anchors,
                before.position,
                before.performance_frame,
                before.arrangement_loop_state.unwrap(),
                due,
            )
            .unwrap();
        let (mut other, other_anchors, _, _) = stream_owner(editor.project());
        assert!(
            other
                .stream_playback()
                .forecast_tempo_cut(
                    &mut old.stream_playback(),
                    2,
                    &mut |_| panic!("separately constructed successor requested pages"),
                    &anchor,
                )
                .is_err()
        );
        let mut forecast_extents = Vec::new();
        let forecast = next
            .stream_playback()
            .forecast_tempo_cut(
                &mut old.stream_playback(),
                2,
                &mut |extent| {
                    forecast_extents.push(extent);
                    Ok(())
                },
                &anchor,
            )
            .unwrap();
        assert!(!forecast_extents.is_empty());
        assert!(
            next.stream_playback()
                .preview_tempo_cut(
                    &mut old.stream_playback(),
                    2,
                    &mut |_| panic!("early cut requested pages"),
                    &anchor,
                )
                .is_err()
        );
        let predecessor_id = old.identity();
        let mut previous = Some(old);
        let mut retired = None;
        let mut manifest = None;
        assert!(
            next.try_inherit_and_retire_at_tempo_cut(
                &mut previous,
                &mut retired,
                &mut manifest,
                &anchor,
            )
            .is_err(),
            "an early tempo cut must leave the donor installed"
        );
        assert!(retired.is_none() && manifest.is_none());
        let old = previous.as_mut().unwrap();
        assert_eq!(old.status(), Some(before));
        let mut prefix = [[0.0; 2]; LOOP + 3];
        old.stream_playback()
            .render_with_pages_and_signal_observers(&mut prefix, &mut pages, None, None)
            .unwrap();
        let exact_status = old.status().unwrap();
        assert_eq!(exact_status.performance_frame, due);
        assert_eq!(exact_status.position, 3);
        let mut exact_extents = Vec::new();
        let exact = next
            .stream_playback()
            .preview_tempo_cut(
                &mut old.stream_playback(),
                2,
                &mut |extent| {
                    exact_extents.push(extent);
                    Ok(())
                },
                &anchor,
            )
            .unwrap();
        assert_eq!(exact, forecast);
        assert_eq!(exact_extents, forecast_extents);
        assert_eq!(exact.arrangement_frame, 6);
        next.try_inherit_and_retire_at_tempo_cut(
            &mut previous,
            &mut retired,
            &mut manifest,
            &anchor,
        )
        .unwrap();
        assert!(previous.is_none());
        let manifest = manifest.unwrap();
        assert_eq!(manifest.predecessor(), predecessor_id);
        assert_eq!(manifest.successor(), next.identity());
        let resumed = next.status().unwrap();
        assert!(resumed.playing);
        assert_eq!((resumed.position, resumed.performance_frame), (6, due));
        assert_eq!(resumed.main_output_latency_frames, latency);
        let mut suffix = [[0.0; 2]; 2];
        next.stream_playback()
            .render_with_pages_and_signal_observers(&mut suffix, &mut pages, None, None)
            .unwrap();
        assert_eq!(suffix, prefix[LOOP + 1..]);

        let mut anchors = [Some(old_anchors), Some(next_anchors), Some(other_anchors)];
        let mut retired = retired.unwrap();
        assert_eq!(retired.identity(), predecessor_id);
        assert!(retired.status().is_none());
        retired
            .dispose_on_control()
            .unwrap()
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        next.dispose_on_control()
            .unwrap()
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        other
            .dispose_on_control()
            .unwrap()
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        for anchor in anchors.iter_mut().flatten() {
            assert!(anchor.all_origins_closed());
            anchor.finish_disposal_on_control().unwrap();
            anchor.dispose_original_inputs_on_control().unwrap();
        }
        // The plan can outlive both archives without keeping a Weak owner in
        // either prepared allocation's exclusive reclamation path.
        drop(anchor);
    }

    #[test]
    fn real_utility_chains_fan_out_by_actual_tail_lifetime_not_history_copy() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        let x = project.tracks()[0].id();
        let mut editor = Editor::new(project, 16).unwrap();
        let mut ids = vec![x];
        for name in ["y", "z"] {
            let Created::Track(id) = editor
                .execute(Command::InsertTrackWithDevice {
                    name: name.into(),
                    before_track: None,
                    slot: hex_project::TrackDeviceSlot::Instrument,
                    state: hex_project::default_instrument_state()
                        .expect("fixture instrument state"),
                })
                .unwrap()
            else {
                panic!("track")
            };
            editor
                .execute(Command::RemoveInstrument { track: id })
                .unwrap();
            editor
                .execute(Command::PlaceClip {
                    track: id,
                    asset,
                    start: 0,
                    duration: 8,
                    source_offset: 0,
                    source_length: 8,
                    looping: false,
                })
                .unwrap();
            ids.push(id);
        }
        let mut snapshot = editor.project().snapshot();
        let definition = &hex_devices::utility::UTILITY_DEFINITION;
        for (i, track) in snapshot.tracks.iter_mut().enumerate() {
            let state = definition
                .patch_parameter(
                    &definition.default_state().unwrap(),
                    if i == 1 {
                        hex_devices::utility::parameter::BASS_MONO
                    } else {
                        hex_devices::utility::parameter::DC_REMOVE
                    },
                    ParameterValue::Bool(true),
                )
                .unwrap();
            let state = definition
                .patch_parameter(
                    &state,
                    hex_devices::utility::parameter::BASS_MONO_CUTOFF,
                    ParameterValue::Float(500.0),
                )
                .unwrap();
            track.devices.push(hex_project::DeviceSnapshot {
                modulation: Default::default(),
                parent: None,
                automation: Vec::new(),
                comparison: None,
                id: snapshot.next_id,
                bypassed: false,
                state,
            });
            snapshot.next_id += 1;
        }
        let a_project = hex_project::Project::from_snapshot(snapshot).unwrap();
        let mut editor = Editor::new(a_project.clone(), 8).unwrap();
        editor.execute(Command::RemoveTrack(ids[0])).unwrap();
        editor.execute(Command::RemoveTrack(ids[1])).unwrap();
        let b_project = editor.project().clone();
        let c_project = b_project.clone();
        let (mut a, aa, ap, ac) = owner(&a_project);
        let (mut b, ba, bp, bc) = owner(&b_project);
        let (mut c, ca, cp, cc) = owner(&c_project);
        let a_identity = a.identity();
        let a_boxes = ids
            .iter()
            .map(|id| a.test_track_boxes(*id).unwrap())
            .collect::<Vec<_>>();
        assert!(a_boxes.iter().all(|(primary, scratch)| primary.is_some()
            && scratch.is_some()
            && primary != scratch));
        a.apply_action(crate::PlaybackAction::Transport(
            crate::PlaybackCommand::Play,
        ))
        .unwrap();
        a.render(&mut [[0.0; 2]; 8]).unwrap();
        let mut previous = Some(a);
        let mut retired = None;
        let mut ab = None;
        // Exhaust the current owned-runtime allowance before the same actual
        // transfer. Failure must precede every owner/origin move, not merely
        // suppress a receipt after moving the filters.
        let original_budget = b
            .test_set_owned_runtime_reservation(crate::MAX_ARRANGEMENT_PREROLL_STORAGE_BYTES)
            .unwrap();
        assert!(
            b.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut ab)
                .is_err()
        );
        assert!(retired.is_none() && ab.is_none());
        assert_eq!(
            previous.as_ref().unwrap().test_track_boxes(ids[0]),
            Some(a_boxes[0])
        );
        assert_eq!(
            previous.as_ref().unwrap().test_track_boxes(ids[1]),
            Some(a_boxes[1])
        );
        assert_eq!(b.test_tail_boxes(ids[0]), None);
        assert_eq!(b.test_tail_boxes(ids[1]), None);
        b.test_set_owned_runtime_reservation(original_budget);
        b.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut ab)
            .unwrap();
        assert!(previous.is_none());
        assert_eq!(ab.as_ref().unwrap().moved_groups(), 2);
        assert_eq!(ab.as_ref().unwrap().moved_groups_from(a_identity), 2);
        let a = retired.take().unwrap();
        assert_eq!(a.test_track_boxes(ids[2]), Some(a_boxes[2]));
        assert_ne!(b.test_track_boxes(ids[2]), Some(a_boxes[2]));
        assert_eq!(b.test_tail_boxes(ids[0]), Some(a_boxes[0]));
        assert_eq!(b.test_tail_boxes(ids[1]), Some(a_boxes[1]));
        // The real fast bass-mono filter drains; the real 5Hz DC filter still
        // owns active history. These flags are read from actual committed DSP.
        b.render(&mut [[0.0; 2]; 4096]).unwrap();
        assert_eq!(b.test_tail_activity(ids[0]), Some((true, true)));
        assert_eq!(b.test_tail_activity(ids[1]), Some((false, false)));
        let mut previous = Some(b);
        let mut bc_manifest = None;
        c.try_inherit_and_retire_in_place(&mut previous, &mut retired, &mut bc_manifest)
            .unwrap();
        assert_eq!(bc_manifest.as_ref().unwrap().moved_groups(), 1);
        assert_eq!(
            bc_manifest.as_ref().unwrap().moved_groups_from(a_identity),
            1
        );
        let b = retired.take().unwrap();
        assert_eq!(b.test_tail_boxes(ids[1]), Some(a_boxes[1]));
        assert_eq!(c.test_tail_boxes(ids[0]), Some(a_boxes[0]));
        assert_eq!(c.test_tail_boxes(ids[1]), None);
        let mut cells = [Some(a), Some(b), Some(c)];
        let mut anchors = [Some(aa), Some(ba), Some(ca)];
        let a_proof = cells[0].as_mut().unwrap().dispose_on_control().unwrap();
        a_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            anchors[0]
                .as_mut()
                .unwrap()
                .finish_disposal_on_control()
                .is_err()
        );
        assert!(!anchors[0].as_ref().unwrap().all_origins_closed());
        let b_proof = cells[1].as_mut().unwrap().dispose_on_control().unwrap();
        b_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            anchors[0]
                .as_mut()
                .unwrap()
                .finish_disposal_on_control()
                .is_err()
        );
        anchors[1]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        let c_proof = cells[2].as_mut().unwrap().dispose_on_control().unwrap();
        c_proof
            .record_anchor_cells_on_control(&mut anchors)
            .unwrap();
        assert!(
            c_proof
                .record_anchor_cells_on_control(&mut anchors)
                .is_err()
        );
        anchors[0]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        anchors[2]
            .as_mut()
            .unwrap()
            .finish_disposal_on_control()
            .unwrap();
        for (owner, (pointer, capacity)) in
            anchors
                .iter_mut()
                .flatten()
                .zip([(ap, ac), (bp, bc), (cp, cc)])
        {
            let bank = owner.recover_inputs_on_control().unwrap();
            assert_eq!(
                (bank.sources.as_ptr() as usize, bank.sources.capacity()),
                (pointer, capacity)
            );
        }
    }
}
