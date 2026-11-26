//! Test-only instrumentation hooks for `NativeAudioSink` under `allocation-probe`.
use super::output_lifecycle::STARTUP_OPEN_DEADLINE;
use hex_audio::WorkerPreparedOutput;
use std::{
    sync::mpsc::{self, SyncSender},
    time::Duration,
};

pub(crate) type OutputFactory = Box<
    dyn FnMut(Option<u32>) -> Result<Option<WorkerPreparedOutput>, hex_audio::OutputOpenError>
        + Send,
>;

pub(crate) struct VerifiedPageHold {
    pub(crate) index: u64,
    pub(crate) armed: SyncSender<(mpsc::Receiver<()>, SyncSender<()>)>,
}

pub(crate) struct TestHooks {
    pub(crate) native_graph_gate: Option<(SyncSender<()>, mpsc::Receiver<()>)>,
    pub(crate) native_graph_busy_observer: Option<SyncSender<()>>,
    pub(crate) verified_page_hold: Option<VerifiedPageHold>,
    pub(crate) startup_open_deadline: Duration,
    pub(crate) resident_files_deadline: Duration,
    pub(crate) project_rate_probe: Box<dyn Fn() -> Option<u32> + Send>,
    pub(crate) output_factory: Option<OutputFactory>,
    pub(crate) lifecycle_status: Option<fn() -> Option<hex_audio::AudioLifecycleFailure>>,
}

impl Default for TestHooks {
    fn default() -> Self {
        Self {
            native_graph_gate: None,
            native_graph_busy_observer: None,
            verified_page_hold: None,
            startup_open_deadline: STARTUP_OPEN_DEADLINE,
            resident_files_deadline: STARTUP_OPEN_DEADLINE,
            project_rate_probe: Box::new(|| None),
            output_factory: None,
            lifecycle_status: None,
        }
    }
}
