//! Nonextracting operations on the host's actual unpublished preparation cell.
use super::*;
use hex_project::{AssetId, Project};

/// Exclusive operations, without a mutable loan of the owning preparation value.
/// No operation returns a bank, draft, owner or their mutable references.
/// Construction/retirement/admission still belong to the actual host.
///
/// ```compile_fail
/// use hex_engine::{AudioCandidatePreparation, AudioCandidatePreparationOperations};
/// fn extract<'a>(loan: &'a mut AudioCandidatePreparationOperations<'_>) -> &'a mut AudioCandidatePreparation { loan }
/// ```
/// ```compile_fail
/// use hex_engine::AudioCandidatePreparationOperations;
/// fn duplicate(loan: AudioCandidatePreparationOperations<'_>) { let _ = loan.clone(); }
/// ```
/// ```compile_fail
/// use hex_engine::{AudioCandidatePreparation, AudioCandidatePreparationOperations};
/// fn fabricate(preparation: &mut AudioCandidatePreparation) {
///     let _ = AudioCandidatePreparationOperations { preparation };
/// }
/// ```
/// ```compile_fail
/// use hex_engine::AudioCandidatePreparationOperations;
/// fn extract(loan: &mut AudioCandidatePreparationOperations<'_>) {
///     let _ = loan.owner_on_control();
/// }
/// ```
/// ```compile_fail
/// use hex_engine::{AudioCandidatePreparation, AudioCandidatePreparationOperations};
/// fn replace(loan: &mut AudioCandidatePreparationOperations<'_>, replacement: AudioCandidatePreparation) {
///     let _ = std::mem::replace(&mut **loan, replacement);
/// }
/// ```
pub struct AudioCandidatePreparationOperations<'a> {
    preparation: &'a mut AudioCandidatePreparation,
}
impl AudioCandidatePreparation {
    /// Borrow the actual cell. This creates no origin, cost or admission permit.
    pub fn operations_on_control(&mut self) -> AudioCandidatePreparationOperations<'_> {
        AudioCandidatePreparationOperations { preparation: self }
    }
}
impl AudioCandidatePreparationOperations<'_> {
    /// Invoke the concrete borrowed original producer after validating this
    /// reserved cell's local allowance and vacant phase/destination. Success is
    /// retained immediately inside its actual transferred preparation custody.
    /// The same-worker source/candidate binding, aggregate before-first-owner
    /// reservation and final guarded publication remain separate requirements.
    /// No bank, owner or mutable preparation reference is returned or accepted.
    /// ```compile_fail
    /// use hex_engine::{AudioCandidatePreparationOperations, CancellationQuery};
    /// use hex_engine::native_device_resources::{DeviceResourceBank, OriginalResourceBankPlan, OriginalResourcePlanReservation};
    /// fn extract(loan: &mut AudioCandidatePreparationOperations<'_>, plan: &OriginalResourceBankPlan<'_>, allowance: OriginalResourcePlanReservation, cancel: &dyn CancellationQuery) -> DeviceResourceBank {
    ///     loan.install_original_resources_on_control(plan, allowance, cancel).unwrap()
    /// }
    /// ```
    /// ```compile_fail
    /// use hex_engine::{AudioCandidatePreparationOperations, CancellationQuery};
    /// use hex_engine::native_device_resources::{DeviceResourceBank, OriginalResourcePlanReservation};
    /// fn adopt(loan: &mut AudioCandidatePreparationOperations<'_>, bank: DeviceResourceBank, allowance: OriginalResourcePlanReservation, cancel: &dyn CancellationQuery) {
    ///     loan.install_original_resources_on_control(&bank, allowance, cancel).unwrap();
    /// }
    /// ```
    pub fn install_original_resources_on_control(
        &mut self,
        plan: &crate::native_device_resources::OriginalResourceBankPlan<'_>,
        allowance: crate::native_device_resources::OriginalResourcePlanReservation,
        cancel: &dyn crate::CancellationQuery,
    ) -> Result<(), crate::native_device_resources::ResourcePreparationError> {
        self.preparation
            .install_original_resources_on_control(plan, allowance, cancel)
    }

    /// Existing legacy-resident copy path; no raw Audio owner is adopted.
    /// This operation is not original-resource provenance or peak certification.
    pub fn copy_resident_source_on_control(
        &mut self,
        asset: AssetId,
        audio: &crate::Audio,
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .copy_resident_source_on_control(asset, audio)
    }
    pub fn add_streamed_source_on_control(
        &mut self,
        metadata: crate::StreamedSourceMetadata,
    ) -> Result<(), AudioStorageError> {
        self.preparation.add_streamed_source_on_control(metadata)
    }
    pub fn prepare_on_control(
        &mut self,
        project: &Project,
        source_generation: u64,
        context: crate::AudioSourceContext,
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .prepare_on_control(project, source_generation, context)
    }
    /// Construct independent Main playback in this SAME existing custodian,
    /// using the actual captured target and current/history inputs. Contexts
    /// are fresh; older native products remain separate mandatory live roots.
    /// This constructor-local operation does not bind Service incarnations or
    /// authorize a native publication. Unknown/unbound original rows refuse.
    pub fn prepare_main_original_sampler_on_control(
        &mut self,
        target: &crate::SharedMetadata<hex_project::Project>,
        producer: &crate::native_device_resources::OriginalResourceBankPlan<'_>,
        editor: &hex_project::Editor,
        original: &crate::AudioRootStorageLedger<'_>,
        transforms: &crate::audio_transform::AudioTransformPreparation,
        reservation: crate::AudioStorageLedgerReservation,
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .prepare_original_sampler_for_purpose_on_control(
                target,
                producer,
                editor,
                None,
                original,
                transforms,
                reservation,
                ConstructorPurpose::MainExport,
            )
    }

    /// Reserve/transfer only inside the actual preparation cell. A registered
    /// immutable compiled reader can be installed before beginning this work.
    /// Native-purpose owners cannot enter Main control work.
    pub fn begin_main_control_work_on_control(
        &mut self,
        frames: u64,
    ) -> Result<(), AudioStorageError> {
        self.preparation.begin_main_control_work_on_control(frames)
    }

    /// Productive bounded work while the existing host loan is held. This
    /// returns only nonowning block correlation, never a runtime owner/borrow.
    pub fn step_main_control_work_on_control(
        &mut self,
        maximum: usize,
    ) -> Result<AudioCandidateControlProgress, AudioStorageError> {
        self.preparation
            .step_main_control_work_with_frames_on_control(maximum)
    }

    /// Match the actual bound Main parent BEFORE any DSP or block copy. An
    /// opaque identity is correlation only; it grants no source/budget proof.
    pub fn step_main_control_work_for_identity_on_control(
        &mut self,
        expected: AudioCandidateIdentity,
        maximum: usize,
    ) -> Result<AudioCandidateControlProgress, AudioStorageError> {
        self.preparation
            .step_main_control_work_for_identity_on_control(expected, maximum)
    }

    /// Copy the retained block before releasing the short host loan. File I/O
    /// MUST happen after that loan; the host charges this separate copy buffer.
    pub fn copy_main_control_block_on_control(
        &self,
        block: AudioCandidateControlBlock,
        output: &mut [crate::Frame; 256],
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .copy_main_control_block_on_control(block, output)
    }

    /// A Busy host retains written-but-not-ACKed correlation outside this loan;
    /// retry this exact ACK without writing the completed File block again.
    pub fn acknowledge_main_control_block_on_control(
        &mut self,
        block: AudioCandidateControlBlock,
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .acknowledge_main_control_block_on_control(block)
    }

    /// Complete actual same-parent Main runtime/root/original-input cleanup.
    /// An unacknowledged/unfinished render or native owner refuses. Legitimate
    /// compiled readers return ExternalReadersPending; no cached retired flag
    /// or caller-created receipt can replace this operation.
    pub fn finish_main_control_work_on_control(
        &mut self,
        expected: AudioCandidateIdentity,
    ) -> Result<(), AudioStorageError> {
        self.preparation
            .finish_main_control_work_on_control(expected)
    }

    /// A registered immutable input retained by the existing constructor root.
    pub fn register_compiled_reader_on_control(
        &self,
    ) -> Result<crate::AudioCompiledRevision, AudioStorageError> {
        self.preparation.register_compiled_reader_on_control()
    }
    /// Actual ready identity only; never cost, custody or publication authority.
    pub fn prepared_identity(&self) -> Option<AudioCandidateIdentity> {
        self.preparation
            .owner_ref_on_control()
            .map(AudioCandidateOwner::identity)
    }
}
