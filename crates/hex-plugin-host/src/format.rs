//! Format-neutral plugin hosting contracts shared across VST3 and CLAP.
//!
//! These traits factor out the bounded copied/shared-memory ring transport,
//! independent cancellation guardian, and control-plane supervision invariants
//! so future CLAP host adapters plug directly into the existing host
//! infrastructure without duplicating transport rings, cancellation sockets, or
//! supervision bounds.

use crate::transport::{Block, Control, Fault, Worker, WorkerRequest};
use hex_lattice_contract::ForeignProcessorBinding as Binding;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Native external plugin format handled by Hex's isolated host boundaries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PluginFormat {
    Vst3,
    Clap,
}

impl PluginFormat {
    /// Lowercase canonical identifier for UI labels, diagnostics, and codecs.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Vst3 => "vst3",
            Self::Clap => "clap",
        }
    }

    /// Expected macOS plugin bundle extension (without leading dot).
    pub const fn bundle_extension(self) -> &'static str {
        match self {
            Self::Vst3 => "vst3",
            Self::Clap => "clap",
        }
    }

    /// Standard macOS Audio Plug-Ins folder name under `/Library/Audio/Plug-Ins`
    /// and `~/Library/Audio/Plug-Ins`.
    pub const fn macos_folder_name(self) -> &'static str {
        match self {
            Self::Vst3 => "VST3",
            Self::Clap => "CLAP",
        }
    }

    /// Parse a bundle extension (case-insensitive) into a supported plugin format.
    pub fn from_extension(extension: &str) -> Option<Self> {
        if extension.eq_ignore_ascii_case("vst3") {
            Some(Self::Vst3)
        } else if extension.eq_ignore_ascii_case("clap") {
            Some(Self::Clap)
        } else {
            None
        }
    }
}

/// Queue-independent cancellation guardian owned on control.
///
/// Implementations must be safe to trigger on control even when the helper or
/// worker is blocked inside foreign code, without signalling numeric PIDs.
pub trait CancellationGuardian: Send + Sync {
    fn cancel_on_control(&self);
    fn is_cancelled_on_control(&self) -> bool;
}

/// Portable atomic cancellation guardian for headless tests and format adapters
/// that do not hold a live helper socket yet.
#[derive(Default)]
pub struct AtomicCancellationGuardian {
    cancelled: AtomicBool,
}

impl AtomicCancellationGuardian {
    pub const fn new() -> Self {
        Self {
            cancelled: AtomicBool::new(false),
        }
    }
}

impl CancellationGuardian for AtomicCancellationGuardian {
    fn cancel_on_control(&self) {
        self.cancelled.store(true, Ordering::Release);
    }

    fn is_cancelled_on_control(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl CancellationGuardian for crate::native::Cancellation {
    fn cancel_on_control(&self) {
        Self::cancel_on_control(self);
    }

    fn is_cancelled_on_control(&self) -> bool {
        Self::is_cancelled_on_control(self)
    }
}

/// Format-neutral control lease over the bounded callback/worker ring pair.
pub trait BoundedTransportControl {
    fn binding(&self) -> Binding;
    fn is_revoked(&self) -> bool;
    fn revoke(&self);
    fn wake_worker(&self);
    fn take_callback_fault(&self) -> Option<Fault>;
}

impl BoundedTransportControl for Control {
    fn binding(&self) -> Binding {
        Self::binding(self)
    }

    fn is_revoked(&self) -> bool {
        Self::is_revoked(self)
    }

    fn revoke(&self) {
        Self::revoke(self);
    }

    fn wake_worker(&self) {
        Self::wake_worker(self);
    }

    fn take_callback_fault(&self) -> Option<Fault> {
        Self::take_callback_fault(self)
    }
}

/// Format-neutral worker endpoint over the bounded callback/worker ring pair.
pub trait BoundedTransportWorker {
    fn binding(&self) -> Binding;
    fn quantum(&self) -> u16;
    fn is_revoked(&self) -> bool;
    fn revoke(&self);
    fn completed_blocks(&self) -> u64;
    fn has_queued_request(&self) -> bool;
    fn poll_request(&mut self) -> Result<Option<WorkerRequest>, Fault>;
    fn complete_block(&mut self, reply: Block) -> Result<(), Fault>;
}

impl BoundedTransportWorker for Worker {
    fn binding(&self) -> Binding {
        Self::binding(self)
    }

    fn quantum(&self) -> u16 {
        Self::quantum(self)
    }

    fn is_revoked(&self) -> bool {
        Self::is_revoked(self)
    }

    fn revoke(&self) {
        Self::revoke(self);
    }

    fn completed_blocks(&self) -> u64 {
        Self::completed_blocks(self)
    }

    fn has_queued_request(&self) -> bool {
        Self::has_queued_request(self)
    }

    fn poll_request(&mut self) -> Result<Option<WorkerRequest>, Fault> {
        Self::receive_request(self)
    }

    fn complete_block(&mut self, reply: Block) -> Result<(), Fault> {
        Self::complete(self, reply)
    }
}

/// Format-neutral supervision timeout bounds for isolated helper lifecycles.
pub trait PluginSupervisionLimits {
    fn startup_limit(&self) -> Duration;
    fn exchange_limit(&self) -> Duration;
    fn retirement_limit(&self) -> Duration;
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl PluginSupervisionLimits for crate::native::Limits {
    fn startup_limit(&self) -> Duration {
        self.startup
    }

    fn exchange_limit(&self) -> Duration {
        self.exchange
    }

    fn retirement_limit(&self) -> Duration {
        self.retirement
    }
}

/// Format-neutral non-RT control session handle for an isolated plugin helper.
pub trait HostedPluginSession {
    fn format(&self) -> PluginFormat;
    fn bundle_path(&self) -> &str;
    fn is_faulted(&self) -> bool;
    fn cancel_native(&mut self);
    fn check_deadlines(&self) -> crate::Result<()>;
}

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
impl HostedPluginSession for crate::native::ControlClient {
    fn format(&self) -> PluginFormat {
        PluginFormat::Vst3
    }

    fn bundle_path(&self) -> &str {
        Self::bundle_path(self)
    }

    fn is_faulted(&self) -> bool {
        Self::is_faulted(self)
    }

    fn cancel_native(&mut self) {
        Self::cancel_native(self);
    }

    fn check_deadlines(&self) -> crate::Result<()> {
        Self::check_deadlines(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport;

    fn test_binding() -> Binding {
        Binding {
            root: 1,
            instance: 2,
            graph: 3,
            epoch: 4,
        }
    }

    #[test]
    fn plugin_format_metadata_and_extension_parsing() {
        assert_eq!(PluginFormat::Vst3.as_str(), "vst3");
        assert_eq!(PluginFormat::Vst3.bundle_extension(), "vst3");
        assert_eq!(PluginFormat::Vst3.macos_folder_name(), "VST3");
        assert_eq!(PluginFormat::Clap.as_str(), "clap");
        assert_eq!(PluginFormat::Clap.bundle_extension(), "clap");
        assert_eq!(PluginFormat::Clap.macos_folder_name(), "CLAP");

        assert_eq!(
            PluginFormat::from_extension("vst3"),
            Some(PluginFormat::Vst3)
        );
        assert_eq!(
            PluginFormat::from_extension("VST3"),
            Some(PluginFormat::Vst3)
        );
        assert_eq!(
            PluginFormat::from_extension("clap"),
            Some(PluginFormat::Clap)
        );
        assert_eq!(
            PluginFormat::from_extension("CLAP"),
            Some(PluginFormat::Clap)
        );
        assert_eq!(PluginFormat::from_extension("component"), None);
    }

    #[test]
    fn format_neutral_transport_traits_round_trip_and_revoke_via_shared_rings() {
        fn pump_one_block(worker: &mut dyn BoundedTransportWorker) -> bool {
            if let Ok(Some(req)) = worker.poll_request() {
                let mut reply = req.block;
                for frame in &mut reply.pcm[..usize::from(reply.frames)] {
                    frame[0] *= 0.5;
                    frame[1] *= 0.5;
                }
                worker.complete_block(reply).expect("complete reply");
                true
            } else {
                false
            }
        }

        fn inspect_control(control: &dyn BoundedTransportControl) -> (Binding, bool) {
            (control.binding(), control.is_revoked())
        }

        let binding = test_binding();
        let (control, mut callback, mut worker) = transport::channel(binding, 4).unwrap();
        assert_eq!(inspect_control(&control), (binding, false));
        assert_eq!(worker.binding(), binding);
        assert_eq!(worker.quantum(), 4);
        assert!(!worker.has_queued_request());

        let input = [[0.8_f32, -0.4_f32]; 4];
        let mut out = [[0.0_f32; 2]; 4];
        callback.stage(&input, &[], &mut out).unwrap();
        callback.finish(true).unwrap();
        assert!(worker.has_queued_request());
        assert!(pump_one_block(&mut worker));
        assert_eq!(worker.completed_blocks(), 1);

        // Revoke through the format-neutral control trait and confirm propagation.
        let control_trait: &dyn BoundedTransportControl = &control;
        control_trait.revoke();
        assert!(control_trait.is_revoked());
        assert!(worker.is_revoked());
        assert!(matches!(worker.poll_request(), Err(Fault::Revoked)));
    }

    #[test]
    fn cancellation_guardian_trait_works_for_atomic_and_native_guardians() {
        fn trigger(guardian: &dyn CancellationGuardian) {
            assert!(!guardian.is_cancelled_on_control());
            guardian.cancel_on_control();
            assert!(guardian.is_cancelled_on_control());
        }

        let atomic = AtomicCancellationGuardian::new();
        trigger(&atomic);

        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            let prep = crate::native::PreparationCancellation::new_on_control().unwrap();
            let token = prep.token_on_control();
            trigger(&token);
        }
    }
}
