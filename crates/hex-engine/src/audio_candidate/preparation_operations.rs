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
    /// Actual ready identity only; never cost, custody or publication authority.
    pub fn prepared_identity(&self) -> Option<AudioCandidateIdentity> {
        self.preparation
            .owner_ref_on_control()
            .map(AudioCandidateOwner::identity)
    }
}
