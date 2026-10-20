//! Four concrete original carrier constructors. These registrations cover the
//! actual Box/cell/lease fields, never their unseen payload children. No legacy
//! Arc is adopted; the Root's before-birth factory reservation stays mandatory.
use super::*;
use crate::{
    candidate_custody::CandidateRoot,
    prepared::{ClientEndpoints, PublicationControl},
};
use hex_engine::{Invalid, sampler_host_fields::NativeCarrierStorageRegistration as Record};
use std::mem::size_of;

pub(crate) trait CarrierPayload {
    const INDEX: usize;
    // Actual channel/Status fields only, not a complete payload certificate.
    fn matches_constructor_channel_fields_on_control(&self, root: &CandidateRoot) -> bool;
}
impl CarrierPayload for NativeCallbackOwner {
    const INDEX: usize = 0;
    fn matches_constructor_channel_fields_on_control(&self, root: &CandidateRoot) -> bool {
        root.matches_status_field(&self.status, crate::prepared::status_owner::NATIVE_DATA)
            && self
                .callback
                .matches_fresh_registered_payload_on_control(root)
    }
}
impl CarrierPayload for ErrorCallbackOwner {
    const INDEX: usize = 1;
    fn matches_constructor_channel_fields_on_control(&self, root: &CandidateRoot) -> bool {
        root.matches_status_field(&self.status, crate::prepared::status_owner::NATIVE_ERROR)
    }
}
impl CarrierPayload for ClientEndpoints {
    const INDEX: usize = 2;
    fn matches_constructor_channel_fields_on_control(&self, root: &CandidateRoot) -> bool {
        self.matches_fresh_registered_payload_on_control(root)
    }
}
impl CarrierPayload for PublicationControl {
    const INDEX: usize = 3;
    fn matches_constructor_channel_fields_on_control(&self, root: &CandidateRoot) -> bool {
        self.matches_fresh_registered_payload_on_control(root)
    }
}
fn cost<T>() -> Option<(usize, usize)> {
    let data = Record::requested_bytes_on_control::<T>().ok()?;
    let peak = data
        .checked_add(Record::construction_scratch_bytes_on_control::<T>().ok()?)?
        .checked_add(size_of::<Guard<T>>())?
        .checked_add(size_of::<Terminal<T>>())?;
    Some((data, peak))
}
pub(crate) fn carrier_constructor_bound() -> Option<(usize, usize)> {
    [
        cost::<NativeCallbackOwner>()?,
        cost::<ErrorCallbackOwner>()?,
        cost::<ClientEndpoints>()?,
        cost::<PublicationControl>()?,
    ]
    .into_iter()
    .try_fold((0usize, 0usize), |(data, peak), (d, p)| {
        Some((data.checked_add(d)?, peak.checked_add(p)?))
    })
    .and_then(|(data, peak)| Some((data, peak.checked_add(CandidateRoot::status_guard_bytes())?)))
}
pub(super) fn validate_birth_limits(root: &CandidateRoot) -> Result<(), Invalid> {
    let Some(limits) = root.native_carrier_birth_limits() else {
        return Ok(());
    };
    let (retained, peak) = crate::prepared::factory::native_birth_bound()
        .ok_or(Invalid("native carrier birth layout unknown"))?;
    if retained > limits.max_retained_data_bytes || peak > limits.max_construction_peak_data_bytes {
        return Err(Invalid("native carrier birth reservation exceeded"));
    }
    Ok(())
}
/// Only the four concrete owning producers implement CarrierPayload. Each is
/// consumed once by its existing constructor. The Root installs the fresh
/// record before either endpoint escapes; registration is not native admission.
pub(crate) fn reserve_for_root<T: CarrierPayload>(
    root: &CandidateRoot,
    value: T,
) -> Result<(Guard<T>, Terminal<T>), Invalid> {
    if root.native_carrier_birth_limits().is_none() {
        return Ok(reserve(value));
    }
    validate_birth_limits(root)?;
    // Each check borrows the actual existing Root record under its control-only
    // guard and releases it before ANY carrier allocation or payload drop.
    if !value.matches_constructor_channel_fields_on_control(root) {
        return Err(Invalid(
            "native prepared queue field belongs to another constructor",
        ));
    }
    if !root.native_carrier_vacant(T::INDEX) {
        return Err(Invalid("native carrier destination occupied"));
    }
    let (mut guard, mut terminal) = reserve(value);
    let GuardState::Live { owner, cell } = &guard.state else {
        unreachable!()
    };
    let (record, [body, producer, receiver]) = Record::pair_actual_fresh_carrier_on_control(
        owner.as_ref(),
        cell,
        terminal.cell.as_ref().unwrap(),
    )
    .map_err(|_| Invalid("fresh native carrier pairing refused"))?;
    guard.storage = Some(producer);
    terminal.cell_storage = Some(receiver);
    terminal.body_storage = Some(body);
    root.retain_native_carrier(T::INDEX, record)
        .map_err(Invalid)?;
    Ok((guard, terminal))
}
