//! One nondefault shared fixture for the production Session/Desktop adapter.
//! Output construction uses actual paired endpoints and four terminals. The
//! lifecycle lane takes/drops a headless handle before issuing its existing
//! application-only completion witness; no CPAL/OS stop success is asserted.
use crate::native_callback_owner::{ErrorCallbackOwner, Guard, NativeCallbackOwner};

pub struct HeadlessBackend {
    data: Option<Guard<NativeCallbackOwner>>,
    errors: Option<Guard<ErrorCallbackOwner>>,
}
impl HeadlessBackend {
    pub(crate) fn new(data: Guard<NativeCallbackOwner>, errors: Guard<ErrorCallbackOwner>) -> Self {
        Self {
            data: Some(data),
            errors: Some(errors),
        }
    }
    /// Actual callback entry with bounded caller-owned output; no owner escape.
    pub fn render(&mut self, output: &mut [hex_engine::Frame]) -> Result<(), hex_engine::Invalid> {
        let owner = self
            .data
            .as_mut()
            .and_then(Guard::owner_mut)
            .ok_or(hex_engine::Invalid("headless backend is closed"))?;
        let mut result = Ok(());
        owner.run(|callback, _| {
            callback.begin_block();
            result = callback.render(output);
        });
        result
    }
    pub fn status(&self) -> Option<&crate::prepared::Status> {
        self.data
            .as_ref()
            .and_then(Guard::owner_ref)
            .map(NativeCallbackOwner::borrowed_status)
    }
    /// Publish/release both real carriers. Their contents remain lifecycle-owned.
    pub fn close(&mut self) {
        drop(self.data.take());
        drop(self.errors.take());
    }
}
impl Drop for HeadlessBackend {
    fn drop(&mut self) {
        self.close();
    }
}

pub fn open(rate: u32) -> Result<(crate::WorkerPreparedOutput, HeadlessBackend), String> {
    crate::device::headless_worker_output(rate)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_worker_endpoints_keep_lease_until_client_and_both_callback_carriers_close() {
        let _serial = crate::prepared::candidate_tests::serial();
        let (mut output, mut backend) = open(48000).unwrap();
        let client = output.take_client().unwrap();
        let mut silence = [[1.0; 2]; 8];
        backend.render(&mut silence).unwrap();
        assert_eq!(silence, [[0.0; 2]; 8]);
        let identity = client.output_identity();
        assert!(identity.is_some_and(|id| id > 0));
        drop(output); // transfers actual handle/terminal owner to lifecycle
        backend.close();
        assert!(crate::lifecycle::Lease::acquire().is_err());
        drop(client); // no receive/poll needed for its whole endpoint transfer
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            if let Ok(lease) = crate::lifecycle::Lease::acquire() {
                drop(lease);
                break;
            }
            assert!(std::time::Instant::now() < deadline);
            std::thread::yield_now();
        }
    }
}
