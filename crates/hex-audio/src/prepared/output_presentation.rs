//! Publication health and identity for a control-side output timing observation.
use super::{Acquire, PublicationControl};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PluginOutputFence {
    root: u64,
    fault_epoch: u64,
    pub(crate) quantum: u16,
    pub(crate) project_rate: u32,
}

impl PublicationControl {
    pub(crate) fn plugin_output_fence_on_control(&self) -> Option<PluginOutputFence> {
        let fault_epoch = self.status.last_output_fault_epoch.load(Acquire);
        let quantum = self.plugin_callback_quantum_on_control()?;
        if self.status.closing.load(Acquire)
            || self.status.device_fault.load(Acquire)
            || self.status.render_fault.load(Acquire)
            || self.status.last_output_fault_epoch.load(Acquire) != fault_epoch
        {
            return None;
        }
        Some(PluginOutputFence {
            root: self.output_identity()?,
            fault_epoch,
            quantum,
            project_rate: self.rate,
        })
    }
}

#[cfg(test)]
mod tests {
    use crate::prepared::{Release, channel};

    #[test]
    fn timing_fence_rejects_closed_and_faulted_output_and_changes_after_recovery() {
        let (mut first, mut callback) = channel(48_000).unwrap();
        assert_eq!(first.plugin_output_fence_on_control(), None);
        first
            .configure_plugin_callback_quantum(&mut callback, 128)
            .unwrap();
        let initial = first.plugin_output_fence_on_control().unwrap();
        assert_eq!(initial.quantum, 128);
        assert_eq!(initial.project_rate, 48_000);
        let (mut replacement, mut callback) = channel(48_000).unwrap();
        replacement
            .configure_plugin_callback_quantum(&mut callback, 128)
            .unwrap();
        assert_ne!(replacement.plugin_output_fence_on_control(), Some(initial));

        first.status.render_fault.store(true, Release);
        assert_eq!(first.plugin_output_fence_on_control(), None);
        first.status.last_output_fault_epoch.store(1, Release);
        first.status.render_fault.store(false, Release);
        let recovered = first.plugin_output_fence_on_control().unwrap();
        assert_ne!(recovered, initial);
        first.status.device_fault.store(true, Release);
        assert_eq!(first.plugin_output_fence_on_control(), None);
        replacement.request_shutdown();
        assert_eq!(replacement.plugin_output_fence_on_control(), None);
    }
}
