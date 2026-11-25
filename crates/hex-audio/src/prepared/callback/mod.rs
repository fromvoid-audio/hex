//! Real-time callback execution, rendering, command consumption, and channel constructors.
mod channel_init;
mod commands;
mod custody;
mod render_loop;
mod telemetry_recording;

#[cfg(test)]
pub(crate) use channel_init::channel_with_interface;
pub use channel_init::{Callback, channel, channel_with_spectrum, channel_with_worker_pool};
pub(crate) use channel_init::{
    channel_with_interface_and_buffer, native_endpoint_bank_storage_quote,
};

#[cfg(feature = "worker-test-support")]
use super::*;

#[cfg(feature = "worker-test-support")]
impl Callback {
    /// Read-only headless control observation while the callback is quiescent.
    /// Empty is queue state, not graph/source application or native readiness.
    pub(crate) fn publication_queue_empty_on_control(&self) -> bool {
        self.publications.is_empty()
    }
    /// Quiescent headless observation of the ordinary handoff, including a
    /// consumed publication still awaiting installation. This grants no ACK.
    pub(crate) fn ordinary_publication_idle_on_control(&self) -> bool {
        self.publications.is_empty()
            && self.deferred.is_none()
            && self.deferred_eligible.is_none()
            && !self.status.publication_pending.load(Acquire)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn callback_channel_enforces_sample_rate_bounds_and_storage_quote() {
        assert!(native_endpoint_bank_storage_quote().is_some());
        assert!(channel(0).is_err());
        assert!(channel(7_999).is_err());
        assert!(channel(192_001).is_err());
        assert!(channel(8_000).is_ok());
        assert!(channel(192_000).is_ok());
    }
}
