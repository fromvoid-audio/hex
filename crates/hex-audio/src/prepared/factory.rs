//! Closed Root + NINE existing queues + shared Status birth. This local bound is not
//! complete channel/family/native admission: the other channel children and
//! retained external parents still need mandatory joint construction inventory.
use super::*;
use crate::{
    candidate_custody::{AudioCandidatePublicationReservation, CandidateRoot},
    native_callback_owner::registered_prepared_ring,
};
use hex_engine::sampler_host_fields::NativeFixedRingRegistration;

pub(super) struct Queues {
    pub(super) status: status_owner::InitialFields,
    pub(super) preview: (
        crate::sample_preview::Control,
        crate::sample_preview::Callback,
    ),
    pub(super) gain: (
        crate::gain_recording::Control,
        crate::gain_recording::Callback,
    ),
    pub(super) spectrum: (
        spectrum_target::SpectrumTargetControl,
        spectrum_target::SpectrumTargetCallback,
    ),
    pub(super) audition: (
        processor_audition::ProcessorAuditionTargetControl,
        processor_audition::ProcessorAuditionTargetCallback,
    ),
    pub(super) publications: Producer<QueuedPublication>,
    pub(super) incoming: Consumer<QueuedPublication>,
    pub(super) returning: Producer<ReturnedPublication>,
    pub(super) retired: Consumer<ReturnedPublication>,
    pub(super) commands: Producer<QueuedCommand>,
    pub(super) events: Consumer<QueuedCommand>,
    // Keep constructor custody alive until all eighteen endpoint fields drop.
    pub(super) root: CandidateRoot,
}
pub(crate) fn ring_bytes<T>(capacity: usize) -> Option<usize> {
    NativeFixedRingRegistration::requested_bytes_on_control::<rtrb::RingBuffer<T>, T>(capacity).ok()
}
pub(crate) fn ring_scratch<T>() -> Option<usize> {
    NativeFixedRingRegistration::construction_scratch_bytes_on_control::<rtrb::RingBuffer<T>>()
        .ok()?
        .checked_add(size_of::<(rtrb::Producer<T>, rtrb::Consumer<T>)>())?
        .checked_add(size_of::<(
            Producer<T>,
            Consumer<T>,
            NativeFixedRingRegistration,
        )>())
}
pub(crate) fn shared_atomic_bytes() -> Option<usize> {
    hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::requested_bytes_on_control()
        .ok()
}
pub(crate) fn atomic_signal_scratch() -> Option<usize> {
    hex_engine::sampler_host_fields::NativeAtomicSignalRegistration::construction_scratch_bytes_on_control().ok()?
        .checked_add(size_of::<[crate::native_callback_owner::atomic_signal::Signal; 2]>())
}

fn root_queue_bound() -> Option<(usize, usize)> {
    let retained = CandidateRoot::requested_birth_bytes(RETIRED)?
        .checked_add(CandidateRoot::resident_requested_bytes()?)?
        .checked_add(CandidateRoot::root_fields_requested_bytes()?)?
        .checked_add(ring_bytes::<QueuedPublication>(1)?)?
        .checked_add(ring_bytes::<ReturnedPublication>(RETIRED)?)?
        .checked_add(ring_bytes::<QueuedCommand>(COMMANDS)?)?;
    // Conservatively include all typed constructor scratch together, plus the
    // actual returned aggregate. No scalar savings from mutually exclusive paths.
    let peak = retained
        .checked_add(CandidateRoot::birth_scratch_bytes()?)?
        .checked_add(CandidateRoot::resident_scratch_bytes()?)?
        .checked_add(CandidateRoot::root_fields_scratch_bytes()?)?
        .checked_add(ring_scratch::<QueuedPublication>()?)?
        .checked_add(ring_scratch::<ReturnedPublication>()?)?
        .checked_add(ring_scratch::<QueuedCommand>()?)?
        .checked_add(size_of::<Queues>())?;
    Some((retained, peak))
}
fn bound() -> Option<(usize, usize)> {
    let (retained, peak) = root_queue_bound()?;
    let status = status_owner::StatusArchive::requested_bytes()?;
    let (spectrum, spectrum_peak) = spectrum_target::constructor_cost()?;
    let (audition, audition_peak) = processor_audition::constructor_cost()?;
    let (preview, preview_peak) = crate::sample_preview::constructor_cost()?;
    let (gain, gain_peak) = crate::gain_recording::constructor_cost()?;
    Some((
        retained
            .checked_add(status)?
            .checked_add(spectrum)?
            .checked_add(audition)?
            .checked_add(preview)?
            .checked_add(gain)?,
        peak.checked_add(status)?
            .checked_add(status_owner::StatusArchive::construction_scratch()?)?
            .checked_add(CandidateRoot::status_guard_bytes())?
            .checked_add(spectrum_peak)?
            .checked_add(audition_peak)?
            .checked_add(preview_peak)?
            .checked_add(gain_peak)?,
    ))
}
pub(crate) fn native_birth_bound() -> Option<(usize, usize)> {
    let (data, peak) = bound()?;
    let (carriers, carrier_peak) = crate::native_callback_owner::carrier_constructor_bound()?;
    Some((data.checked_add(carriers)?, peak.checked_add(carrier_peak)?))
}
pub(super) fn construct(
    reservation: Option<AudioCandidatePublicationReservation>,
    rate: u32,
) -> Result<Queues, Invalid> {
    if let Some(reservation) = reservation {
        reservation.validate_shape().map_err(Invalid)?;
        let (retained, peak) = native_birth_bound().ok_or(Invalid(
            "prepared Root/queue/status constructor layout is unknown",
        ))?;
        if retained > reservation.max_retained_data_bytes
            || peak > reservation.max_construction_peak_data_bytes
        {
            return Err(Invalid(
                "prepared Root/queue/status birth reservation exceeded",
            ));
        }
    }
    let root = if reservation.is_some() {
        CandidateRoot::new_registered(RETIRED)
    } else {
        CandidateRoot::new(RETIRED)
    }
    .map_err(Invalid)?;
    if let Some(limits) = reservation {
        root.plan_native_carrier_birth(limits)?;
        let (publications, incoming, record) = registered_prepared_ring(1)
            .map_err(|_| Invalid("publication queue constructor pairing refused"))?;
        root.retain_prepared_ring(0, record).map_err(Invalid)?;
        let (returning, retired, record) = registered_prepared_ring(RETIRED)
            .map_err(|_| Invalid("return queue constructor pairing refused"))?;
        root.retain_prepared_ring(1, record).map_err(Invalid)?;
        let (commands, events, record) = registered_prepared_ring(COMMANDS)
            .map_err(|_| Invalid("command queue constructor pairing refused"))?;
        root.retain_prepared_ring(2, record).map_err(Invalid)?;
        Ok(Queues {
            status: root.initialize_status()?,
            spectrum: spectrum_target::registered_channel(&root, 3)?,
            audition: processor_audition::registered_channel(&root, 4)?,
            preview: crate::sample_preview::registered_channel(rate, &root)?,
            gain: crate::gain_recording::registered_channel(&root)?,
            root,
            publications,
            incoming,
            returning,
            retired,
            commands,
            events,
        })
    } else {
        // Historical resident/legacy constructor stays unqualified. Its actual
        // endpoints are not adopted by a later reservation/configuration call.
        let (publications, incoming) = native_prepared_ring(1);
        let (returning, retired) = native_prepared_ring(RETIRED);
        let (commands, events) = native_prepared_ring(COMMANDS);
        Ok(Queues {
            status: status_owner::InitialFields::legacy()?,
            spectrum: spectrum_target::channel(),
            audition: processor_audition::channel(),
            preview: crate::sample_preview::channel(rate),
            gain: crate::gain_recording::channel(),
            root,
            publications,
            incoming,
            returning,
            retired,
            commands,
            events,
        })
    }
}
#[cfg(all(test, feature = "allocation-probe"))]
mod tests;
