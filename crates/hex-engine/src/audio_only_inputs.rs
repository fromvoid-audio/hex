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
enum StreamIngress {
    Raw,
    Pitch,
    Repitch,
}

impl OwnedAudioSource {
    /// All reported refusals precede the Arc wrapper allocation; original pointer/capacity survive.
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
            let receipt = crate::pcm_arc::PcmArcReceipt::current(frames.len(), frames.capacity())
                .ok_or(AudioStorageError::UnsupportedConstructor)?;
            let handle = handle()?;
            Ok((receipt, handle))
        })();
        match result {
            Err(reason) => Err(ResidentSourceRefusal { reason, frames }),
            Ok((receipt, handle)) => Ok(Self {
                kind: SourceKind::Resident(Audio {
                    sample_rate: rate,
                    frames: std::sync::Arc::new(crate::AudioData {
                        values: frames,
                        _derived: None,
                    }),
                    pcm_arc_receipt: Some(receipt),
                }),
                handle,
            }),
        }
    }
    /// Metadata was numerically validated by its value constructor; copying a
    /// value into a NEW owned allocation never adopts a caller-created Arc.
    /// Asset-only inputs accept original sources and refuse derived pitch pages.
    /// Error boxing is control-only and follows existing fatal-OOM semantics.
    pub fn try_streamed(
        metadata: StreamedSourceMetadata,
    ) -> Result<Self, Box<StreamedSourceRefusal>> {
        Self::streamed_with_kind(metadata, StreamIngress::Raw)
    }
    /// Construct an owned pitch identity for the explicit pitch-aware bank.
    /// That bank still requires the matching raw source before this entry and
    /// verifies all source facts; this constructor grants no worker or page lease.
    pub fn try_pitched_streamed(
        metadata: StreamedSourceMetadata,
    ) -> Result<Self, Box<StreamedSourceRefusal>> {
        Self::streamed_with_kind(metadata, StreamIngress::Pitch)
    }
    /// Construct an owned Re-Pitch identity. A derived bank must still match it to raw source facts.
    pub fn try_repitch_streamed(
        metadata: StreamedSourceMetadata,
    ) -> Result<Self, Box<StreamedSourceRefusal>> {
        Self::streamed_with_kind(metadata, StreamIngress::Repitch)
    }
    fn streamed_with_kind(
        metadata: StreamedSourceMetadata,
        kind: StreamIngress,
    ) -> Result<Self, Box<StreamedSourceRefusal>> {
        let result = (|| {
            let pitch = metadata.key().pitch();
            if match kind {
                StreamIngress::Raw => !metadata.key().is_raw(),
                StreamIngress::Pitch => pitch.is_none_or(|(cents, _)| cents == 0),
                StreamIngress::Repitch => metadata.key().repitch_identity().is_none(),
            } {
                return Err(AudioStorageError::SourceMismatch);
            }
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
impl<'a> OwnedAudioSourceView<'a> {
    pub fn sample_rate(&self) -> u32 {
        match self.kind {
            SourceKind::Resident(a) => a.sample_rate(),
            SourceKind::Streamed(s) => s.project_rate(),
        }
    }
    pub fn resident_frames(&self) -> Option<&'a [Frame]> {
        match self.kind {
            SourceKind::Resident(a) => Some(a.frames()),
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
#[derive(Clone, Copy)]
enum BankMode {
    Raw,
    Pitch,
    Derived,
}

impl OwnedAudioInputs {
    /// Scalar conservation observation only; no source/vector/owner escapes.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_bank_identity(&self) -> (usize, usize, u64, u64) {
        (
            self.sources.as_ptr() as usize,
            self.sources.capacity(),
            self.bank,
            self.custodian,
        )
    }
    pub fn try_new(
        sources: Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
    ) -> Result<Self, Box<AudioInputsRefusal>> {
        Self::try_new_with_mode(sources, reservation, BankMode::Raw)
    }
    /// Explicit bank supporting multiple derived pitches after each raw source.
    /// Entries are accounted independently; ordinary asset lookup stays raw.
    pub fn try_new_with_pitched(
        sources: Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
    ) -> Result<Self, Box<AudioInputsRefusal>> {
        Self::try_new_with_mode(sources, reservation, BankMode::Pitch)
    }
    /// Bank of raw sources and exact supported derived pitch/Re-Pitch identities.
    pub fn try_new_with_derived(
        sources: Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
    ) -> Result<Self, Box<AudioInputsRefusal>> {
        Self::try_new_with_mode(sources, reservation, BankMode::Derived)
    }
    fn try_new_with_mode(
        sources: Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
        mode: BankMode,
    ) -> Result<Self, Box<AudioInputsRefusal>> {
        let mut capacity = AudioPreparationCapacity {
            original_source_capacity: sources.capacity(),
            ..AudioPreparationCapacity::default()
        };
        match Self::prepare_sources(&sources, reservation, &mut capacity, mode) {
            Ok((aliases, bank)) => Ok(Self {
                sources,
                capacity,
                bank,
                custodian: 1,
                aliases,
                reservation,
            }),
            Err(reason) => Err(Box::new(AudioInputsRefusal {
                reason,
                sources,
                reservation,
                capacity,
            })),
        }
    }
    /// A host-held source collection remains in its persistent destination
    /// throughout every fallible bank validation/allocation. On success only
    /// fixed moves occur into the already-empty destination. No source owner is
    /// returned in a transient error or dropped on refusal.
    pub fn try_new_in_place(
        sources: &mut Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
        destination: &mut Option<Self>,
    ) -> Result<(), AudioStorageError> {
        Self::try_new_in_place_with_mode(sources, reservation, destination, BankMode::Raw)
    }
    /// In-place variant of the explicit pitch-aware bank. Refusal leaves all
    /// original source owners and their vector in the caller's custody.
    pub fn try_new_with_pitched_in_place(
        sources: &mut Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
        destination: &mut Option<Self>,
    ) -> Result<(), AudioStorageError> {
        Self::try_new_in_place_with_mode(sources, reservation, destination, BankMode::Pitch)
    }
    /// Transfer a derived-capable bank without losing owners on refusal.
    pub fn try_new_with_derived_in_place(
        sources: &mut Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
        destination: &mut Option<Self>,
    ) -> Result<(), AudioStorageError> {
        Self::try_new_in_place_with_mode(sources, reservation, destination, BankMode::Derived)
    }
    fn try_new_in_place_with_mode(
        sources: &mut Vec<(AssetId, OwnedAudioSource)>,
        reservation: AudioStorageReservation,
        destination: &mut Option<Self>,
        mode: BankMode,
    ) -> Result<(), AudioStorageError> {
        if destination.is_some() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let mut capacity = AudioPreparationCapacity {
            original_source_capacity: sources.capacity(),
            ..AudioPreparationCapacity::default()
        };
        let (aliases, bank) = Self::prepare_sources(sources, reservation, &mut capacity, mode)?;
        *destination = Some(Self {
            sources: std::mem::take(sources),
            capacity,
            bank,
            custodian: 1,
            aliases,
            reservation,
        });
        Ok(())
    }
    fn prepare_sources(
        sources: &[(AssetId, OwnedAudioSource)],
        reservation: AudioStorageReservation,
        capacity: &mut AudioPreparationCapacity,
        mode: BankMode,
    ) -> Result<(Vec<AliasRow>, u64), AudioStorageError> {
        let mut aliases = Vec::new();
        capacity.source_container_bytes = capacity
            .original_source_capacity
            .checked_mul(std::mem::size_of::<(AssetId, OwnedAudioSource)>())
            .ok_or(AudioStorageError::Overflow)?;
        let alias_limit = reservation
            .handle_obligations
            .checked_sub(sources.len())
            .ok_or(AudioStorageError::Capacity)?;
        // Reject a known-insufficient caller allowance before allocating the
        // alias backing; actual allocator capacity is still checked below.
        let requested = alias_limit
            .checked_mul(std::mem::size_of::<AliasRow>())
            .and_then(|n| n.checked_add(capacity.source_container_bytes))
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| n.checked_add(reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if requested > reservation.max_prepared_data_bytes
            || requested
                .checked_add(reservation.caller_construction_bytes)
                .is_none_or(|n| n > reservation.max_construction_data_bytes)
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
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
        let mut allocation_count = 0_usize;
        for (index, (_, source)) in sources.iter().enumerate() {
            if !sources[..index]
                .iter()
                .any(|(_, old)| crate::audio_input_storage::same(old, source))
            {
                allocation_count = allocation_count
                    .checked_add(match &source.kind {
                        SourceKind::Resident(_) => 2,
                        SourceKind::Streamed(_) => 1,
                    })
                    .ok_or(AudioStorageError::Overflow)?;
            }
        }
        if sources.len() > reservation.handle_obligations
            || allocation_count > reservation.allocation_items
        {
            return Err(AudioStorageError::Capacity);
        }
        for (index, (id, source)) in sources.iter().enumerate() {
            let derived = match &source.kind {
                SourceKind::Streamed(metadata) if !metadata.key().is_raw() => Some(metadata),
                _ => None,
            };
            if let Some(metadata) = derived {
                if metadata.key().asset() != *id {
                    return Err(AudioStorageError::SourceMismatch);
                }
                let raw = sources[..index]
                    .iter()
                    .find_map(|(previous, source)| {
                        if previous != id {
                            return None;
                        }
                        match &source.kind {
                            SourceKind::Streamed(raw) if raw.key().is_raw() => Some(raw),
                            _ => None,
                        }
                    })
                    .ok_or(AudioStorageError::SourceMismatch)?;
                let matches = if let Some((cents, law)) = metadata.key().pitch() {
                    !matches!(mode, BankMode::Raw)
                        && cents != 0
                        && raw
                            .with_plain_pitch(cents, law)
                            .is_ok_and(|expected| &expected == metadata.arc().as_ref())
                } else {
                    matches!(mode, BankMode::Derived) && metadata.is_repitch_of(raw)
                };
                if !matches {
                    return Err(AudioStorageError::SourceMismatch);
                }
                if sources[..index].iter().any(|(previous, source)| {
                    previous == id && matches!(&source.kind, SourceKind::Streamed(old) if old.key() == metadata.key())
                }) { return Err(AudioStorageError::DuplicateAsset); }
            } else if sources[..index].iter().any(|(previous, _)| previous == id) {
                return Err(AudioStorageError::DuplicateAsset);
            }
        }
        // These are only actual BANK container observations, never complete
        // graph/domain/allocator-memory totals or a prepared admission.
        let bank = handle()?;
        aliases.resize_with(
            reservation.handle_obligations - sources.len(),
            AliasRow::default,
        );
        Ok((aliases, bank))
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
    /// Nonowning token for an exact derived pitch; never falls back to raw PCM.
    pub fn pitched_source_token(
        &self,
        key: crate::StreamSourceKey,
    ) -> Result<AudioSourceToken, AudioStorageError> {
        if key.pitch().is_none() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let index = self.sources.iter().position(|(asset, source)| {
            *asset == key.asset() && matches!(&source.kind, SourceKind::Streamed(metadata) if metadata.key() == key)
        }).ok_or(AudioStorageError::SourceMismatch)?;
        Ok(AudioSourceToken {
            bank: self.bank,
            custodian: self.custodian,
            source: index,
            source_handle: self.sources[index].1.handle,
            alias: None,
        })
    }
    /// Nonowning token for an exact Re-Pitch source; never falls back to raw PCM.
    pub fn repitch_source_token(
        &self,
        key: crate::StreamSourceKey,
    ) -> Result<AudioSourceToken, AudioStorageError> {
        if key.repitch_identity().is_none() {
            return Err(AudioStorageError::SourceMismatch);
        }
        let index = self.sources.iter().position(|(asset, source)| *asset == key.asset()
            && matches!(&source.kind, SourceKind::Streamed(metadata) if metadata.key() == key))
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
    #[test]
    fn pitch_aware_bank_retains_exact_sources_and_accounts_each_owner() {
        if !constructor_supported("pitched_source_bank") {
            return;
        }
        let asset = ids()[0];
        let raw = StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(7, asset),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let up = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        let down = raw
            .with_plain_pitch(-300, hex_project::WarpResampler::V2)
            .unwrap();
        let source =
            |m: &StreamedSourceMetadata| OwnedAudioSource::try_streamed(m.clone()).ok().unwrap();
        let pitch = |m: &StreamedSourceMetadata| {
            OwnedAudioSource::try_pitched_streamed(m.clone())
                .ok()
                .unwrap()
        };
        let mut bank = OwnedAudioInputs::try_new_with_pitched(
            vec![
                (asset, source(&raw)),
                (asset, pitch(&up)),
                (asset, pitch(&down)),
            ],
            reservation(),
        )
        .ok()
        .unwrap();
        let raw_token = bank.source_token(asset).unwrap();
        let up_token = bank.pitched_source_token(up.key()).unwrap();
        let down_token = bank.pitched_source_token(down.key()).unwrap();
        assert_eq!(
            bank.source_view(&raw_token).unwrap().streamed_metadata(),
            Some(&raw)
        );
        assert_eq!(
            bank.source_view(&up_token).unwrap().streamed_metadata(),
            Some(&up)
        );
        assert_eq!(
            bank.source_view(&down_token).unwrap().streamed_metadata(),
            Some(&down)
        );
        assert!(bank.pitched_source_token(raw.key()).is_err());
        assert!(
            bank.pitched_source_token(
                crate::StreamSourceKey::plain_pitch(8, asset, 700, hex_project::WarpResampler::V2)
                    .unwrap()
            )
            .is_err()
        );
        let alias = bank.alias_on_control(&up_token).unwrap();
        assert_eq!(
            bank.source_view(&alias).unwrap().streamed_metadata(),
            Some(&up)
        );
        let storage = bank.prepare_input_storage_on_control().ok().unwrap();
        assert_eq!(storage.summary().unique_allocations, 3);
        assert_eq!(storage.summary().unresolved_handles, 4);
        assert!(storage.validate_unresolved_on_control().is_ok());
        let raw_after = storage.source_token(asset).unwrap();
        assert_eq!(
            storage.source_view(&raw_after).unwrap().streamed_metadata(),
            Some(&raw)
        );
        let pitch_after = storage.pitched_source_token(up.key()).unwrap();
        assert_eq!(
            storage
                .source_view(&pitch_after)
                .unwrap()
                .streamed_metadata(),
            Some(&up)
        );
    }

    #[test]
    fn pitch_aware_bank_refuses_foreign_duplicate_or_unanchored_sources_without_moving_them() {
        if !constructor_supported("pitched_source_refusal") {
            return;
        }
        let ids = ids();
        let raw = StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(7, ids[0]),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let up = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        assert_eq!(
            OwnedAudioSource::try_pitched_streamed(raw.clone())
                .err()
                .unwrap()
                .reason,
            AudioStorageError::SourceMismatch
        );
        let raw_source = || OwnedAudioSource::try_streamed(raw.clone()).ok().unwrap();
        let pitch_source = |m: &StreamedSourceMetadata| {
            OwnedAudioSource::try_pitched_streamed(m.clone())
                .ok()
                .unwrap()
        };
        assert_eq!(
            OwnedAudioInputs::try_new(vec![(ids[0], pitch_source(&up))], reservation())
                .err()
                .unwrap()
                .reason,
            AudioStorageError::SourceMismatch
        );
        assert_eq!(
            OwnedAudioInputs::try_new_with_pitched(
                vec![(ids[0], pitch_source(&up)), (ids[0], raw_source())],
                reservation()
            )
            .err()
            .unwrap()
            .reason,
            AudioStorageError::SourceMismatch
        );
        assert_eq!(
            OwnedAudioInputs::try_new_with_pitched(
                vec![
                    (ids[0], raw_source()),
                    (ids[0], pitch_source(&up)),
                    (ids[0], pitch_source(&up))
                ],
                reservation()
            )
            .err()
            .unwrap()
            .reason,
            AudioStorageError::DuplicateAsset
        );
        assert_eq!(
            OwnedAudioInputs::try_new_with_pitched(
                vec![(ids[0], raw_source()), (ids[1], pitch_source(&up))],
                reservation()
            )
            .err()
            .unwrap()
            .reason,
            AudioStorageError::SourceMismatch
        );
        let foreign = StreamedSourceMetadata::new(
            raw.key(),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [8; 32],
        )
        .unwrap()
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
        let mut sources = Vec::with_capacity(5);
        sources.push((ids[0], raw_source()));
        sources.push((ids[0], pitch_source(&foreign)));
        let backing = (sources.as_ptr(), sources.capacity());
        let handles: Vec<_> = sources.iter().map(|(_, owner)| owner.handle).collect();
        let mut destination = None;
        assert_eq!(
            OwnedAudioInputs::try_new_with_pitched_in_place(
                &mut sources,
                reservation(),
                &mut destination
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(destination.is_none());
        assert_eq!((sources.as_ptr(), sources.capacity()), backing);
        assert_eq!(
            sources
                .iter()
                .map(|(_, owner)| owner.handle)
                .collect::<Vec<_>>(),
            handles
        );
    }
    #[test]
    fn repitch_bank_refuses_wrong_raw_without_moving_owners() {
        if !constructor_supported("repitch_source_bank") {
            return;
        }
        let asset = ids()[0];
        let raw = StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(7, asset),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let other = StreamedSourceMetadata::new(
            raw.key(),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [8; 32],
        )
        .unwrap();
        let map = hex_warp::TimeMap::new(
            vec![hex_warp::MapSegment {
                output_start: 0,
                output_end: 8,
                source_start_q32: 0,
                source_end_q32: 8_i128 << 32,
            }],
            8,
        )
        .unwrap();
        let derived = raw.with_repitch(&map, hex_warp::Resampler::V2).unwrap();
        assert!(OwnedAudioSource::try_streamed(derived.clone()).is_err());
        assert!(OwnedAudioSource::try_pitched_streamed(derived.clone()).is_err());
        let mut sources = vec![
            (asset, OwnedAudioSource::try_streamed(other).ok().unwrap()),
            (
                asset,
                OwnedAudioSource::try_repitch_streamed(derived.clone())
                    .ok()
                    .unwrap(),
            ),
        ];
        let backing = (sources.as_ptr(), sources.capacity());
        let handles: Vec<_> = sources.iter().map(|(_, owner)| owner.handle).collect();
        let mut destination = None;
        assert_eq!(
            OwnedAudioInputs::try_new_with_derived_in_place(
                &mut sources,
                reservation(),
                &mut destination
            ),
            Err(AudioStorageError::SourceMismatch)
        );
        assert!(destination.is_none());
        assert_eq!((sources.as_ptr(), sources.capacity()), backing);
        assert_eq!(
            sources
                .iter()
                .map(|(_, owner)| owner.handle)
                .collect::<Vec<_>>(),
            handles
        );
        let pitch_only = vec![
            (
                asset,
                OwnedAudioSource::try_streamed(raw.clone()).ok().unwrap(),
            ),
            (
                asset,
                OwnedAudioSource::try_repitch_streamed(derived.clone())
                    .ok()
                    .unwrap(),
            ),
        ];
        assert!(OwnedAudioInputs::try_new_with_pitched(pitch_only, reservation()).is_err());
        let bank = OwnedAudioInputs::try_new_with_derived(
            vec![
                (asset, OwnedAudioSource::try_streamed(raw).ok().unwrap()),
                (
                    asset,
                    OwnedAudioSource::try_repitch_streamed(derived.clone())
                        .ok()
                        .unwrap(),
                ),
            ],
            reservation(),
        )
        .ok()
        .unwrap();
        let token = bank.repitch_source_token(derived.key()).unwrap();
        assert_eq!(
            bank.source_view(&token).unwrap().streamed_metadata(),
            Some(&derived)
        );
    }

    #[test]
    fn raw_asset_ingress_refuses_derived_pitch_and_returns_metadata() {
        let raw = StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(7, ids()[0]),
            48_000,
            48_000,
            8,
            8,
            hex_project::IDENTITY_CONVERSION,
            [7; 32],
        )
        .unwrap();
        let pitched = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        let refusal = match OwnedAudioSource::try_streamed(pitched.clone()) {
            Ok(_) => panic!("derived audio must not enter the raw asset bank"),
            Err(refusal) => refusal,
        };
        assert_eq!(refusal.reason, AudioStorageError::SourceMismatch);
        assert_eq!(refusal.metadata, pitched);
    }
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
    fn constructor_supported(test: &str) -> bool {
        if crate::pcm_arc::native_layout_known() {
            eprintln!("hex audio bank evidence: {test}; branch=supported-constructor");
            return true;
        }
        let refusal = OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]])
            .err()
            .unwrap();
        assert_eq!(refusal.reason, AudioStorageError::UnsupportedConstructor);
        eprintln!("hex audio bank evidence: {test}; branch=unsupported-refusal");
        false
    }
    #[test]
    fn in_place_bank_refusal_and_transfer_preserve_actual_collection_backing() {
        if !constructor_supported("rooted_source_collection") {
            return;
        }
        let ids = ids();
        let source = OwnedAudioSource::try_resident(48_000, vec![[0.5; 2]; 8])
            .ok()
            .unwrap();
        let handle = source.handle;
        let mut sources = Vec::with_capacity(5);
        sources.push((ids[0], source));
        let original = (sources.as_ptr(), sources.capacity());
        let mut destination = None;
        let mut insufficient = reservation();
        insufficient.handle_obligations = 0;
        assert_eq!(
            OwnedAudioInputs::try_new_in_place(&mut sources, insufficient, &mut destination),
            Err(AudioStorageError::Capacity)
        );
        assert_eq!((sources.as_ptr(), sources.capacity()), original);
        assert_eq!(sources[0].1.handle, handle);
        assert!(destination.is_none());
        let mut insufficient = reservation();
        insufficient.max_prepared_data_bytes = 1;
        assert_eq!(
            OwnedAudioInputs::try_new_in_place(&mut sources, insufficient, &mut destination),
            Err(AudioStorageError::ReservationExceeded)
        );
        assert_eq!((sources.as_ptr(), sources.capacity()), original);
        assert_eq!(sources[0].1.handle, handle);
        assert!(destination.is_none());
        OwnedAudioInputs::try_new_in_place(&mut sources, reservation(), &mut destination).unwrap();
        assert!(sources.is_empty());
        assert_eq!(sources.capacity(), 0);
        let bank = destination.as_ref().unwrap();
        assert_eq!((bank.sources.as_ptr(), bank.sources.capacity()), original);
        assert_eq!(bank.sources[0].1.handle, handle);
        let bank_identity = bank.test_bank_identity();
        assert_eq!(
            OwnedAudioInputs::try_new_in_place(&mut sources, reservation(), &mut destination),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            destination.as_ref().unwrap().test_bank_identity(),
            bank_identity
        );
    }
    #[test]
    fn retained_vec_capacity_and_wrapper_are_independent_charges_without_pcm_copy() {
        if !constructor_supported("retained_vec_pair") {
            return;
        }
        let mut frames = Vec::with_capacity(73);
        frames.resize(8, [0.5; 2]);
        let pointer = frames.as_ptr();
        let capacity = frames.capacity();
        let source = OwnedAudioSource::try_resident(48_000, frames).ok().unwrap();
        let SourceKind::Resident(audio) = &source.kind else {
            panic!("resident");
        };
        assert_eq!(audio.frames.values.as_ptr(), pointer);
        let charges = audio
            .pcm_arc_receipt
            .unwrap()
            .checked_allocations(audio)
            .unwrap();
        assert_eq!(charges[0].0, std::mem::size_of::<crate::AudioData>());
        assert_eq!(charges[1], (capacity * std::mem::size_of::<Frame>(), 0));
        assert!(charges[1].0 > audio.frames.len() * std::mem::size_of::<Frame>());
        let ids = ids();
        let mut bank = OwnedAudioInputs::try_new(vec![(ids[0], source)], reservation())
            .ok()
            .unwrap();
        let token = bank.source_token(ids[0]).unwrap();
        let _first = bank.alias_on_control(&token).unwrap();
        let _second = bank.alias_on_control(&token).unwrap();
        let output = bank.prepare_input_storage_on_control().ok().unwrap();
        assert_eq!(output.summary().unique_allocations, 2);
        assert_eq!(output.summary().unresolved_handles, 3);
        assert_eq!(
            output.summary().declared_source_data_bytes,
            charges.iter().map(|(p, o)| p + o).sum()
        );
        output.validate_unresolved_on_control().unwrap();
    }
    #[test]
    fn raw_constructor_never_inherits_sealed_resident_provenance() {
        let raw = OwnedAudioSource {
            kind: SourceKind::Resident(Audio::new(48_000, vec![[0.0; 2]; 8]).unwrap()),
            handle: handle().unwrap(),
        };
        assert_eq!(
            crate::audio_input_storage::parts(&raw),
            Err(AudioStorageError::UnknownStorage)
        );
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
        if !constructor_supported("bank_aliases") {
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
        eprintln!("hex audio bank evidence: bank_aliases; supported-controls=completed");
    }
    #[test]
    fn duplicate_and_byte_refusal_return_original_source_vector_and_capacity() {
        if !constructor_supported("bank_duplicate_and_bytes") {
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
        eprintln!(
            "hex audio bank evidence: bank_duplicate_and_bytes; supported-controls=completed"
        );
    }
    #[test]
    fn typed_bank_data_dedups_allocations_never_handles_and_commit_invalidates_old_tokens() {
        if !constructor_supported("typed_bank_data") {
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
        assert_eq!(output.summary().unique_allocations, 4); // two allocations per distinct PCM owner
        assert_eq!(output.summary().unresolved_handles, 4);
        assert!(output.validate_unresolved_on_control().is_ok());
        assert!(output.source_view(&token).is_err());
        assert!(
            output
                .source_view(&output.source_token(ids[0]).unwrap())
                .is_ok()
        );
        eprintln!("hex audio bank evidence: typed_bank_data; supported-controls=completed");
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
            eprintln!("hex audio bank evidence: streamed_bank; branch=unsupported-refusal");
            return;
        }
        let source = OwnedAudioSource::try_streamed(metadata.clone())
            .ok()
            .unwrap();
        eprintln!("hex audio bank evidence: streamed_bank; branch=supported-constructor");
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
        eprintln!("hex audio bank evidence: streamed_bank; supported-controls=completed");
    }
    #[test]
    fn borrowed_ledger_conserves_capacity_distinct_owners_and_caller_aliases() {
        if !constructor_supported("borrowed_ledger_aliases") {
            return;
        }
        let ids = ids();
        let make = || {
            let mut pcm = Vec::with_capacity(73);
            pcm.resize(8, [0.5; 2]);
            OwnedAudioSource::try_resident(48_000, pcm).ok().unwrap()
        };
        let mut bank =
            OwnedAudioInputs::try_new(vec![(ids[0], make()), (ids[1], make())], reservation())
                .ok()
                .unwrap();
        let token = bank.source_token(ids[0]).unwrap();
        bank.alias_on_control(&token).unwrap();
        bank.alias_on_control(&token).unwrap();
        let expected_data = std::mem::size_of::<OwnedAudioInputs>()
            + bank.capacity.source_container_bytes
            + bank.capacity.alias_container_bytes
            + bank
                .sources
                .iter()
                .map(|(_, s)| {
                    let (p, o) = crate::audio_input_storage::parts(s).unwrap();
                    p + o
                })
                .sum::<usize>();
        let r = crate::AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        };
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
        ledger.borrow_caller_inputs_on_control(&bank).unwrap();
        let summary = ledger.summary().unwrap();
        assert_eq!(summary.unresolved_handles, 4);
        assert_eq!(summary.data_allocations, 6);
        assert_eq!(summary.data_bytes, expected_data);
        assert_eq!(summary.unknown_storage, 0);
        drop(ledger);
        assert!(bank.source_view(&token).is_ok());
        let mut bad = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
        bad.borrow_caller_inputs_on_control(&bank).unwrap();
        assert_eq!(
            bad.borrow_caller_inputs_on_control(&bank),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(bad.summary(), Err(AudioStorageError::UnknownStorage));
    }

    #[test]
    fn borrowed_ledger_refuses_raw_weak_and_ambiguous_caller_costs() {
        if !constructor_supported("borrowed_ledger_refusals") {
            return;
        }
        let ids = ids();
        let r = crate::AudioStorageLedgerReservation {
            data_rows: 32,
            handle_rows: 16,
            root_rows: 4,
            max_retained_bytes: usize::MAX,
            max_construction_peak_bytes: usize::MAX,
        };
        for raw in [false, true] {
            let mut bank = OwnedAudioInputs::try_new(
                vec![(
                    ids[0],
                    OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]; 8])
                        .ok()
                        .unwrap(),
                )],
                reservation(),
            )
            .ok()
            .unwrap();
            let SourceKind::Resident(audio) = &mut bank.sources[0].1.kind else {
                panic!("resident")
            };
            if raw {
                audio.pcm_arc_receipt = None;
            }
            let observer = std::sync::Arc::downgrade(&audio.frames);
            let pointer = bank.sources.as_ptr();
            let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
            assert_eq!(
                ledger.borrow_caller_inputs_on_control(&bank),
                Err(AudioStorageError::UnknownStorage)
            );
            drop(ledger);
            drop(observer);
            assert_eq!(bank.sources.as_ptr(), pointer);
            if raw {
                let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
                assert_eq!(
                    ledger.borrow_caller_inputs_on_control(&bank),
                    Err(AudioStorageError::UnknownStorage)
                );
            }
            if !raw {
                let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
                ledger.borrow_caller_inputs_on_control(&bank).unwrap();
                assert!(ledger.summary().unwrap().require_known_storage().is_ok());
            }
        }
        let mut bank = OwnedAudioInputs::try_new(
            vec![(
                ids[0],
                OwnedAudioSource::try_resident(48_000, vec![[0.0; 2]; 8])
                    .ok()
                    .unwrap(),
            )],
            reservation(),
        )
        .ok()
        .unwrap();
        bank.reservation.caller_retained_bytes = 1;
        let mut ledger = crate::AudioRootStorageLedger::try_new_on_control(r).unwrap();
        ledger.borrow_caller_inputs_on_control(&bank).unwrap();
        assert_eq!(
            ledger.summary().unwrap().require_known_storage(),
            Err(crate::AudioStorageIncomplete::UnknownStorage)
        );
    }
}
