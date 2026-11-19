//! Nonblocking serialization of one paired source's host-input generation.
//! This does not identify a graph, prove SDK processing, or admit publication.
use super::{Callback, Control, Fault, Revocation};
use std::{
    num::NonZeroU64,
    sync::{
        Arc, Weak,
        atomic::Ordering::{AcqRel, Acquire, Release},
    },
};

// The upper bit is a permanent revocation marker and the lower bit is a
// control/callback reservation. Revocation and a commit decision use the same
// atomic, so their order is explicit even when the two lanes race.
pub(super) const REVOKED_BIT: u64 = 1 << 63;
const RESERVED_BIT: u64 = 1;
fn generation(ordinal: NonZeroU64) -> Option<u64> {
    ordinal
        .get()
        .checked_mul(2)
        .filter(|value| value & REVOKED_BIT == 0)
}

/// Control-side observation of one generation on the actual source pair.
/// This retains only a weak identity, not the worker or permission to publish.
/// It must be destroyed on control, never in an audio callback.
pub struct ParameterInputSource {
    signal: Weak<Revocation>,
    ordinal: NonZeroU64,
}

/// A private control-side admission. Dropping without publishing restores the
/// previous generation; retain the original source while this borrow exists.
pub struct ParameterInputAdmission<'a> {
    signal: &'a Revocation,
    previous: u64,
    next: u64,
    published: bool,
}

impl ParameterInputAdmission<'_> {
    /// Publish only after the caller has installed the complete private input.
    pub fn publish(mut self) {
        // Revocation can win after the input was prepared. Keep its marker
        // sticky; the ordinary revoked-input terminal still owns the slot.
        if self
            .signal
            .parameter_input_generation
            .compare_exchange(self.previous | RESERVED_BIT, self.next, AcqRel, Acquire)
            .is_err()
        {
            self.signal
                .parameter_input_generation
                .fetch_and(!RESERVED_BIT, Release);
        }
        self.published = true;
    }
}
impl Drop for ParameterInputAdmission<'_> {
    fn drop(&mut self) {
        if !self.published {
            self.signal
                .parameter_input_generation
                .fetch_and(!RESERVED_BIT, Release);
        }
    }
}

/// A bounded callback-side borrow of the exact paired input generation. The
/// host must still validate its graph, source, transport and native custody.
/// It must release this borrow before returning the endpoint to control.
pub struct ParameterInputFence<'a> {
    signal: &'a Revocation,
    generation: u64,
    commit_decided: bool,
}
impl ParameterInputFence<'_> {
    /// Observe current revocation. This is only a snapshot: revocation may
    /// race any later check or graph commit, and needs separate serialization.
    pub fn is_live(&self) -> bool {
        !self.signal.is_revoked()
    }
    /// Establish one ordering point against source revocation without waiting.
    /// Success orders this decision before later revocation; it does not prove
    /// any graph, candidate, SDK or transport fact and is not publication
    /// authority. The host must complete or refuse its transaction separately.
    pub fn try_order_before_revocation(&mut self) -> bool {
        if self.commit_decided {
            return false;
        }
        let reserved = self.generation | RESERVED_BIT;
        if self
            .signal
            .parameter_input_generation
            .compare_exchange(reserved, reserved, AcqRel, Acquire)
            .is_err()
        {
            return false;
        }
        self.commit_decided = true;
        true
    }
}
impl Drop for ParameterInputFence<'_> {
    fn drop(&mut self) {
        self.signal
            .parameter_input_generation
            .fetch_and(!RESERVED_BIT, Release);
    }
}

impl Control {
    /// Observe a currently admitted generation on this exact pair. The result
    /// may already be stale when returned; the callback must reserve it again.
    /// This does not prove that the worker processed the input.
    pub fn observe_parameter_input_on_control(
        &self,
        ordinal: NonZeroU64,
    ) -> Option<ParameterInputSource> {
        let generation = generation(ordinal)?;
        if self.is_revoked() || self.revoked.parameter_input_generation.load(Acquire) != generation
        {
            return None;
        }
        Some(ParameterInputSource {
            signal: Arc::downgrade(&self.revoked),
            ordinal,
        })
    }

    /// Reserve the next monotonically increasing input without waiting. A busy
    /// callback returns Saturated before the caller admits or publishes input.
    /// Ordinals above `2^62 - 1` return Exhausted; they cannot share the
    /// reserved and revocation bits in the encoded generation.
    pub fn reserve_parameter_input_on_control(
        &self,
        ordinal: NonZeroU64,
    ) -> Result<ParameterInputAdmission<'_>, Fault> {
        let next = generation(ordinal).ok_or(Fault::Exhausted)?;
        if self.is_revoked() {
            return Err(Fault::Revoked);
        }
        let previous = self.revoked.parameter_input_generation.load(Acquire);
        if previous & REVOKED_BIT != 0 {
            return Err(Fault::Revoked);
        }
        if previous & RESERVED_BIT != 0 {
            return Err(Fault::Saturated);
        }
        if next <= previous {
            return Err(Fault::Protocol);
        }
        self.revoked
            .parameter_input_generation
            .compare_exchange(previous, previous | RESERVED_BIT, AcqRel, Acquire)
            .map_err(|_| Fault::Saturated)?;
        let admission = ParameterInputAdmission {
            signal: &self.revoked,
            previous,
            next,
            published: false,
        };
        if self.is_revoked() {
            return Err(Fault::Revoked);
        }
        Ok(admission)
    }
}

impl Callback {
    /// Reserve a previously observed generation only on its actual paired
    /// endpoint. Scalar bindings and equal ordinals on another pair do not match.
    pub fn reserve_observed_parameter_input_on_callback(
        &self,
        source: &ParameterInputSource,
    ) -> Option<ParameterInputFence<'_>> {
        if !std::ptr::eq(source.signal.as_ptr(), Arc::as_ptr(&self.revoked)) {
            return None;
        }
        self.reserve_parameter_input_on_callback(source.ordinal)
    }

    /// Reserve only the named generation on this actual paired endpoint. None
    /// covers stale generation, contention, exhaustion and revocation. This
    /// operation never waits, allocates, or transfers source custody.
    pub fn reserve_parameter_input_on_callback(
        &self,
        ordinal: NonZeroU64,
    ) -> Option<ParameterInputFence<'_>> {
        let generation = generation(ordinal)?;
        if self.is_revoked() {
            return None;
        }
        self.revoked
            .parameter_input_generation
            .compare_exchange(generation, generation | RESERVED_BIT, AcqRel, Acquire)
            .ok()?;
        let fence = ParameterInputFence {
            signal: &self.revoked,
            generation,
            commit_decided: false,
        };
        if !fence.is_live() {
            return None;
        }
        Some(fence)
    }
}
