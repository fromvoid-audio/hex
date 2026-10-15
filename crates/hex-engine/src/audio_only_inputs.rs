//! Consuming source ingress. Constructor declarations are not admission/retirement.
use crate::{Audio, Frame, StreamedSourceMetadata, owned_shared::OwnedShared};
use hex_project::AssetId;
use std::sync::atomic::{AtomicU64, Ordering};

// Identity allocator only: no owner registry or pointer/content-based identity.
static NEXT_HANDLE: AtomicU64 = AtomicU64::new(1);
pub(crate) fn handle() -> Result<u64, AudioStorageError> {
    NEXT_HANDLE
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_add(1))
        .map_err(|_| AudioStorageError::HandleExhausted)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AudioStorageError {
    UnsupportedConstructor,
    InvalidSamples,
    InvalidRate,
    DuplicateAsset,
    Allocation,
    Overflow,
    Capacity,
    ReservationExceeded,
    HandleExhausted,
    UnsupportedGraph,
    SourceMismatch,
    UnknownStorage,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudioStorageReservation {
    pub allocation_items: usize,
    pub handle_obligations: usize,
    pub max_prepared_data_bytes: usize,
    pub max_construction_data_bytes: usize,
    pub caller_retained_bytes: usize,
    pub caller_construction_bytes: usize,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioPreparationCapacity {
    pub original_source_capacity: usize,
    pub source_container_bytes: usize,
    pub alias_capacity: usize,
    pub alias_container_bytes: usize,
    pub bank_container_bytes: usize,
    pub prepared_data_bytes: Option<usize>,
    pub construction_peak_bytes: Option<usize>,
}
pub struct ResidentSourceRefusal {
    pub reason: AudioStorageError,
    pub frames: Vec<Frame>,
}
pub struct StreamedSourceRefusal {
    pub reason: AudioStorageError,
    pub metadata: StreamedSourceMetadata,
}
/// No raw Arc/Weak/Deref escape. Explicit control aliases have distinct sealed
/// owning-handle identities but retain the same constructor-paired allocation.
pub struct OwnedAudioSource {
    pub(crate) kind: SourceKind,
    pub(crate) handle: u64,
}
pub(crate) enum SourceKind {
    Resident(Audio),
    Streamed(OwnedShared<StreamedSourceMetadata>),
}
impl OwnedAudioSource {
    /// All reported refusals precede Vec→Arc; original pointer/capacity survive.
    /// Process-fatal allocator OOM/abort is outside API-reported recovery.
    pub fn try_resident(rate: u32, frames: Vec<Frame>) -> Result<Self, ResidentSourceRefusal> {
        Self::resident_with_gate(rate, frames, crate::pcm_arc::native_layout_known())
    }
    fn resident_with_gate(
        rate: u32,
        frames: Vec<Frame>,
        gate: bool,
    ) -> Result<Self, ResidentSourceRefusal> {
        let result = (|| {
            if rate == 0 {
                return Err(AudioStorageError::InvalidRate);
            }
            if frames.is_empty() || frames.iter().flatten().any(|v| !v.is_finite()) {
                return Err(AudioStorageError::InvalidSamples);
            }
            if !gate {
                return Err(AudioStorageError::UnsupportedConstructor);
            }
            let receipt = crate::pcm_arc::PcmArcReceipt::current(frames.len())
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
            let handle = handle()?;
            Ok((receipt, handle))
        })();
        match result {
            Err(reason) => Err(ResidentSourceRefusal { reason, frames }),
            Ok((receipt, handle)) => Ok(Self {
                kind: SourceKind::Resident(Audio {
                    sample_rate: rate,
                    frames: frames.into(),
                    pcm_arc_receipt: Some(receipt),
                }),
                handle,
            }),
        }
    }
    /// Metadata was numerically validated by its value constructor; copying a
    /// value into a NEW owned allocation never adopts a caller-created Arc.
    /// Error boxing is control-only and follows existing fatal-OOM semantics.
    pub fn try_streamed(
        metadata: StreamedSourceMetadata,
    ) -> Result<Self, Box<StreamedSourceRefusal>> {
        let result = (|| {
            if !crate::pcm_arc::native_layout_known() {
                return Err(AudioStorageError::UnsupportedConstructor);
            }
            handle()
        })();
        match result {
            Err(reason) => Err(Box::new(StreamedSourceRefusal { reason, metadata })),
            Ok(handle) => Ok(Self {
                kind: SourceKind::Streamed(OwnedShared::new_owned(metadata)),
                handle,
            }),
        }
    }
    // Owning alias creation intentionally absent until bounded caller custody
    // is decided. A bare clone would escape the consumed bank's handle ledger.
    pub fn sample_rate(&self) -> u32 {
        match &self.kind {
            SourceKind::Resident(a) => a.sample_rate(),
            SourceKind::Streamed(s) => s.project_rate(),
        }
    }
    pub fn frames(&self) -> u64 {
        match &self.kind {
            SourceKind::Resident(a) => a.frames.len() as u64,
            SourceKind::Streamed(s) => s.playback_frames(),
        }
    }
}
/// Original order and source Vec capacity are preserved. Lookup is control-only;
/// no temporary BTreeMap, sorted-copy bank or public graph owner is introduced.
#[derive(Default)]
pub(crate) struct AliasRow {
    incarnation: u64,
    pub(crate) owner: Option<OwnedAudioSource>,
    pub(crate) source: usize,
}
/// Checked NONOWNING token; no Clone, scalar constructor or source owner.
pub struct AudioSourceToken {
    bank: u64,
    custodian: u64,
    source: usize,
    source_handle: u64,
    alias: Option<(usize, u64, u64)>,
}
/// Borrowed leaf-only view tied to the actual live bank, never to a token alone.
pub struct OwnedAudioSourceView<'a> {
    kind: &'a SourceKind,
}
impl OwnedAudioSourceView<'_> {
    pub fn sample_rate(&self) -> u32 {
        match self.kind {
            SourceKind::Resident(a) => a.sample_rate(),
            SourceKind::Streamed(s) => s.project_rate(),
        }
    }
    pub fn resident_frames(&self) -> Option<&[Frame]> {
        match self.kind {
            SourceKind::Resident(a) => Some(a.frames.as_ref()),
            _ => None,
        }
    }
    pub fn streamed_metadata(&self) -> Option<&StreamedSourceMetadata> {
        match self.kind {
            SourceKind::Streamed(s) => Some(s.as_ref()),
            _ => None,
        }
    }
}
pub struct OwnedAudioInputs {
    bank: u64,
    custodian: u64,
    pub(crate) aliases: Vec<AliasRow>,
    pub(crate) reservation: AudioStorageReservation,
    pub(crate) sources: Vec<(AssetId, OwnedAudioSource)>,
    pub(crate) capacity: AudioPreparationCapacity,
}
pub struct AudioInputsRefusal {
    pub reason: AudioStorageError,
    pub sources: Vec<(AssetId, OwnedAudioSource)>,
    pub reservation: AudioStorageReservation,
    pub capacity: AudioPreparationCapacity,
}
impl OwnedAudioInputs {
    pub fn try_new(
        sources: Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
    ) -> Result<Self, Box<AudioInputsRefusal>> {
        let mut aliases = Vec::new();
        let mut capacity = AudioPreparationCapacity {
            original_source_capacity: sources.capacity(),
            ..AudioPreparationCapacity::default()
        };
        let result = (|| {
            capacity.source_container_bytes = sources
                .capacity()
                .checked_mul(std::mem::size_of::<(AssetId, OwnedAudioSource)>())
                .ok_or(AudioStorageError::Overflow)?;
            let alias_limit = reservation
                .handle_obligations
                .checked_sub(sources.len())
                .ok_or(AudioStorageError::Capacity)?;
            aliases
                .try_reserve_exact(alias_limit)
                .map_err(|_| AudioStorageError::Allocation)?;
            capacity.alias_capacity = aliases.capacity();
            capacity.alias_container_bytes = aliases
                .capacity()
                .checked_mul(std::mem::size_of::<AliasRow>())
                .ok_or(AudioStorageError::Overflow)?;
            capacity.bank_container_bytes = std::mem::size_of::<Self>();
            let retained = capacity
                .source_container_bytes
                .checked_add(capacity.alias_container_bytes)
                .and_then(|n| n.checked_add(capacity.bank_container_bytes))
                .ok_or(AudioStorageError::Overflow)?;
            let retained = retained
                .checked_add(reservation.caller_retained_bytes)
                .ok_or(AudioStorageError::Overflow)?;
            let peak = retained
                .checked_add(reservation.caller_construction_bytes)
                .ok_or(AudioStorageError::Overflow)?;
            if retained > reservation.max_prepared_data_bytes
                || peak > reservation.max_construction_data_bytes
            {
                return Err(AudioStorageError::ReservationExceeded);
            }
            if sources.len() > reservation.handle_obligations
                || sources.len() > reservation.allocation_items
            {
                return Err(AudioStorageError::Capacity);
            }
            for (index, (id, _)) in sources.iter().enumerate() {
                if sources[..index].iter().any(|(previous, _)| previous == id) {
                    return Err(AudioStorageError::DuplicateAsset);
                }
            }
            // These are only actual BANK container observations, never complete
            // graph/domain/allocator-memory totals or a prepared admission.
            let bank = handle()?;
            Ok(bank)
        })();
        match result {
            Ok(bank) => {
                aliases.resize_with(
                    reservation.handle_obligations - sources.len(),
                    AliasRow::default,
                );
                Ok(Self {
                    sources,
                    capacity,
                    bank,
                    custodian: 1,
                    aliases,
                    reservation,
                })
            }
            Err(reason) => Err(Box::new(AudioInputsRefusal {
                reason,
                sources,
                reservation,
                capacity,
            })),
        }
    }
}

impl OwnedAudioInputs {
    pub fn source_token(&self, asset: AssetId) -> Result<AudioSourceToken, AudioStorageError> {
        let index = self
            .sources
            .iter()
            .position(|(id, _)| *id == asset)
            .ok_or(AudioStorageError::SourceMismatch)?;
        Ok(AudioSourceToken {
            bank: self.bank,
            custodian: self.custodian,
            source: index,
            source_handle: self.sources[index].1.handle,
            alias: None,
        })
    }
    fn checked_source(
        &self,
        token: &AudioSourceToken,
    ) -> Result<&OwnedAudioSource, AudioStorageError> {
        if token.bank != self.bank || token.custodian != self.custodian {
            return Err(AudioStorageError::SourceMismatch);
        }
        let original = self
            .sources
            .get(token.source)
            .map(|(_, source)| source)
            .filter(|source| source.handle == token.source_handle)
            .ok_or(AudioStorageError::SourceMismatch)?;
        match token.alias {
            None => Ok(original),
            Some((index, incarnation, handle)) => self
                .aliases
                .get(index)
                .filter(|row| row.source == token.source && row.incarnation == incarnation)
                .and_then(|row| row.owner.as_ref())
                .filter(|owner| owner.handle == handle)
                .ok_or(AudioStorageError::SourceMismatch),
        }
    }
    pub fn source_view(
        &self,
        token: &AudioSourceToken,
    ) -> Result<OwnedAudioSourceView<'_>, AudioStorageError> {
        Ok(OwnedAudioSourceView {
            kind: &self.checked_source(token)?.kind,
        })
    }
    /// All destinations/identities are preflighted before cloning. No owning
    /// clone is returned: both actual handles remain inside bank custody.
    pub fn alias_on_control(
        &mut self,
        token: &AudioSourceToken,
    ) -> Result<AudioSourceToken, AudioStorageError> {
        self.checked_source(token)?;
        let index = self
            .aliases
            .iter()
            .position(|row| row.owner.is_none())
            .ok_or(AudioStorageError::Capacity)?;
        let incarnation = self.aliases[index]
            .incarnation
            .checked_add(1)
            .ok_or(AudioStorageError::HandleExhausted)?;
        let handle = handle()?;
        let kind = match &self.checked_source(token)?.kind {
            SourceKind::Resident(audio) => SourceKind::Resident(audio.clone()),
            SourceKind::Streamed(source) => SourceKind::Streamed(source.clone_on_control()),
        };
        self.aliases[index] = AliasRow {
            incarnation,
            source: token.source,
            owner: Some(OwnedAudioSource { kind, handle }),
        };
        Ok(AudioSourceToken {
            bank: self.bank,
            custodian: self.custodian,
            source: token.source,
            source_handle: token.source_handle,
            alias: Some((index, incarnation, handle)),
        })
    }
    /// Scalar rejection returns the original nonowning token. The original
    /// source remains anchored, so retiring an alias cannot final-drop its data.
    pub fn retire_alias_on_control(
        &mut self,
        token: AudioSourceToken,
    ) -> Result<(), (AudioStorageError, AudioSourceToken)> {
        if let Err(reason) = self.checked_source(&token) {
            return Err((reason, token));
        }
        let Some((index, _, _)) = token.alias else {
            return Err((AudioStorageError::SourceMismatch, token));
        };
        drop(self.aliases[index].owner.take());
        Ok(())
    }
    pub fn capacity_report(&self) -> AudioPreparationCapacity {
        self.capacity
    }
    pub fn owning_handle_count(&self) -> usize {
        self.sources.len()
            + self
                .aliases
                .iter()
                .filter(|row| row.owner.is_some())
                .count()
    }
    /// Only the final infallible preparation commit may install this previously
    /// checked nonce. Errors never mutate token validity or alias owners.
    pub(crate) fn next_custodian(&self) -> Result<u64, AudioStorageError> {
        self.custodian
            .checked_add(1)
            .ok_or(AudioStorageError::HandleExhausted)
    }
    pub(crate) fn commit_custodian(&mut self, next: u64) {
        self.custodian = next;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ids() -> Vec<AssetId> {
        let mut editor =
            hex_project::Editor::new(hex_project::Project::new(48_000).unwrap(), 8).unwrap();
        (0..3)
            .map(|n| {
                let hex_project::Created::Asset(id) = editor
                    .execute(hex_project::Command::AddAsset {
                        path: format!("source-{n}.wav").into(),
                        sample_rate: 48_000,
                        frames: 8,
                    })
                    .unwrap()
                else {
                    panic!("asset");
                };
                id
            })
            .collect()
    }
    fn reservation() -> AudioStorageReservation {
        AudioStorageReservation {
            allocation_items: 8,
            handle_obligations: 8,
            max_prepared_data_bytes: usize::MAX,
            max_construction_data_bytes: usize::MAX,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        }
    }
    fn copy_negative(token: &AudioSourceToken) -> AudioSourceToken {
        AudioSourceToken {
            bank: token.bank,
            custodian: token.custodian,
            source: token.source,
            source_handle: token.source_handle,
            alias: token.alias,
        }
    }
    #[test]
    fn original_frame_owners_survive_numerical_and_unsupported_origin_refusal() {
        for (rate, sample, gate, expected) in [
            (0, 0.0, true, AudioStorageError::InvalidRate),
            (48_000, f32::NAN, true, AudioStorageError::InvalidSamples),
            (
                48_000,
                0.0,
                false,
                AudioStorageError::UnsupportedConstructor,
            ),
        ] {
            let mut frames = Vec::with_capacity(19);
            frames.push([sample; 2]);
            let pointer = frames.as_ptr();
            let capacity = frames.capacity();
            let refusal = OwnedAudioSource::resident_with_gate(rate, frames, gate)
                .err()
                .unwrap();
            assert_eq!(refusal.reason, expected);
            assert_eq!(refusal.frames.as_ptr(), pointer);
            assert_eq!(refusal.frames.capacity(), capacity);
        }
    }
    #[test]
    fn aliases_are_bank_owned_tokens_refuse_wrong_stale_duplicate_and_overflow() {
        if !crate::pcm_arc::native_layout_known() {
            assert!(OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]]).is_err());
            return;
        }
        let ids = ids();
        let make = || {
            OwnedAudioSource::try_resident(48_000, vec![[0.25; 2]; 8])
                .ok()
                .unwrap()
        };
        let mut bank = OwnedAudioInputs::try_new(vec![(ids[0], make())], reservation())
            .ok()
            .unwrap();
        let other = OwnedAudioInputs::try_new(vec![(ids[0], make())], reservation())
            .ok()
            .unwrap();
        let original = bank.source_token(ids[0]).unwrap();
        let alias = bank.alias_on_control(&original).unwrap();
        let stale = copy_negative(&alias);
        assert_eq!(bank.owning_handle_count(), 2);
        assert!(other.source_view(&alias).is_err());
        assert_eq!(
            bank.source_view(&alias).unwrap().resident_frames().unwrap()[0],
            [0.25; 2]
        );
        bank.retire_alias_on_control(alias).ok().unwrap();
        assert!(bank.source_view(&stale).is_err());
        let (_, stale) = bank.retire_alias_on_control(stale).err().unwrap();
        let replacement = bank.alias_on_control(&original).unwrap();
        assert!(bank.source_view(&stale).is_err());
        assert!(bank.source_view(&replacement).is_ok());
        bank.retire_alias_on_control(replacement).ok().unwrap();
        bank.aliases[0].incarnation = u64::MAX;
        let before = bank.owning_handle_count();
        assert!(matches!(
            bank.alias_on_control(&original),
            Err(AudioStorageError::HandleExhausted)
        ));
        assert_eq!(bank.owning_handle_count(), before);
        assert!(bank.source_view(&original).is_ok());
    }
    #[test]
    fn duplicate_and_byte_refusal_return_original_source_vector_and_capacity() {
        if !crate::pcm_arc::native_layout_known() {
            assert!(OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]]).is_err());
            return;
        }
        let ids = ids();
        for duplicate in [true, false] {
            let mut sources = Vec::with_capacity(17);
            for id in [ids[0], if duplicate { ids[0] } else { ids[1] }] {
                sources.push((
                    id,
                    OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]; 8])
                        .ok()
                        .unwrap(),
                ));
            }
            let pointer = sources.as_ptr();
            let capacity = sources.capacity();
            let mut limit = reservation();
            if !duplicate {
                limit.max_prepared_data_bytes = 0;
            }
            let refusal = OwnedAudioInputs::try_new(sources, limit).err().unwrap();
            assert_eq!(
                refusal.reason,
                if duplicate {
                    AudioStorageError::DuplicateAsset
                } else {
                    AudioStorageError::ReservationExceeded
                }
            );
            assert_eq!(refusal.sources.as_ptr(), pointer);
            assert_eq!(refusal.sources.capacity(), capacity);
            assert_eq!(refusal.reservation, limit);
        }
    }
    #[test]
    fn typed_bank_data_dedups_allocations_never_handles_and_commit_invalidates_old_tokens() {
        if !crate::pcm_arc::native_layout_known() {
            assert!(OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]]).is_err());
            return;
        }
        let ids = ids();
        let make = || {
            OwnedAudioSource::try_resident(48_000, vec![[0.5; 2]; 8])
                .ok()
                .unwrap()
        };
        let mut bank =
            OwnedAudioInputs::try_new(vec![(ids[0], make()), (ids[1], make())], reservation())
                .ok()
                .unwrap();
        let token = bank.source_token(ids[0]).unwrap();
        let _one = bank.alias_on_control(&token).unwrap();
        let _two = bank.alias_on_control(&token).unwrap();
        assert_eq!(bank.owning_handle_count(), 4);
        let output = bank.prepare_input_storage_on_control().ok().unwrap();
        assert_eq!(output.summary().unique_allocations, 2); // identical PCM ≠ identity
        assert_eq!(output.summary().unresolved_handles, 4);
        assert!(output.validate_unresolved_on_control().is_ok());
        assert!(output.source_view(&token).is_err());
        assert!(
            output
                .source_view(&output.source_token(ids[0]).unwrap())
                .is_ok()
        );
    }
    #[test]
    fn actual_streamed_constructor_and_refusal_preserve_alias_custody_and_tokens() {
        let ids = ids();
        let metadata = StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, ids[0]),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [9; 32],
        )
        .unwrap();
        if !crate::pcm_arc::native_layout_known() {
            assert_eq!(
                OwnedAudioSource::try_streamed(metadata)
                    .err()
                    .unwrap()
                    .reason,
                AudioStorageError::UnsupportedConstructor
            );
            return;
        }
        let source = OwnedAudioSource::try_streamed(metadata.clone())
            .ok()
            .unwrap();
        let mut bank = OwnedAudioInputs::try_new(vec![(ids[0], source)], reservation())
            .ok()
            .unwrap();
        let original = bank.source_token(ids[0]).unwrap();
        let alias = bank.alias_on_control(&original).unwrap();
        let pointer = bank.sources.as_ptr();
        let capacity = bank.sources.capacity();
        bank.custodian = u64::MAX; // checked commit overflow BEFORE any move
        let mut alias = alias;
        alias.custodian = u64::MAX;
        let refusal = bank.prepare_input_storage_on_control().err().unwrap();
        assert_eq!(refusal.reason, AudioStorageError::HandleExhausted);
        assert_eq!(refusal.inputs.sources.as_ptr(), pointer);
        assert_eq!(refusal.inputs.sources.capacity(), capacity);
        assert_eq!(refusal.inputs.owning_handle_count(), 2);
        assert_eq!(
            refusal
                .inputs
                .source_view(&alias)
                .unwrap()
                .streamed_metadata(),
            Some(&metadata)
        );
    }
}
