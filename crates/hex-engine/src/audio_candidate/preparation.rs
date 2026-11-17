//! A host's persistent, unpublished construction destination. No Drop cleanup
//! and no owner getter: cancellation must retain this cell until explicit normal
//! cleanup, while a failed cell remains quarantined by its host.
use super::*;
use hex_project::{AssetId, Project};
use std::{collections::BTreeMap, sync::Arc};

/// Control-only borrowed compiler context. The host retains the actual source
/// owners behind these definitions; this view grants no native admission and
/// never escapes into the prepared graph. Session consumers use the Engine
/// contract without acquiring a device registry or native lifecycle of their own.
/// The same borrowed facts may compile ordinary registered resources and file
/// catalogs; that does not opt those owners into the closed constructor.
pub struct AudioCandidateEffects<'a, 'definitions> {
    registry: &'a hex_lattice::NativeDeviceRegistry<'definitions>,
}
impl<'a, 'definitions> AudioCandidateEffects<'a, 'definitions> {
    pub fn new(registry: &'a hex_lattice::NativeDeviceRegistry<'definitions>) -> Self {
        Self { registry }
    }
    /// Compose host definitions with the builtins for one synchronous control
    /// borrow. Neither the registry nor a native owner escapes this callback.
    pub fn with_host_definitions<T>(
        additional: &[&dyn hex_lattice::NativeDeviceDefinition],
        prepare: impl FnOnce(&AudioCandidateEffects<'_, '_>) -> Result<T, crate::Invalid>,
    ) -> Result<T, crate::Invalid> {
        let builtins = hex_devices::native_registry()
            .map_err(|_| crate::Invalid("invalid built-in native device registry"))?;
        let count = builtins
            .definitions()
            .len()
            .checked_add(additional.len())
            .filter(|count| *count <= hex_lattice_contract::MAX_NATIVE_DEVICE_TYPES)
            .ok_or(crate::Invalid("host effect registry capacity exceeded"))?;
        let mut definitions = Vec::new();
        definitions
            .try_reserve_exact(count)
            .map_err(|_| crate::Invalid("effect registry allocation refused"))?;
        definitions.extend_from_slice(builtins.definitions());
        definitions.extend_from_slice(additional);
        let registry = hex_lattice::NativeDeviceRegistry::new(&definitions)
            .map_err(|_| crate::Invalid("invalid host effect registry"))?;
        prepare(&AudioCandidateEffects::new(&registry))
    }
    /// Compile resident clips with the original registered instrument resource
    /// set. This borrows facts only; native custody remains in the audio owner.
    #[allow(clippy::too_many_arguments)] // Existing complete registered preparation inputs.
    pub fn prepare_registered_revision(
        &self,
        project: &Project,
        media: &BTreeMap<AssetId, crate::Audio>,
        table: Arc<crate::SineTable>,
        previous: Option<&crate::PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<crate::PreparedRevision, crate::Invalid> {
        crate::PreparedRevision::prepare_with_device_resources_and_effect_registry(
            project,
            media,
            table,
            previous,
            transforms,
            resources,
            scan_frames,
            cancel,
            self.registry,
            native_input,
        )
    }
    /// Same ordinary resource preparation with verified file-source metadata.
    /// The host still supplies actual pages and native endpoints at publication.
    #[allow(clippy::too_many_arguments)] // Existing mixed-file/resource inputs.
    pub fn prepare_registered_source_revision(
        &self,
        project: &Project,
        sources: &BTreeMap<AssetId, crate::compile::PreparedAudioSource>,
        generation: u64,
        table: Arc<crate::SineTable>,
        previous: Option<&crate::PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<crate::PreparedRevision, crate::Invalid> {
        crate::PreparedRevision::prepare_with_prepared_audio_sources_device_resources_and_effect_registry(
            project, sources, generation, table, previous, transforms, resources,
            scan_frames, cancel, self.registry, native_input,
        )
    }
    /// Same resource/effect preparation with exact separately verified pitch
    /// metadata. Page and native source custody remain with the host.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_registered_pitch_source_revision(
        &self,
        project: &Project,
        sources: &BTreeMap<AssetId, crate::compile::PreparedAudioSource>,
        pitched: &BTreeMap<crate::StreamSourceKey, Arc<crate::StreamedSourceMetadata>>,
        generation: u64,
        table: Arc<crate::SineTable>,
        previous: Option<&crate::PreparedSessionInventory>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        resources: &crate::native_device_resources::PreparedDeviceResources,
        scan_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
        native_input: Option<crate::NativeParameterInputTarget>,
    ) -> Result<crate::PreparedRevision, crate::Invalid> {
        crate::PreparedRevision::prepare_with_prepared_pitch_sources_device_resources_and_effect_registry(
            project, sources, pitched, generation, table, previous, transforms, resources,
            scan_frames, cancel, self.registry, native_input,
        )
    }
    pub fn quote(
        &self,
        project: &Project,
        sources: usize,
    ) -> Result<super::AudioCandidateReservationQuote, AudioStorageError> {
        super::AudioCandidateReservationQuote::for_project_with_effect_registry(
            project,
            sources,
            self.registry,
        )
    }
    /// Quote a candidate containing both effects and verified derived streams.
    pub fn quote_with_pitched_streams(
        &self,
        project: &Project,
        sources: usize,
        streams: &[crate::StreamedSourceMetadata],
    ) -> Result<super::AudioCandidateReservationQuote, AudioStorageError> {
        super::AudioCandidateReservationQuote::for_project_with_effects_and_streams(
            project,
            sources,
            streams,
            self.registry,
        )
    }
    /// Add only the armed slot/growth delta to the existing complete caller quote.
    /// Existing device row envelopes already reserve each automation vector family.
    pub fn quote_with_native_parameter_input(
        &self,
        project: &Project,
        sources: usize,
        input: crate::device_automation::NativeParameterInputTarget,
    ) -> Result<super::AudioCandidateReservationQuote, AudioStorageError> {
        if crate::device_automation::native_parameter_input_target(
            project,
            input.device(),
            input.target(),
        )
        .map_err(|_| AudioStorageError::UnsupportedGraph)?
            != input
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let device = project
            .device(input.device())
            .ok_or(AudioStorageError::SourceMismatch)?;
        let retained = crate::device_automation::quote_with_input(device, Some(input))?
            .checked_sub(crate::device_automation::quote(device)?)
            .ok_or(AudioStorageError::Overflow)?;
        let growth =
            crate::device_automation::input_construction_growth_overlap(device, Some(input))?;
        let mut quote = self.quote(project, sources)?;
        // The optional runtime Box adds one exact exclusive DATA row; it is
        // not an independently transferable processor or shared HANDLE.
        quote.data_rows = quote
            .data_rows
            .checked_add(1)
            .ok_or(AudioStorageError::Overflow)?;
        quote.metadata_data_bytes = quote
            .metadata_data_bytes
            .checked_add(crate::SessionPlayback::native_parameter_input_state_bytes())
            .and_then(|bytes| bytes.checked_add(retained))
            .ok_or(AudioStorageError::Overflow)?;
        quote.compiler_workspace_bytes = quote
            .compiler_workspace_bytes
            .checked_add(growth)
            .ok_or(AudioStorageError::Overflow)?;
        Ok(quote)
    }
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_native_parameter_input(
        &self,
        destination: &mut AudioCandidatePreparation,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
        input: crate::device_automation::NativeParameterInputTarget,
    ) -> Result<(), AudioStorageError> {
        destination.prepare_with_effect_registry_and_native_parameter_input_on_control(
            project,
            source_generation,
            context,
            self.registry,
            sampler,
            input,
        )
    }
    pub fn prepare(
        &self,
        destination: &mut AudioCandidatePreparation,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
    ) -> Result<(), AudioStorageError> {
        destination.prepare_with_effect_registry_on_control(
            project,
            source_generation,
            context,
            self.registry,
            sampler,
        )
    }
}

pub struct AudioCandidatePreparation {
    sources: Vec<(AssetId, crate::OwnedAudioSource)>,
    resident_draft: Vec<crate::Frame>,
    reservation: crate::AudioStorageReservation,
    bank: Option<OwnedAudioInputs>,
    custody: Option<AudioPreparationCustody>,
    token: Option<AudioPreparationToken>,
    draft: Option<AudioCandidateDraft>,
    owner: Option<AudioCandidateOwner>,
    owner_runtime_done: bool,
    cleaning: bool,
    complete: bool,
    failed: bool,
    first_panic: Option<Box<dyn Any + Send>>,
}
impl AudioCandidatePreparation {
    /// The host reserves the actual runtime slot and this full construction
    /// allowance before calling. No source or graph owner is consumed here.
    pub fn try_new_on_control(
        source_capacity: usize,
        reservation: crate::AudioStorageReservation,
    ) -> Result<Self, AudioStorageError> {
        let requested = source_capacity
            .checked_mul(std::mem::size_of::<(AssetId, crate::OwnedAudioSource)>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| {
                n.checked_add(
                    reservation
                        .handle_obligations
                        .checked_mul(std::mem::size_of::<crate::audio_only_inputs::AliasRow>())?,
                )
            })
            .and_then(|n| n.checked_add(reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        if source_capacity > reservation.handle_obligations
            || requested > reservation.max_prepared_data_bytes
            || requested
                .checked_add(reservation.caller_construction_bytes)
                .is_none_or(|n| n > reservation.max_construction_data_bytes)
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        let mut result = Self {
            sources: Vec::new(),
            resident_draft: Vec::new(),
            reservation,
            bank: None,
            custody: Some(AudioPreparationCustody::try_new_on_control()?),
            token: None,
            draft: Some(AudioCandidateDraft::new_on_control()),
            owner: None,
            owner_runtime_done: false,
            cleaning: false,
            complete: false,
            failed: false,
            first_panic: None,
        };
        result
            .sources
            .try_reserve_exact(source_capacity)
            .map_err(|_| AudioStorageError::Allocation)?;
        result.validate_collection_bytes(0)?;
        Ok(result)
    }
    fn collecting(&self, asset: AssetId) -> Result<(), AudioStorageError> {
        self.collecting_slot()?;
        if self.sources.iter().any(|(id, _)| *id == asset) {
            return Err(AudioStorageError::DuplicateAsset);
        }
        Ok(())
    }
    fn collecting_slot(&self) -> Result<(), AudioStorageError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.bank.is_some()
            || self.owner.is_some()
            || self.sources.len() == self.sources.capacity()
            || self.resident_draft.capacity() != 0
        {
            return Err(AudioStorageError::Capacity);
        }
        Ok(())
    }
    fn validate_collection_bytes(&self, additional: usize) -> Result<(), AudioStorageError> {
        let data = self.sources.iter().try_fold(0usize, |n, (_, source)| {
            let (payload, overhead) = crate::audio_input_storage::parts(source)?;
            n.checked_add(payload)
                .and_then(|n| n.checked_add(overhead))
                .ok_or(AudioStorageError::Overflow)
        })?;
        let total = self
            .sources
            .capacity()
            .checked_mul(std::mem::size_of::<(AssetId, crate::OwnedAudioSource)>())
            .and_then(|n| n.checked_add(std::mem::size_of::<Self>()))
            .and_then(|n| {
                n.checked_add(
                    self.reservation
                        .handle_obligations
                        .checked_mul(std::mem::size_of::<crate::audio_only_inputs::AliasRow>())?,
                )
            })
            .and_then(|n| {
                n.checked_add(
                    self.resident_draft
                        .capacity()
                        .checked_mul(std::mem::size_of::<crate::Frame>())?,
                )
            })
            .and_then(|n| n.checked_add(data))
            .and_then(|n| n.checked_add(additional))
            .and_then(|n| n.checked_add(self.reservation.caller_retained_bytes))
            .ok_or(AudioStorageError::Overflow)?;
        let peak = total
            .checked_add(self.reservation.caller_construction_bytes)
            .ok_or(AudioStorageError::Overflow)?;
        if total > self.reservation.max_prepared_data_bytes
            || peak > self.reservation.max_construction_data_bytes
        {
            return Err(AudioStorageError::ReservationExceeded);
        }
        Ok(())
    }
    /// Copies a control-side resident value into a fresh constructor allocation;
    /// the caller's legacy Audio Arc is never adopted as native custody.
    pub fn copy_resident_source_on_control(
        &mut self,
        asset: AssetId,
        audio: &crate::Audio,
    ) -> Result<(), AudioStorageError> {
        self.copy_resident_frames_on_control(asset, audio.sample_rate(), audio.frames())
    }
    /// Copy only the borrowed original-rate leaf into this actual persistent
    /// candidate cell. No source owner, file path or project-rate derivative is
    /// adopted, and no borrow survives the synchronous control operation.
    pub fn copy_original_source_on_control(
        &mut self,
        asset: AssetId,
        source: crate::OwnedAudioSourceView<'_>,
    ) -> Result<(), AudioStorageError> {
        let frames = source
            .resident_frames()
            .ok_or(AudioStorageError::UnsupportedGraph)?;
        self.copy_resident_frames_on_control(asset, source.sample_rate(), frames)
    }
    fn copy_resident_frames_on_control(
        &mut self,
        asset: AssetId,
        sample_rate: u32,
        frames: &[crate::Frame],
    ) -> Result<(), AudioStorageError> {
        self.collecting(asset)?;
        if !self.resident_draft.is_empty() || self.resident_draft.capacity() != 0 {
            return Err(AudioStorageError::SourceMismatch);
        }
        let requested = frames
            .len()
            .checked_mul(std::mem::size_of::<crate::Frame>())
            .and_then(|n| n.checked_add(std::mem::size_of::<crate::AudioData>()))
            .ok_or(AudioStorageError::Overflow)?;
        self.validate_collection_bytes(requested)?;
        self.resident_draft
            .try_reserve_exact(frames.len())
            .map_err(|_| AudioStorageError::Allocation)?;
        self.validate_collection_bytes(std::mem::size_of::<crate::AudioData>())?;
        self.resident_draft.extend_from_slice(frames);
        // This fixed constructor has no user code or recoverable unwind after
        // taking the Vec; all Result refusals return that exact Vec. Fatal OOM
        // follows the existing constructor convention.
        match crate::OwnedAudioSource::try_resident(
            sample_rate,
            std::mem::take(&mut self.resident_draft),
        ) {
            Ok(source) => {
                self.sources.push((asset, source));
                Ok(())
            }
            Err(refused) => {
                self.resident_draft = refused.frames;
                Err(refused.reason)
            }
        }
    }
    pub fn add_streamed_source_on_control(
        &mut self,
        metadata: crate::StreamedSourceMetadata,
    ) -> Result<(), AudioStorageError> {
        let asset = metadata.key().asset();
        self.collecting(asset)?;
        self.validate_collection_bytes(std::mem::size_of::<crate::StreamedSourceMetadata>())?;
        let source =
            crate::OwnedAudioSource::try_streamed(metadata).map_err(|refused| refused.reason)?;
        self.sources.push((asset, source));
        Ok(())
    }
    /// Add a derived stream only for an already collected matching raw source.
    pub fn add_pitched_streamed_source_on_control(
        &mut self,
        metadata: crate::StreamedSourceMetadata,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_only_inputs::SourceKind;

        self.collecting_slot()?;
        let key = metadata.key();
        let (cents, law) = key.pitch().ok_or(AudioStorageError::SourceMismatch)?;
        let raw = self
            .sources
            .iter()
            .find_map(|(asset, source)| {
                if *asset != key.asset() {
                    return None;
                }
                match &source.kind {
                    SourceKind::Streamed(raw) if raw.key().is_raw() => Some(raw),
                    _ => None,
                }
            })
            .ok_or(AudioStorageError::SourceMismatch)?;
        if cents == 0
            || !raw
                .with_plain_pitch(cents, law)
                .is_ok_and(|expected| expected == metadata)
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.sources.iter().any(
            |(_, source)| matches!(&source.kind, SourceKind::Streamed(old) if old.key() == key),
        ) {
            return Err(AudioStorageError::DuplicateAsset);
        }
        self.validate_collection_bytes(std::mem::size_of::<crate::StreamedSourceMetadata>())?;
        let source = crate::OwnedAudioSource::try_pitched_streamed(metadata)
            .map_err(|refused| refused.reason)?;
        self.sources.push((key.asset(), source));
        Ok(())
    }
    /// Retain a Re-Pitch stream only alongside its exact original source facts.
    pub fn add_repitch_streamed_source_on_control(
        &mut self,
        metadata: crate::StreamedSourceMetadata,
    ) -> Result<(), AudioStorageError> {
        use crate::audio_only_inputs::SourceKind;
        self.collecting_slot()?;
        let key = metadata.key();
        let raw = self
            .sources
            .iter()
            .find_map(|(asset, source)| {
                if *asset != key.asset() {
                    return None;
                }
                match &source.kind {
                    SourceKind::Streamed(raw) if raw.key().is_raw() => Some(raw),
                    _ => None,
                }
            })
            .ok_or(AudioStorageError::SourceMismatch)?;
        if !metadata.is_repitch_of(raw) {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.sources.iter().any(
            |(_, source)| matches!(&source.kind, SourceKind::Streamed(old) if old.key() == key),
        ) {
            return Err(AudioStorageError::DuplicateAsset);
        }
        self.validate_collection_bytes(std::mem::size_of::<crate::StreamedSourceMetadata>())?;
        let source = crate::OwnedAudioSource::try_repitch_streamed(metadata)
            .map_err(|refused| refused.reason)?;
        self.sources.push((key.asset(), source));
        Ok(())
    }
    /// Low-level exact Project/context bridge. Session snapshots call this with
    /// their paired canonical Project and counters; numbers grant no custody.
    pub fn prepare_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
    ) -> Result<(), AudioStorageError> {
        self.prepare_with_sampler(project, source_generation, context, None, None, None)
    }
    /// Explicit closed native CURRENT construction from already copied original
    /// PCM. Generic audio-only preparation still refuses resourceful graphs.
    pub fn prepare_current_sampler_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        hash_frames: u64,
        cancel: &std::sync::atomic::AtomicBool,
    ) -> Result<(), AudioStorageError> {
        self.prepare_with_sampler(
            project,
            source_generation,
            context,
            Some((hash_frames, cancel)),
            None,
            None,
        )
    }
    /// Closed effect metadata preparation. Definitions are borrowed only for
    /// scalar foreign facts; source/worker/endpoint custody remains with audio.
    /// The optional original sampler work uses the same current source bank.
    pub fn prepare_with_effect_registry_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
    ) -> Result<(), AudioStorageError> {
        self.prepare_with_sampler(
            project,
            source_generation,
            context,
            sampler,
            Some(effects),
            None,
        )
    }
    /// Prepare one transient native input through the same original-bank custody.
    /// No authored points, provider owner or publication authority are fabricated.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare_with_effect_registry_and_native_parameter_input_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        effects: &hex_lattice::NativeDeviceRegistry<'_>,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
        input: crate::device_automation::NativeParameterInputTarget,
    ) -> Result<(), AudioStorageError> {
        self.prepare_with_sampler(
            project,
            source_generation,
            context,
            sampler,
            Some(effects),
            Some(input),
        )
    }
    #[allow(clippy::too_many_arguments)]
    fn prepare_with_sampler(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
        sampler: Option<(u64, &std::sync::atomic::AtomicBool)>,
        effects: Option<&hex_lattice::NativeDeviceRegistry<'_>>,
        native_input: Option<crate::device_automation::NativeParameterInputTarget>,
    ) -> Result<(), AudioStorageError> {
        if native_input.is_some() && effects.is_none() {
            return Err(AudioStorageError::UnsupportedGraph);
        }
        if self.failed
            || self.cleaning
            || self.complete
            || self.owner.is_some()
            || self.resident_draft.capacity() != 0
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        if self.bank.is_none() && self.token.is_none() {
            OwnedAudioInputs::try_new_with_derived_in_place(
                &mut self.sources,
                self.reservation,
                &mut self.bank,
            )?;
        }
        let custody = self
            .custody
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if self.token.is_none() {
            self.token = Some(
                if let Some(effects) = effects {
                    if native_input.is_some() {
                        custody.begin_effect_revision_with_input_in_place_on_control(
                            project,
                            source_generation,
                            context,
                            &mut self.bank,
                            sampler,
                            effects,
                            native_input,
                        )
                    } else {
                        custody.begin_effect_revision_in_place_on_control(
                            project,
                            source_generation,
                            context,
                            &mut self.bank,
                            sampler,
                            effects,
                        )
                    }
                } else {
                    match sampler {
                        Some((work, cancel)) => custody.begin_current_sampler_in_place_on_control(
                            project,
                            source_generation,
                            context,
                            &mut self.bank,
                            work,
                            cancel,
                        ),
                        None => custody.begin_prepared_revision_in_place_on_control(
                            project,
                            source_generation,
                            context,
                            &mut self.bank,
                        ),
                    }
                }
                .map_err(|refused| refused.reason)?,
            );
        }
        let draft = self
            .draft
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        draft.prepare_on_control(
            custody,
            self.token
                .as_ref()
                .ok_or(AudioStorageError::SourceMismatch)?,
        )?;
        draft.finish_on_control(&mut self.custody, &mut self.token, &mut self.owner)
    }
    /// Scalar test observation of the actual rooted collection backing; never
    /// an owner, source accessor or publication capability.
    #[cfg(any(test, feature = "instrument-test-support"))]
    pub fn test_source_collection_identity(&self) -> (usize, usize) {
        (self.sources.as_ptr() as usize, self.sources.capacity())
    }
    /// Correlation only, never an owner/source or native admission capability.
    pub fn current_sampler_context(&self) -> Option<crate::AudioSourceContext> {
        let owner = self.owner.as_ref()?;
        owner.inner.runtime.as_ref()?.sampler_sources.as_ref()?;
        owner.source_context()
    }
    /// Actual resident PCM backing in this completed native constructor. This
    /// scalar observes capacity and grants no ownership or retirement authority.
    pub fn current_sampler_pcm_capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.owner
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .inner
            .custody
            .native_sampler_pcm_capacity_frames()
    }
    pub fn resident_pcm_capacity_frames(&self) -> Result<u64, AudioStorageError> {
        self.owner
            .as_ref()
            .ok_or(AudioStorageError::SourceMismatch)?
            .inner
            .custody
            .resident_pcm_capacity_frames()
    }
    /// Validate an intact, never-published resident CURRENT constructor before
    /// a destination native Root takes the opaque cell. Refusal retains it.
    pub fn preflight_current_sampler_adoption_on_control(
        &mut self,
        context: crate::AudioSourceContext,
        sample_rate: u32,
        source_capacity: usize,
        ledger: crate::AudioStorageLedgerReservation,
        external_runtime_bytes: usize,
    ) -> Result<super::AudioCandidateConstructorSummary, AudioStorageError> {
        if self.failed
            || self.cleaning
            || self.complete
            || self.owner_runtime_done
            || self.first_panic.is_some()
            || self.failed_incomplete()
            || self.bank.is_some()
            || self.custody.is_some()
            || self.token.is_some()
            || self.sources.capacity() != 0
            || self.resident_draft.capacity() != 0
            || self.draft.as_ref().is_none_or(|d| !d.transferred_empty())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        let owner = self
            .owner
            .as_mut()
            .ok_or(AudioStorageError::SourceMismatch)?;
        if owner
            .inner
            .custody
            .native_sampler_source_capacity()
            .is_none_or(|n| n > source_capacity)
            || owner.source_context() != Some(context)
            || owner.sample_rate() != sample_rate
            || owner.phase() != AudioCandidateDisposalPhase::Owned
            || owner.streamed_sources().next().is_some()
            || owner
                .inner
                .runtime
                .as_ref()
                .is_none_or(|r| r.sampler_sources.is_none())
        {
            return Err(AudioStorageError::SourceMismatch);
        }
        owner.seal_constructor_with_runtime_storage_on_control(ledger, external_runtime_bytes)
    }
    pub(super) fn take_preflighted_owner(&mut self) -> Option<AudioCandidateOwner> {
        self.owner.take()
    }
    pub fn owner_on_control(&mut self) -> Option<&mut AudioCandidateOwner> {
        self.owner.as_mut()
    }
    pub fn failed_incomplete(&self) -> bool {
        self.failed
            || self
                .draft
                .as_ref()
                .is_some_and(AudioCandidateDraft::failed_incomplete)
            || self
                .custody
                .as_ref()
                .is_some_and(|c| c.phase() == AudioPreparationPhase::FailedIncomplete)
            || self
                .owner
                .as_ref()
                .is_some_and(|o| o.phase() == AudioCandidateDisposalPhase::FailedIncomplete)
    }
    /// Explicit cleanup of unpublished work; no NeverInstalled/Returned receipt
    /// exists because this owner has not crossed a publication queue boundary.
    /// Includes the emptied draft/custody wrappers and original source backing.
    pub fn cleanup_on_control(&mut self) -> Result<(), AudioStorageError> {
        if self.complete {
            return Ok(());
        }
        if self.failed_incomplete() || self.cleaning {
            return Err(AudioStorageError::UnknownStorage);
        }
        self.cleaning = true;
        match catch_unwind(AssertUnwindSafe(|| {
            if let Some(owner) = self.owner.as_mut() {
                if !self.owner_runtime_done {
                    owner.dispose_on_control()?;
                    self.owner_runtime_done = true;
                }
                owner.dispose_original_inputs_on_control()?;
            }
            drop(self.owner.take());
            if let Some(draft) = self.draft.as_mut() {
                draft.dispose_on_control()?;
            }
            drop(self.draft.take());
            if let Some(custody) = self.custody.as_mut() {
                custody.close_on_control();
                custody.cleanup_on_control();
                if custody.phase() == AudioPreparationPhase::FailedIncomplete {
                    return Err(AudioStorageError::UnknownStorage);
                }
                if let Some(token) = self.token.as_ref() {
                    if self.bank.is_some() {
                        return Err(AudioStorageError::SourceMismatch);
                    }
                    self.bank = Some(
                        custody
                            .recover_on_control(token)
                            .ok_or(AudioStorageError::SourceMismatch)?,
                    );
                } else if custody.phase() != AudioPreparationPhase::Vacant {
                    return Err(AudioStorageError::SourceMismatch);
                }
            }
            drop(self.custody.take());
            self.token = None;
            drop(self.bank.take());
            drop(std::mem::take(&mut self.sources));
            drop(std::mem::take(&mut self.resident_draft));
            Ok(())
        })) {
            Ok(Ok(())) => {
                self.cleaning = false;
                self.complete = true;
                Ok(())
            }
            Ok(Err(reason)) => {
                self.failed = true;
                Err(reason)
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.failed = true;
                Err(AudioStorageError::UnknownStorage)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    fn reservation() -> crate::AudioStorageReservation {
        crate::AudioStorageReservation {
            allocation_items: 256,
            handle_obligations: 4096,
            max_prepared_data_bytes: 64 * 1024 * 1024,
            max_construction_data_bytes: 64 * 1024 * 1024,
            caller_retained_bytes: 0,
            caller_construction_bytes: 0,
        }
    }
    fn collected() -> Option<(Project, AudioCandidatePreparation)> {
        if !crate::audio_storage_constructor_layout_supported() {
            return None;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap().id();
        let audio = crate::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap();
        let original = audio.frames().as_ptr();
        let mut cell = AudioCandidatePreparation::try_new_on_control(1, reservation()).unwrap();
        cell.copy_resident_source_on_control(asset, &audio).unwrap();
        let crate::audio_only_inputs::SourceKind::Resident(copy) = &cell.sources[0].1.kind else {
            panic!("resident");
        };
        assert_ne!(copy.frames().as_ptr(), original);
        assert_eq!(copy.frames(), audio.frames());
        Some((project, cell))
    }
    #[test]
    fn repitch_collection_matches_raw_and_preserves_owners_through_bank_transfer() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap();
        let raw = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, asset.id()),
            asset.source_sample_rate(),
            project.sample_rate(),
            asset.source_frames(),
            asset.playback_frames(),
            asset.conversion_version(),
            [7; 32],
        )
        .unwrap();
        let frames = raw.playback_frames();
        let map = hex_warp::TimeMap::new(
            vec![hex_warp::MapSegment {
                output_start: 0,
                output_end: frames * 2,
                source_start_q32: 0,
                source_end_q32: i128::from(frames) << 32,
            }],
            (frames * 2) as usize,
        )
        .unwrap();
        let repitch = raw.with_repitch(&map, hex_warp::Resampler::V2).unwrap();
        let other = crate::StreamedSourceMetadata::new(
            raw.key(),
            raw.source_rate(),
            raw.project_rate(),
            raw.source_frames(),
            raw.playback_frames(),
            raw.conversion_version(),
            [8; 32],
        )
        .unwrap()
        .with_repitch(&map, hex_warp::Resampler::V2)
        .unwrap();
        let mut cell = AudioCandidatePreparation::try_new_on_control(3, reservation()).unwrap();
        assert_eq!(
            cell.add_repitch_streamed_source_on_control(repitch.clone()),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            cell.add_streamed_source_on_control(repitch.clone()),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            cell.add_pitched_streamed_source_on_control(repitch.clone()),
            Err(AudioStorageError::SourceMismatch)
        );
        cell.add_streamed_source_on_control(raw.clone()).unwrap();
        let handle = cell.sources[0].1.handle;
        assert_eq!(
            cell.add_repitch_streamed_source_on_control(other),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(cell.sources[0].1.handle, handle);
        cell.add_repitch_streamed_source_on_control(repitch.clone())
            .unwrap();
        let handles: Vec<_> = cell.sources.iter().map(|(_, owner)| owner.handle).collect();
        assert_eq!(
            cell.add_repitch_streamed_source_on_control(repitch.clone()),
            Err(AudioStorageError::DuplicateAsset)
        );
        assert_eq!(
            cell.sources
                .iter()
                .map(|(_, owner)| owner.handle)
                .collect::<Vec<_>>(),
            handles
        );
        let context = crate::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        };
        assert_eq!(
            cell.prepare_on_control(&project, 0, context),
            Err(AudioStorageError::SourceMismatch)
        );
        let bank = cell.bank.as_ref().unwrap();
        assert!(bank.pitched_source_token(repitch.key()).is_err());
        let token = bank.repitch_source_token(repitch.key()).unwrap();
        assert_eq!(
            bank.source_view(&token).unwrap().streamed_metadata(),
            Some(&repitch)
        );
        assert_eq!(
            bank.source_view(&bank.source_token(asset.id()).unwrap())
                .unwrap()
                .streamed_metadata(),
            Some(&raw)
        );
        cell.prepare_on_control(&project, 11, context).unwrap();
        cell.cleanup_on_control().unwrap();
    }

    #[test]
    fn pitched_stream_collection_refuses_invalid_sources_and_transfers_bank() {
        if !crate::audio_storage_constructor_layout_supported() {
            return;
        }
        let project = crate::audio_preparation::candidate_test_project();
        let asset = project.assets().next().unwrap();
        let raw = crate::StreamedSourceMetadata::new(
            crate::StreamSourceKey::new(11, asset.id()),
            asset.source_sample_rate(),
            project.sample_rate(),
            asset.source_frames(),
            asset.playback_frames(),
            asset.conversion_version(),
            [7; 32],
        )
        .unwrap();
        let up = raw
            .with_plain_pitch(700, hex_project::WarpResampler::V2)
            .unwrap();
        let down = raw
            .with_plain_pitch(-300, hex_project::WarpResampler::V2)
            .unwrap();
        let mut cell = AudioCandidatePreparation::try_new_on_control(3, reservation()).unwrap();
        assert_eq!(
            cell.add_pitched_streamed_source_on_control(up.clone()),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            cell.add_streamed_source_on_control(up.clone()),
            Err(AudioStorageError::SourceMismatch)
        );
        cell.add_streamed_source_on_control(raw.clone()).unwrap();
        let foreign = crate::StreamedSourceMetadata::new(
            raw.key(),
            raw.source_rate(),
            raw.project_rate(),
            raw.source_frames(),
            raw.playback_frames(),
            raw.conversion_version(),
            [8; 32],
        )
        .unwrap()
        .with_plain_pitch(700, hex_project::WarpResampler::V2)
        .unwrap();
        assert_eq!(
            cell.add_pitched_streamed_source_on_control(foreign),
            Err(AudioStorageError::SourceMismatch)
        );
        cell.add_pitched_streamed_source_on_control(up.clone())
            .unwrap();
        assert_eq!(
            cell.add_pitched_streamed_source_on_control(up.clone()),
            Err(AudioStorageError::DuplicateAsset)
        );
        cell.add_pitched_streamed_source_on_control(down.clone())
            .unwrap();
        let handles: Vec<_> = cell.sources.iter().map(|(_, s)| s.handle).collect();
        assert_eq!(
            cell.add_pitched_streamed_source_on_control(up.clone()),
            Err(AudioStorageError::Capacity)
        );
        assert_eq!(
            cell.sources
                .iter()
                .map(|(_, s)| s.handle)
                .collect::<Vec<_>>(),
            handles
        );
        let context = crate::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        };
        assert_eq!(
            cell.prepare_on_control(&project, 0, context),
            Err(AudioStorageError::SourceMismatch)
        );
        let bank = cell.bank.as_ref().unwrap();
        for metadata in [&up, &down] {
            let token = bank.pitched_source_token(metadata.key()).unwrap();
            assert_eq!(
                bank.source_view(&token).unwrap().streamed_metadata(),
                Some(metadata)
            );
        }
        cell.prepare_on_control(&project, 11, context).unwrap();
        cell.cleanup_on_control().unwrap();
    }
    #[test]
    fn armed_native_state_box_is_quoted_and_observed_once_in_closed_custody() {
        use hex_lattice_contract::{DeviceAutomationTarget, NativeDeviceMetadata};
        use hex_project::{Command, Created, Editor};
        let mut editor =
            Editor::new(crate::audio_preparation::candidate_test_project(), 8).unwrap();
        let track = editor.project().tracks()[0].id();
        let Created::Device(device) = editor
            .execute(Command::InsertDevice {
                track,
                before: None,
                state: hex_devices::utility::UTILITY_DEFINITION
                    .default_state()
                    .unwrap(),
            })
            .unwrap()
        else {
            panic!("native device fixture")
        };
        let project = editor.project();
        let asset = project.assets().next().unwrap().id();
        let input = crate::native_parameter_input_target(
            project,
            device,
            DeviceAutomationTarget::HostInputGain,
        )
        .unwrap();
        let registry = hex_devices::native_registry().unwrap();
        let effects = AudioCandidateEffects::new(&registry);
        let context = crate::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        };
        let prepare = |armed| {
            let quote = if armed {
                effects
                    .quote_with_native_parameter_input(project, 1, input)
                    .unwrap()
            } else {
                effects.quote(project, 1).unwrap()
            };
            let limits = quote.storage_reservation(65536).unwrap();
            let mut cell = AudioCandidatePreparation::try_new_on_control(1, limits).unwrap();
            cell.copy_resident_source_on_control(
                asset,
                &crate::Audio::new(48000, vec![[0.25; 2]; 8]).unwrap(),
            )
            .unwrap();
            if armed {
                effects
                    .prepare_with_native_parameter_input(
                        &mut cell, project, 11, context, None, input,
                    )
                    .unwrap();
            } else {
                effects
                    .prepare(&mut cell, project, 11, context, None)
                    .unwrap();
            }
            let summary = cell
                .owner_on_control()
                .unwrap()
                .seal_constructor_on_control(crate::AudioStorageLedgerReservation {
                    data_rows: quote.data_rows,
                    handle_rows: quote.handle_rows,
                    root_rows: 1,
                    max_retained_bytes: limits.max_prepared_data_bytes,
                    max_construction_peak_bytes: limits.max_construction_data_bytes,
                })
                .unwrap();
            assert!(summary.data.data_allocations <= quote.data_rows);
            assert!(summary.data.retained_bytes <= limits.max_prepared_data_bytes);
            cell.cleanup_on_control().unwrap();
            (quote, summary)
        };
        // Host arming adds no provider/curve vector: its sole new DATA backing
        // is the control-created runtime state and the corresponding ledger row.
        let (plain_quote, plain) = prepare(false);
        let (armed_quote, armed) = prepare(true);
        let bytes = crate::SessionPlayback::native_parameter_input_state_bytes();
        assert_eq!(armed_quote.data_rows, plain_quote.data_rows + 1);
        assert_eq!(
            armed_quote.metadata_data_bytes,
            plain_quote.metadata_data_bytes + bytes
        );
        assert_eq!(armed.data.data_allocations, plain.data.data_allocations + 1);
        assert_eq!(armed.data.data_bytes, plain.data.data_bytes + bytes);
    }
    #[test]
    fn persistent_collection_refusal_retry_and_original_owner_cleanup_are_control_side() {
        let Some((project, mut cell)) = collected() else {
            return;
        };
        let pointer = cell.sources.as_ptr();
        let capacity = cell.sources.capacity();
        let context = crate::AudioSourceContext {
            project_generation: 2,
            editor_revision: 8,
        };
        assert_eq!(
            cell.prepare_on_control(&project, 0, context),
            Err(AudioStorageError::SourceMismatch)
        );
        assert_eq!(
            (
                cell.bank.as_ref().unwrap().sources.as_ptr(),
                cell.bank.as_ref().unwrap().sources.capacity()
            ),
            (pointer, capacity)
        );
        cell.prepare_on_control(&project, 11, context).unwrap();
        assert!(cell.bank.is_none() && cell.custody.is_none() && cell.token.is_none());
        let observed = Arc::new(std::sync::Mutex::new(None));
        let result = observed.clone();
        let owner = cell.owner.as_mut().unwrap();
        let runtime = owner.inner.runtime.as_deref().unwrap() as *const RuntimeOwner as usize;
        owner.inner.runtime.as_mut().unwrap().probe =
            Some(RuntimeDropProbe(Some(Box::new(move || {
                *result.lock().unwrap() = Some((runtime, std::thread::current().id()));
            }))));
        let caller = std::thread::current().id();
        let cell = std::thread::spawn(move || {
            cell.cleanup_on_control().unwrap();
            cell.cleanup_on_control().unwrap();
            cell
        })
        .join()
        .unwrap();
        let (_, cleanup_thread) = observed.lock().unwrap().unwrap();
        assert_ne!(cleanup_thread, caller);
        assert_eq!(*observed.lock().unwrap(), Some((runtime, cleanup_thread)));
        assert!(
            cell.complete && cell.owner.is_none() && cell.custody.is_none() && cell.draft.is_none()
        );
        assert_eq!(cell.sources.capacity(), 0);
        assert!(!cell.failed_incomplete());
    }
    #[test]
    fn collection_and_completed_owner_cancel_clean_but_failed_runtime_is_retained_once() {
        let Some((_, mut partial)) = collected() else {
            return;
        };
        partial.cleanup_on_control().unwrap();
        assert!(partial.complete && partial.custody.is_none() && partial.draft.is_none());
        let Some((project, mut cell)) = collected() else {
            return;
        };
        cell.prepare_on_control(
            &project,
            11,
            crate::AudioSourceContext {
                project_generation: 0,
                editor_revision: 0,
            },
        )
        .unwrap();
        struct Payload(Arc<AtomicUsize>);
        impl Drop for Payload {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let calls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let payload = drops.clone();
        cell.owner
            .as_mut()
            .unwrap()
            .inner
            .runtime
            .as_mut()
            .unwrap()
            .probe = Some(RuntimeDropProbe(Some(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            std::panic::panic_any(Payload(payload));
        }))));
        assert!(cell.cleanup_on_control().is_err());
        assert!(cell.owner.is_some() && cell.draft.is_some() && cell.failed_incomplete());
        assert_eq!(
            (calls.load(Ordering::SeqCst), drops.load(Ordering::SeqCst)),
            (1, 0)
        );
        assert!(cell.cleanup_on_control().is_err());
        assert_eq!(
            (calls.load(Ordering::SeqCst), drops.load(Ordering::SeqCst)),
            (1, 0)
        );
        drop(cell); // Explicit fixture teardown; failed production cells stay rooted.
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
}
#[test]
fn original_leaf_copy_keeps_original_rate_and_rebinds_catalog_to_actual_new_bank() {
    use crate::native_device_resources::{
        DeviceResourceCatalog, OriginalResourceSource, ResourceCatalogReservation,
    };
    use hex_lattice_contract::{DeviceAssetKey, DeviceAssetReference, DeviceAssetSlot};
    let mut editor =
        hex_project::Editor::new(hex_project::Project::new(16000).unwrap(), 8).unwrap();
    let hex_project::Created::Asset(asset) = editor
        .execute(hex_project::Command::ImportDeviceAsset {
            path: "original.wav".into(),
            sample_rate: 8000,
            frames: 4,
        })
        .unwrap()
    else {
        panic!("asset")
    };
    let input_limits = crate::AudioStorageReservation {
        allocation_items: 16,
        handle_obligations: 8,
        max_prepared_data_bytes: 1_000_000,
        max_construction_data_bytes: 2_000_000,
        caller_retained_bytes: 0,
        caller_construction_bytes: 0,
    };
    let pcm = vec![[0.125, -0.25], [0.5, 0.0], [-0.125, 0.25], [0.0, 0.0]];
    let original = OwnedAudioInputs::try_new(
        vec![(
            asset,
            crate::OwnedAudioSource::try_resident(8000, pcm)
                .ok()
                .unwrap(),
        )],
        input_limits,
    )
    .ok()
    .unwrap();
    let original_token = original.source_token(asset).unwrap();
    let leaf = original.source_view(&original_token).unwrap();
    let original_pointer = leaf.resident_frames().unwrap().as_ptr();
    let mut cell = AudioCandidatePreparation::try_new_on_control(1, input_limits).unwrap();
    cell.copy_original_source_on_control(asset, leaf).unwrap();
    OwnedAudioInputs::try_new_in_place(&mut cell.sources, input_limits, &mut cell.bank).unwrap();
    let copied = cell.bank.as_ref().unwrap();
    let view = copied
        .source_view(&copied.source_token(asset).unwrap())
        .unwrap();
    assert_eq!(view.sample_rate(), 8000);
    assert_eq!(view.resident_frames().unwrap().len(), 4);
    assert_ne!(view.resident_frames().unwrap().as_ptr(), original_pointer);
    assert_eq!(
        view.resident_frames().unwrap(),
        original
            .source_view(&original_token)
            .unwrap()
            .resident_frames()
            .unwrap()
    );
    let catalog = DeviceResourceCatalog::prepare_inputs_on_control(
        copied,
        &[OriginalResourceSource {
            asset,
            sample_rate: 8000,
            frames: 4,
        }],
        ResourceCatalogReservation {
            metadata_bytes: 65536,
            hash_frames: 4,
        },
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let state = hex_devices::sampler::state::SamplerState::for_source_frames(4)
        .unwrap()
        .to_native()
        .unwrap()
        .with_asset_references(vec![DeviceAssetReference {
            slot: DeviceAssetSlot::new(1).unwrap(),
            asset: DeviceAssetKey::new(asset.value()).unwrap(),
        }])
        .unwrap();
    let scope = catalog.prepare_scope_on_control(&state, 65536).unwrap();
    assert!(catalog.resources_from_inputs(&original, &scope).is_err());
    let resources = catalog.resources_from_inputs(copied, &scope).unwrap();
    let observed = hex_lattice::DeviceResources::get(&resources, 0).unwrap();
    assert_eq!(observed.sample_rate, 8000);
    assert_eq!(observed.pcm.frame(0), Some([0.125, -0.25]));
}

#[cfg(test)]
#[path = "current_sampler/tests.rs"]
mod current_sampler_tests;
