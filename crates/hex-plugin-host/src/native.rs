//! Actual Mac child control/worker custody. No process-global signal changes or
//! numeric PID signalling. The helper's pre-SDK guardian owns self-cancellation.
mod edits;
mod epoch;
mod live_capture;
mod parameter_page;
pub use epoch::Quiescent;
pub use parameter_page::{AuthoredParameterExtensionProgress, AuthoredParameterMetadata};
mod preparation_failure;
pub use preparation_failure::{PreparationDisposal, PreparationFailure};
mod storage;
pub use storage::{
    PendingStorage, PreparationResourceBound, PreparationStorageBound, PreparedStorage,
};

use crate::transport::Block;
use crate::{
    Error, Result,
    channel::{Channel, Packet, end_after, pause},
    protocol::{self, Configuration, Facts, PresentationLatency, Ready, Terminal},
};
use hex_lattice_contract::{ForeignProcessorBinding as Binding, NativeDeviceState};
use hex_plugin_scan::macos::{
    build_identity::InstanceBuild,
    identity::{Bundle, Executable},
};
use hex_plugin_state::{BinaryIdentity, ClassId, Identity};
use std::{
    io::{Cursor, Read},
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Source {
    bundle: Bundle,
    executable: Executable,
    identity: Identity,
    grouped_editor_callbacks: bool,
    transport_space: bool,
}
impl Source {
    pub fn inspect_on_control(path: &Path, executable: &str, class: ClassId) -> Result<Self> {
        let bundle = Bundle::inspect(path).map_err(Error::Source)?;
        let executable = bundle.admit(executable).map_err(Error::Source)?;
        let identity = Identity {
            class,
            binary: BinaryIdentity {
                executable_sha256: digest(&executable.sha256)?,
                info_sha256: digest(&bundle.info_sha256)?,
            },
        };
        Ok(Self {
            bundle,
            executable,
            identity,
            grouped_editor_callbacks: false,
            transport_space: false,
        })
    }
    /// Explicit opt-in on the exact inspected source before the SDK creates
    /// its component handler. This enables callbacks, not recording custody.
    pub fn with_grouped_editor_callbacks(mut self) -> Self {
        self.grouped_editor_callbacks = true;
        self
    }
    pub fn with_transport_space(mut self) -> Self {
        self.transport_space = true;
        self
    }
    pub fn identity(&self) -> Identity {
        self.identity
    }
    fn unchanged(&self) -> Result<()> {
        self.bundle
            .executable_unchanged(&self.executable)
            .map_err(Error::Validation)
    }
}
fn digest(text: &str) -> Result<[u8; 32]> {
    if text.len() != 64 {
        return Err(Error::Protocol("source digest width"));
    }
    let mut out = [0; 32];
    for (n, pair) in text.as_bytes().chunks_exact(2).enumerate() {
        let number = |c: u8| match c {
            b'0'..=b'9' => Ok(c - b'0'),
            b'a'..=b'f' => Ok(c - b'a' + 10),
            _ => Err(Error::Protocol("source digest encoding")),
        };
        out[n] = number(pair[0])? * 16 + number(pair[1])?;
    }
    Ok(out)
}
#[derive(Clone, Copy, Debug)]
pub struct Limits {
    pub startup: Duration,
    pub exchange: Duration,
    pub retirement: Duration,
}
impl Limits {
    fn validate(self) -> Result<()> {
        if [self.startup, self.exchange, self.retirement]
            .iter()
            .any(|v| v.is_zero() || *v > Duration::from_secs(30))
        {
            return Err(Error::Protocol(
                "supervision limits must be positive and at most30 seconds",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Preparation {
    pub binding: Binding,
    pub sample_rate: f64,
    pub quantum: u16,
    pub input_channels: u8,
    pub output_channels: u8,
    pub limits: Limits,
}
/// Known-presentation convenience preparation uses the same two-phase protocol.
#[derive(Clone, Copy, Debug)]
pub struct Options {
    pub preparation: Preparation,
    pub presentation: PresentationLatency,
}

/// Explicitly trusted user-selected module. This is caller consent, not a source
/// authenticity or native resource/publication permit.
pub enum TrustUserCode {
    Acknowledged,
}
/// Construct on control before starting preparation; retain its token for load
/// cancellation. The unique receiver can be moved into exactly one helper.
pub struct PreparationCancellation {
    token: Cancellation,
    receiver: UnixStream,
}
impl PreparationCancellation {
    pub fn new_on_control() -> Result<Self> {
        let (sender, receiver) = UnixStream::pair().map_err(Error::Io)?;
        // Only the parent reads shortcuts and shuts down its write half. The
        // helper's distinct socket still blocks in its cancellation guardian.
        sender.set_nonblocking(true).map_err(Error::Io)?;
        Ok(Self {
            token: Cancellation {
                stream: Arc::new(sender),
                cancelled: Arc::new(AtomicBool::new(false)),
            },
            receiver,
        })
    }
    pub fn token_on_control(&self) -> Cancellation {
        self.token.clone()
    }
}
#[derive(Clone)]
pub struct Cancellation {
    stream: Arc<UnixStream>,
    cancelled: Arc<AtomicBool>,
}
impl Cancellation {
    /// One editor Space token on the helper-to-parent half of this exact
    /// source's socket. The parent-to-helper half remains cancellation only.
    pub fn take_transport_space_on_control(&self) -> bool {
        let mut token = [0];
        !self.cancelled.load(Ordering::Acquire)
            && self.stream.as_ref().read(&mut token).ok() == Some(1)
            && token == [1]
    }
    /// Test-only lifetime observation; never resource provenance or admission.
    /// A weak observer cannot keep the descriptor payload alive.
    #[cfg(feature = "test-support")]
    pub fn descriptor_lifetime_probe_on_control(&self) -> std::sync::Weak<UnixStream> {
        Arc::downgrade(&self.stream)
    }
    /// Control-only, one-shot and independent of the PCM queue. The guardian's
    /// blocking read wakes on descriptor shutdown, even during foreign entry.
    pub fn cancel_on_control(&self) {
        self.cancelled.store(true, Ordering::Release);
        let _ = self.stream.shutdown(std::net::Shutdown::Write);
    }
    fn check(&self) -> Result<()> {
        if self.cancelled.load(Ordering::Acquire) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Retirement {
    pub status: ExitStatus,
    pub terminal: Option<Terminal>,
}
#[derive(Clone, Copy)]
enum EditorTouchMode<'a> {
    Single(protocol::EditorTouchPlan),
    Group(&'a protocol::EditorTouchPlanSet),
}

/// Unique actual child. Move this into the existing joining worker, never into
/// the callback. Drop may wait for actual process retirement on that lane.
pub struct Prepared {
    lease: Lease,
    config: Configuration,
    ready: Ready,
    limits: Limits,
    sequence: u64,
    packet: Packet,
    retired: Option<Retirement>,
    faulted: bool,
    live_capture: Option<live_capture::PendingCapture>,
    live_capture_sequence: u64,
    edit_query_sequence: u64,
    idle_flush_sequence: u64,
    last_edit_sequence: u64,
    last_pcm_edit_sequence: u64,
    last_pcm_restoration_sequence: u64,
    last_edit_frame: u64,
    parameter_page: Option<parameter_page::PendingPage>,
    parameter_page_sequence: u64,
    authored_parameter_metadata: Option<AuthoredParameterMetadata>,
    grouped_editor_callbacks: bool,
    offline: bool,
}
/// Source-bound configured component awaiting actual graph presentation facts.
/// No processing/state capture or canonical/audio acceptance has occurred. Keep
/// on control; dropping it cancels and joins this exact child through its lease.
/// Derive the graph plan from `facts`, then consume it once through completion.
pub struct Pending {
    lease: Lease,
    config: Configuration,
    facts: Facts,
    limits: Limits,
    packet: Packet,
    source: Source,
    build: InstanceBuild,
    offline: bool,
}
impl Pending {
    /// Preserve actual failed-construction custody for the host's already
    /// reserved Root cell. Failure grants no source/graph admission or disposal.
    pub fn prepare_retained_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> std::result::Result<Self, PreparationFailure> {
        Self::prepare_retained(build, source, preparation, None, cancellation, |_| {})
    }
    /// The retained form of canonical startup restore; original state is not
    /// replaced by capture or by any later preparation failure.
    pub fn prepare_restored_retained_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> std::result::Result<Self, PreparationFailure> {
        Self::prepare_retained(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |_| {},
        )
    }
    pub fn prepare_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare(build, source, preparation, cancellation, |_| {})
    }
    /// Recreate captured state on a new source-bound instance. This never
    /// edits the caller's canonical envelope, even if later capture differs.
    pub fn prepare_restored_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare_with_restore(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |_| {},
        )
    }
    /// Configure a new source in the SDK's offline mode from confirmed state.
    /// The caller owns it independently of any live processor or output.
    pub fn prepare_offline_restored_on_control(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        state: NativeDeviceState,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare_with_restore(
            build,
            source,
            preparation,
            Some(state),
            cancellation,
            |command| {
                command.arg("--offline");
            },
        )
        .map(|mut pending| {
            pending.offline = true;
            pending
        })
    }
    fn prepare(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> Result<Self> {
        Self::prepare_with_restore(build, source, preparation, None, cancellation, setup)
    }
    fn prepare_with_restore(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> Result<Self> {
        Self::prepare_retained(build, source, preparation, restore, cancellation, setup)
            .map_err(PreparationFailure::finish_on_control)
    }
    fn prepare_retained(
        build: &InstanceBuild,
        source: Source,
        preparation: Preparation,
        restore: Option<NativeDeviceState>,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> std::result::Result<Self, PreparationFailure> {
        let options = preparation;
        cancellation
            .token
            .check()
            .map_err(PreparationFailure::before_native)?;
        options
            .limits
            .validate()
            .map_err(PreparationFailure::before_native)?;
        let config = Configuration {
            binding: options.binding,
            identity: source.identity,
            restore,
            sample_rate: options.sample_rate,
            quantum: options.quantum,
            input_channels: options.input_channels,
            output_channels: options.output_channels,
            bundle: storage::copy_configuration_path(
                source
                    .bundle
                    .canonical_path
                    .to_str()
                    .ok_or(Error::Protocol("bundle path encoding"))
                    .map_err(PreparationFailure::before_native)?,
            )
            .map_err(PreparationFailure::before_native)?,
            executable: storage::copy_configuration_path(&source.executable.relative_path)
                .map_err(PreparationFailure::before_native)?,
        };
        let request = config.encode().map_err(PreparationFailure::before_native)?;
        // Establish the eventual Pending's bounded provenance copy before any
        // probe or instance child starts, never through a late derived Clone.
        let owned_build = build
            .try_copy_on_control()
            .map_err(Error::Validation)
            .map_err(PreparationFailure::before_native)?;
        source
            .unchanged()
            .map_err(PreparationFailure::before_native)?;
        build
            .unchanged()
            .map_err(Error::Validation)
            .map_err(PreparationFailure::before_native)?;
        // No foreign module is loaded by this exact executable's probe. A
        // missing/auto-reaped child refuses before any SDK instance starts.
        let probe = Command::new(build.helper())
            .arg("--child-custody-probe")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        let mut probe = ChildCustody::Owned(probe);
        let status = match probe.reap() {
            Ok(status) => status,
            Err(cause) => return Err(PreparationFailure::probe(cause, probe)),
        };
        if !status.success() {
            return Err(PreparationFailure::before_native(Error::Custody(
                "waitable-child preflight did not exit normally".into(),
            )));
        }
        build
            .unchanged()
            .map_err(Error::Validation)
            .map_err(PreparationFailure::before_native)?;
        source
            .unchanged()
            .map_err(PreparationFailure::before_native)?;
        let (data, child_data) = UnixStream::pair()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        let PreparationCancellation {
            token: cancellation,
            receiver: child_cancel,
        } = cancellation;
        let channel = Channel::new(data).map_err(PreparationFailure::before_native)?;
        let mut packet = Packet::new().map_err(PreparationFailure::before_native)?;
        let mut command = Command::new(build.helper());
        command
            .arg("--supervised")
            .env(
                "HEX_VST3_TRANSPORT_SPACE",
                if source.transport_space { "1" } else { "0" },
            )
            .stdin(Stdio::from(OwnedFd::from(child_data)))
            .stdout(Stdio::from(OwnedFd::from(child_cancel)))
            .stderr(Stdio::null());
        if source.grouped_editor_callbacks {
            command.arg("--test-grouped-editor");
        }
        setup(&mut command);
        let child = command
            .spawn()
            .map_err(Error::Io)
            .map_err(PreparationFailure::before_native)?;
        // Command retains its Stdio descriptors for reuse. Close the parent
        // copies now, otherwise a crashed/cancelled child cannot produce EOF.
        drop(command);
        // The process exists before Ready. The local lease below owns every
        // early error path, including malformed data/source changes/timeouts.
        let mut pending = Lease {
            child: ChildCustody::Owned(child),
            channel,
            cancellation,
            #[cfg(test)]
            reap_probe: None,
        };
        let token = pending.cancellation.clone();
        let prepared = (|| {
            let until = end_after(options.limits.startup)?;
            pending
                .channel
                .send(&request, until, &mut || token.check())?;
            pending
                .channel
                .send(&config.startup_request()?, until, &mut || token.check())?;
            pending
                .channel
                .read(&mut packet, until, &mut || token.check())?;
            if packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    packet.body()?,
                    config.binding,
                )?));
            }
            if packet.magic() != b"HXVIFA04" {
                return Err(Error::Protocol("worker did not report preparation facts"));
            }
            let facts = Facts::decode(packet.body()?, &config)?;
            token.check()?;
            source.unchanged()?;
            build.unchanged().map_err(Error::Validation)?;
            match pending.child.observe()? {
                None => {}
                Some(_) => return Err(Error::Custody("prepared worker already exited".into())),
            }
            Ok(facts)
        })();
        let facts = match prepared {
            Ok(facts) => facts,
            Err(cause) => {
                return Err(PreparationFailure::native(
                    cause,
                    pending,
                    options.limits.retirement,
                ));
            }
        };
        Ok(Self {
            lease: pending,
            config,
            facts,
            limits: options.limits,
            packet,
            source,
            build: owned_build,
            offline: false,
        })
    }
    pub fn facts(&self) -> Facts {
        self.facts
    }
    /// Immutable facts of this actual pending lease, not admission authority.
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub fn cancellation_on_control(&self) -> Cancellation {
        self.lease.cancellation.clone()
    }
    /// Cancel this actual pending source and retain its physical cleanup owner
    /// for Root polling. No source/graph can be accepted from the returned error.
    pub fn cancel_retained_on_control(self) -> PreparationFailure {
        PreparationFailure::native(Error::Cancelled, self.lease, self.limits.retirement)
    }
    /// Explicit pending cancellation with actual direct reap. This is OS
    /// retirement, not a successful SDK close or prepared-instance terminal.
    pub fn cancel_and_reap_on_control(mut self) -> Result<Retirement> {
        self.lease.cancel_native();
        let status = wait(
            &mut self.lease.child,
            end_after(self.limits.retirement)?,
            &mut || Ok(()),
        )?;
        Ok(Retirement {
            status,
            terminal: None,
        })
    }
    /// Supply actual per-direction graph/device contributions, or explicit None
    /// when unavailable. A present optional interface requires known facts;
    /// absence may complete without inventing zero. Source and final Ready are
    /// checked again; a changed reported latency invalidates the caller's plan.
    pub fn complete_on_control(
        self,
        presentation: Option<PresentationLatency>,
    ) -> Result<Prepared> {
        self.complete_with_plan_on_control(presentation.into())
    }
    pub fn complete_with_plan_on_control(
        self,
        presentation: protocol::PresentationPlan,
    ) -> Result<Prepared> {
        self.complete_retained_on_control(presentation)
            .map_err(PreparationFailure::finish_on_control)
    }
    /// Completion errors retain the same actual child for Root's independent
    /// cleanup observation. Callers can explicitly allow unknown presentation.
    pub fn complete_retained_on_control(
        mut self,
        presentation: impl Into<protocol::PresentationPlan>,
    ) -> std::result::Result<Prepared, PreparationFailure> {
        let presentation = presentation.into();
        let token = self.lease.cancellation.clone();
        let result = (|| {
            token.check()?;
            self.source.unchanged()?;
            self.build.unchanged().map_err(Error::Validation)?;
            let expected = presentation.expected(self.facts.presentation_available)?;
            let until = end_after(self.limits.startup)?;
            let request = protocol::presentation_request(self.config.binding, presentation)?;
            self.lease
                .channel
                .send(&request, until, &mut || token.check())?;
            self.lease
                .channel
                .read(&mut self.packet, until, &mut || token.check())?;
            if self.packet.magic() == b"HXVIER01" {
                return Err(Error::Worker(protocol::error(
                    self.packet.body()?,
                    self.config.binding,
                )?));
            }
            if self.packet.magic() != b"HXVIRD05" {
                return Err(Error::Protocol(
                    "missing final prepared instance acknowledgement",
                ));
            }
            let ready = Ready::decode(self.packet.body()?, &self.config, presentation)?;
            if ready.restored != self.facts.restored
                || ready.algorithmic_latency_frames != self.facts.algorithmic_latency_frames
                || ready.tail_frames != self.facts.tail_frames
                || ready.presentation != expected
            {
                return Err(Error::Protocol(
                    "prepared facts changed before final acknowledgement",
                ));
            }
            self.source.unchanged()?;
            self.build.unchanged().map_err(Error::Validation)?;
            if self.lease.child.observe()?.is_some() {
                return Err(Error::Custody("prepared worker already exited".into()));
            }
            token.check()?;
            Ok(ready)
        })();
        let ready = match result {
            Ok(ready) => ready,
            Err(cause) => {
                return Err(PreparationFailure::native(
                    cause,
                    self.lease,
                    self.limits.retirement,
                ));
            }
        };
        Ok(Prepared {
            lease: self.lease,
            config: self.config,
            ready,
            limits: self.limits,
            sequence: 0,
            packet: self.packet,
            retired: None,
            faulted: false,
            live_capture: None,
            live_capture_sequence: 0,
            edit_query_sequence: 0,
            idle_flush_sequence: 0,
            last_edit_sequence: 0,
            last_pcm_edit_sequence: 0,
            last_pcm_restoration_sequence: 0,
            last_edit_frame: 0,
            parameter_page: None,
            parameter_page_sequence: 0,
            authored_parameter_metadata: None,
            grouped_editor_callbacks: self.source.grouped_editor_callbacks,
            offline: self.offline,
        })
    }
}

impl Prepared {
    /// The original helper was started in SDK offline processing mode.
    pub const fn is_offline_on_control(&self) -> bool {
        self.offline
    }

    /// Cancel an instance still held by control during host preparation and
    /// retain its actual lease for independent, nonblocking disposal observation.
    /// This neither returns any callback endpoint nor grants graph admission.
    pub fn cancel_retained_on_control(self) -> PreparationFailure {
        PreparationFailure::native(Error::Cancelled, self.lease, self.limits.retirement)
    }
    /// Off-callback preparation. The caller must not independently reap these
    /// children or change their signal disposition while they are owned here.
    pub fn prepare_on_control(
        build: &InstanceBuild,
        source: Source,
        options: Options,
        cancellation: PreparationCancellation,
        _trust: TrustUserCode,
    ) -> Result<Self> {
        Self::prepare(build, source, options, cancellation, |_| {})
    }
    fn prepare(
        build: &InstanceBuild,
        source: Source,
        options: Options,
        cancellation: PreparationCancellation,
        setup: impl FnOnce(&mut Command),
    ) -> Result<Self> {
        Pending::prepare(build, source, options.preparation, cancellation, setup)?
            .complete_on_control(Some(options.presentation))
    }
    /// Cumulative actual successful PCM exchanges, not callback/graph acceptance.
    pub fn completed_blocks_on_control(&self) -> u64 {
        self.sequence
    }
    pub fn grouped_editor_callbacks_enabled(&self) -> bool {
        self.grouped_editor_callbacks
    }
    pub fn ready(&self) -> &Ready {
        &self.ready
    }
    pub fn cancellation_on_control(&self) -> Cancellation {
        self.lease.cancellation.clone()
    }
    pub fn configuration(&self) -> &Configuration {
        &self.config
    }
    pub fn child_id_on_control(&self) -> Option<u32> {
        match &self.lease.child {
            ChildCustody::Owned(child) => Some(child.id()),
            _ => None,
        }
    }
    /// Only on the existing joining transport worker. A failed operation
    /// permanently faults this lease; no foreign DSP retry/replay is possible.
    pub fn exchange_on_worker(&mut self, block: Block) -> Result<Block> {
        self.exchange_on_worker_with_metadata(block, None, None, false, None, None)
            .map(|reply| reply.block)
    }

    /// Return the original PCM reply and its helper-side diagnostic timing.
    pub fn exchange_timed_on_worker(&mut self, block: Block) -> Result<protocol::EditorPcm> {
        self.exchange_on_worker_with_metadata(block, None, None, false, None, None)
    }

    /// Opt this exchange into facts about the effective editor input. This
    /// does not drain or acknowledge the ordered editor receipt reader.
    pub fn exchange_editor_on_worker(&mut self, block: Block) -> Result<protocol::EditorPcm> {
        self.exchange_on_worker_with_metadata(block, None, None, true, None, None)
    }
    /// Apply an eligible unlaned editor touch with the original normalized base.
    /// The caller must bind this plan to the actual canonical source.
    pub fn exchange_editor_touch_on_worker(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
    ) -> Result<protocol::EditorPcm> {
        if !self.has_armed_authored_parameter(plan.parameter) || plan.reading {
            return Err(Error::Protocol(
                "editor touch requires an exact writable unlaned target",
            ));
        }
        self.exchange_on_worker_with_metadata(
            block,
            None,
            None,
            true,
            Some(EditorTouchMode::Single(plan)),
            None,
        )
    }

    /// Deliver one authored input block to the same lease that observed the
    /// selected parameters. The SDK processing snapshot rechecks eligibility
    /// and steps before entering DSP. Normalized feedback cannot stale them.
    pub fn exchange_authored_on_worker(
        &mut self,
        block: Block,
        metadata: &AuthoredParameterMetadata,
    ) -> Result<Block> {
        self.exchange_authored_with_manual_on_worker(block, metadata, None, false, None)
            .map(|reply| reply.block)
    }

    /// Deliver one dense authored lane using metadata from this exact lease.
    /// The worker rechecks the raw ID and SDK step count before applying it.
    pub fn exchange_dense_authored_on_worker(
        &mut self,
        block: Block,
        metadata: &AuthoredParameterMetadata,
        dense: crate::transport::DenseParameterLane<'_>,
    ) -> Result<Block> {
        if !metadata.matches(self) {
            return Err(Error::Protocol("authored parameter source differs"));
        }
        self.exchange_on_worker_with_metadata(block, Some(metadata), None, false, None, Some(dense))
            .map(|reply| reply.block)
    }

    /// The prepared callback sidecar uses the metadata retained by its own
    /// armed lease. An editor touch plan is a separate ownership admission.
    pub fn exchange_armed_dense_on_worker(
        &mut self,
        block: Block,
        dense: crate::transport::DenseParameterLane<'_>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
    ) -> Result<protocol::EditorPcm> {
        let metadata = self
            .authored_parameter_metadata
            .take()
            .ok_or(Error::Protocol("dense parameter metadata is not armed"))?;
        let result = if metadata.matches(self) {
            self.exchange_on_worker_with_metadata(
                block,
                Some(&metadata),
                manual_parameter,
                observe_editor,
                None,
                Some(dense),
            )
        } else {
            Err(Error::Protocol("authored parameter source differs"))
        };
        self.authored_parameter_metadata = Some(metadata);
        result
    }

    fn exchange_authored_with_manual_on_worker(
        &mut self,
        block: Block,
        metadata: &AuthoredParameterMetadata,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
    ) -> Result<protocol::EditorPcm> {
        if !metadata.matches(self) {
            return Err(Error::Protocol("authored parameter source differs"));
        }
        self.exchange_on_worker_with_metadata(
            block,
            Some(metadata),
            manual_parameter,
            observe_editor,
            editor_touch,
            None,
        )
    }
    /// Retain the exact capability observation with the lease that will run
    /// authored requests. Admission and endpoint construction remain with
    /// the caller. A renewed epoch must install a new observation.
    pub fn arm_authored_parameter_metadata_on_control(
        &mut self,
        mut metadata: AuthoredParameterMetadata,
    ) -> Result<()> {
        self.require_live_epoch()?;
        if !metadata.matches(self) {
            return Err(Error::Protocol("authored parameter source differs"));
        }
        metadata.retain_reserved_capacity(self.authored_parameter_metadata.as_ref())?;
        self.authored_parameter_metadata = Some(metadata);
        Ok(())
    }

    /// Used only by the original joining worker when admitting a live hold.
    pub fn has_armed_authored_parameter(&self, id: u32) -> bool {
        self.authored_parameter_metadata
            .as_ref()
            .is_some_and(|metadata| metadata.matches(self) && metadata.step_count(id).is_some())
    }

    pub fn exchange_armed_authored_on_worker(&mut self, block: Block) -> Result<Block> {
        self.exchange_armed_authored_with_manual_on_worker(block, None)
    }

    /// On the joining worker, distinguish its one newly admitted manual input
    /// from prepared authored points. Native SDK metadata validates both kinds.
    pub fn exchange_armed_authored_with_manual_on_worker(
        &mut self,
        block: Block,
        manual_parameter: Option<crate::transport::ParameterPoint>,
    ) -> Result<Block> {
        self.exchange_armed_with_mode(block, manual_parameter, false, None)
            .map(|reply| reply.block)
    }

    /// Observe editor input alongside the exact source-bound authored quantum.
    pub fn exchange_armed_authored_with_editor_on_worker(
        &mut self,
        block: Block,
        manual_parameter: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_armed_with_mode(block, manual_parameter, true, None)
    }

    /// Return the original authored PCM reply and its helper-side diagnostic timing.
    pub fn exchange_armed_authored_timed_on_worker(
        &mut self,
        block: Block,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_armed_with_mode(block, manual, false, None)
    }

    /// A scoped editor gesture may temporarily replace one authored parameter.
    /// Other authored values retain their existing per-sample SDK contract.
    pub fn exchange_armed_authored_with_editor_touch_on_worker(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        if !self.has_armed_authored_parameter(plan.parameter)
            || manual.is_some_and(|point| point.id == plan.parameter)
        {
            return Err(Error::Protocol(
                "editor touch requires an exact writable target",
            ));
        }
        let points = block.parameters.as_slice();
        let has_target = points.iter().any(|point| point.id == plan.parameter);
        if (plan.reading
            && !points
                .iter()
                .any(|point| point.id == plan.parameter && point.frame == 0))
            || (!plan.reading
                && (has_target || points.len() == crate::transport::MAX_PARAMETER_POINTS))
        {
            return Err(Error::Protocol(
                "editor touch cannot reserve its exact restoration",
            ));
        }
        self.exchange_armed_with_mode(block, manual, true, Some(EditorTouchMode::Single(plan)))
    }

    /// Exact grouped unlaned takeover on the original prepared SDK worker.
    pub fn exchange_editor_group_touch_on_worker(
        &mut self,
        block: Block,
        plan: &protocol::EditorTouchPlanSet,
    ) -> Result<protocol::EditorPcm> {
        self.validate_editor_group(&block, plan, None)?;
        if plan.targets().iter().any(|target| target.reading) {
            return Err(Error::Protocol("unlaned group cannot read authored input"));
        }
        self.exchange_on_worker_with_metadata(
            block,
            None,
            None,
            true,
            Some(EditorTouchMode::Group(plan)),
            None,
        )
    }

    /// Exact grouped takeover with authored input and per-target restoration.
    pub fn exchange_armed_authored_with_editor_group_touch_on_worker(
        &mut self,
        block: Block,
        plan: &protocol::EditorTouchPlanSet,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.validate_editor_group(&block, plan, manual)?;
        self.exchange_armed_with_mode(block, manual, true, Some(EditorTouchMode::Group(plan)))
    }

    fn validate_editor_group(
        &self,
        block: &Block,
        plan: &protocol::EditorTouchPlanSet,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<()> {
        let points = block.parameters.as_slice();
        let mut reserved = points.len() + usize::from(manual.is_some());
        for target in plan.targets() {
            let id = target.parameter;
            if !self.has_armed_authored_parameter(id) || manual.is_some_and(|point| point.id == id)
            {
                return Err(Error::Protocol(
                    "editor group requires exact writable targets",
                ));
            }
            if target.reading {
                if !points
                    .iter()
                    .any(|point| point.id == id && point.frame == 0)
                {
                    return Err(Error::Protocol(
                        "editor group is missing its authored restoration",
                    ));
                }
            } else {
                if points.iter().any(|point| point.id == id) {
                    return Err(Error::Protocol("unlaned group target has authored input"));
                }
                reserved += 1;
            }
        }
        if reserved > crate::transport::MAX_PARAMETER_POINTS {
            return Err(Error::Protocol(
                "editor group cannot reserve its exact restorations",
            ));
        }
        Ok(())
    }

    fn exchange_armed_with_mode(
        &mut self,
        block: Block,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
    ) -> Result<protocol::EditorPcm> {
        let metadata = self
            .authored_parameter_metadata
            .take()
            .ok_or(Error::Protocol(
                "authored parameter capability is not armed",
            ))?;
        let result = self.exchange_authored_with_manual_on_worker(
            block,
            &metadata,
            manual_parameter,
            observe_editor,
            editor_touch,
        );
        self.authored_parameter_metadata = Some(metadata);
        result
    }

    fn exchange_on_worker_with_metadata(
        &mut self,
        block: Block,
        metadata: Option<&AuthoredParameterMetadata>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
        dense: Option<crate::transport::DenseParameterLane<'_>>,
    ) -> Result<protocol::EditorPcm> {
        if self.faulted || self.retired.is_some() {
            return Err(Error::Protocol("instance lease is retired or faulted"));
        }
        let result = self.exchange(
            block,
            metadata,
            manual_parameter,
            observe_editor,
            editor_touch,
            dense,
        );
        if result.is_err() {
            self.faulted = true;
            self.lease.cancel_native();
        }
        result
    }
    fn exchange(
        &mut self,
        block: Block,
        metadata: Option<&AuthoredParameterMetadata>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
        dense: Option<crate::transport::DenseParameterLane<'_>>,
    ) -> Result<protocol::EditorPcm> {
        self.check_live_capture_deadline()?;
        self.check_parameter_page_deadline()?;
        self.config.validate_block(&block, self.sequence)?;
        let next = self
            .sequence
            .checked_add(1)
            .ok_or(Error::Protocol("instance sequence exhausted"))?;
        let bytes = self.packet.request_buffer(
            crate::transport::wire::MAX_DENSE_PACKET_BYTES
                + protocol::EditorTouchPlanSet::WIRE_BYTES,
        )?;
        let mut cursor = Cursor::new(&mut *bytes);
        match (metadata, dense) {
            (Some(metadata), Some(dense)) => crate::transport::wire::write_dense_authored(
                &mut cursor,
                &block,
                dense,
                manual_parameter,
                &|id| metadata.step_count(id),
            ),
            (Some(metadata), None) => crate::transport::wire::write_authored(
                &mut cursor,
                &block,
                manual_parameter,
                &|id| metadata.step_count(id),
            ),
            (None, None) => crate::transport::wire::write(&mut cursor, &block),
            (None, Some(_)) => return Err(Error::Protocol("dense parameter metadata is missing")),
        }
        .map_err(Error::Io)?;
        let mut length = cursor.position() as usize;
        if let Some(plan) = editor_touch {
            let mut append = |plan: &[u8], bit: u16| -> Result<()> {
                let end = length + plan.len();
                bytes
                    .get_mut(length..end)
                    .ok_or(Error::Protocol(
                        "editor touch packet extent exceeds its bound",
                    ))?
                    .copy_from_slice(plan);
                length = end;
                bytes[5] |= (bit >> 8) as u8;
                Ok(())
            };
            match plan {
                EditorTouchMode::Single(plan) => append(&plan.wire()?, protocol::EDITOR_TOUCH_BIT)?,
                EditorTouchMode::Group(plan) => {
                    append(&plan.wire(), protocol::EDITOR_GROUP_TOUCH_BIT)?
                }
            }
        }
        if observe_editor {
            bytes[5] |= (protocol::EDITOR_PCM_BIT >> 8) as u8;
        }
        bytes[5] |= (protocol::SDK_TIMING_BIT >> 8) as u8;
        let until = end_after(self.limits.exchange)?;
        let token = self.lease.cancellation.clone();
        self.lease
            .channel
            .send(&bytes[..length], until, &mut || token.check())?;
        self.lease
            .channel
            .read(&mut self.packet, until, &mut || token.check())?;
        if self.packet.magic() == b"HXVIER01" {
            return Err(Error::Worker(protocol::error(
                self.packet.body()?,
                self.config.binding,
            )?));
        }
        let output = if let Some(EditorTouchMode::Single(plan)) = editor_touch {
            protocol::editor_touch_pcm(
                self.packet.data(),
                &self.config,
                self.sequence,
                self.last_pcm_edit_sequence,
                plan,
                self.last_pcm_restoration_sequence,
            )?
        } else if let Some(EditorTouchMode::Group(plan)) = editor_touch {
            protocol::editor_group_touch_pcm(
                self.packet.data(),
                &self.config,
                self.sequence,
                self.last_pcm_edit_sequence,
                plan,
                self.last_pcm_restoration_sequence,
            )?
        } else if observe_editor {
            protocol::editor_pcm(
                self.packet.data(),
                &self.config,
                self.sequence,
                self.last_pcm_edit_sequence,
            )?
        } else {
            let (block, sdk_process_ns) =
                protocol::timed_pcm(self.packet.data(), &self.config, self.sequence)?;
            protocol::EditorPcm {
                block,
                performs: protocol::EditorPerforms::default(),
                restorations: Default::default(),
                sdk_process_ns,
            }
        };
        if output.sdk_process_ns.is_none() {
            return Err(Error::Protocol("missing requested SDK processing timing"));
        }
        token.check()?;
        if let Some(last) = output.performs.as_slice().last() {
            self.last_pcm_edit_sequence = last.sequence;
        }
        if let Some(restored) = output.restorations.as_slice().last() {
            self.last_pcm_restoration_sequence = restored.sequence;
        }
        self.sequence = next;
        Ok(output)
    }
    pub fn stop_and_reap_on_control(&mut self) -> Result<Retirement> {
        if self.parameter_page.is_some() {
            return Err(Error::Protocol("controller page is still pending"));
        }
        if self.live_capture.is_some() {
            return Err(Error::Protocol("native capture is still pending"));
        }
        if self.faulted {
            return Err(Error::Protocol("faulted instance requires cancellation"));
        }
        if let Some(value) = self.retired {
            return Ok(value);
        }
        let frames = self
            .sequence
            .checked_mul(u64::from(self.config.quantum))
            .ok_or(Error::Protocol("instance position exhausted"))?;
        let until = end_after(self.limits.retirement)?;
        let token = self.lease.cancellation.clone();
        let work = (|| {
            self.lease.channel.send(
                &protocol::stop(self.config.binding, self.sequence, frames),
                until,
                &mut || token.check(),
            )?;
            self.lease
                .channel
                .read(&mut self.packet, until, &mut || token.check())?;
            if self.packet.magic() != b"HXVITR01" {
                return Err(Error::Protocol("missing instance retirement terminal"));
            }
            let terminal = Terminal::decode(self.packet.body()?, self.config.binding)?;
            let status = wait(&mut self.lease.child, until, &mut || token.check())?;
            let retired = Retirement {
                status,
                terminal: Some(terminal),
            };
            self.retired = Some(retired);
            token.check()?;
            self.lease.channel.reject_queued_after_reap()?;
            // A clean process-only stop can leave the module mapped until
            // the OS reclaims it. Require the same terminal and reaped status.
            if terminal.failure != 0
                || !terminal.closed
                || terminal.blocks != self.sequence
                || terminal.frames != frames
                || !status.success()
            {
                return Err(Error::Protocol(
                    "instance retirement was not clean normal completion",
                ));
            }
            Ok(retired)
        })();
        if work.is_err() {
            self.faulted = true;
            self.lease.cancel_native();
        }
        work
    }
    pub fn cancel_and_reap_on_control(&mut self) -> Result<Retirement> {
        if let Some(value) = self.retired {
            return Ok(value);
        }
        self.faulted = true;
        self.lease.cancel_native();
        let until = end_after(self.limits.retirement)?;
        let status = wait(&mut self.lease.child, until, &mut || Ok(()))?;
        let retired = Retirement {
            status,
            terminal: None,
        };
        self.retired = Some(retired);
        Ok(retired)
    }
}
struct Lease {
    child: ChildCustody,
    channel: Channel,
    cancellation: Cancellation,
    #[cfg(test)]
    reap_probe: Option<Arc<std::sync::atomic::AtomicI32>>,
}
// The process handle is consumed exactly once. Lost custody is terminal: even
// cleanup/Drop must not retry a numeric wait against a potentially reused PID.
enum ChildCustody {
    Owned(Child),
    Reaped(ExitStatus),
    Lost(Arc<std::io::Error>),
}
impl ChildCustody {
    fn observe(&mut self) -> Result<Option<ExitStatus>> {
        let result = match self {
            Self::Owned(child) => child.try_wait(),
            Self::Reaped(status) => return Ok(Some(*status)),
            Self::Lost(error) => {
                return Err(Error::Custody(crate::CustodyError::Lost(Arc::clone(error))));
            }
        };
        match result {
            Ok(Some(status)) => {
                *self = Self::Reaped(status);
                Ok(Some(status))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                let source = Arc::new(error);
                *self = Self::Lost(Arc::clone(&source));
                Err(Error::Custody(crate::CustodyError::Lost(source)))
            }
        }
    }
    fn reap(&mut self) -> Result<ExitStatus> {
        let result = match self {
            Self::Owned(child) => child.wait(),
            Self::Reaped(status) => return Ok(*status),
            Self::Lost(error) => {
                return Err(Error::Custody(crate::CustodyError::Lost(Arc::clone(error))));
            }
        };
        match result {
            Ok(status) => {
                *self = Self::Reaped(status);
                Ok(status)
            }
            Err(error) => {
                let source = Arc::new(error);
                *self = Self::Lost(Arc::clone(&source));
                Err(Error::Custody(crate::CustodyError::Lost(source)))
            }
        }
    }
}
impl Lease {
    fn cancel_native(&self) {
        self.cancellation.cancel_on_control();

        self.channel.close();
    }
}
fn wait(
    child: &mut ChildCustody,
    until: Instant,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<ExitStatus> {
    loop {
        check()?;
        match child.observe() {
            Ok(Some(s)) => return Ok(s),
            Ok(None) => {}
            Err(e) => return Err(e),
        }
        if Instant::now() >= until {
            return Err(Error::Timeout);
        }
        pause(until);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        self.cancel_native();
        let reaped = self.child.reap(); // joining control/worker only; lost custody never retries
        #[cfg(test)]
        if let Some(probe) = &self.reap_probe {
            let code = match &reaped {
                Ok(status) => status.code().unwrap_or(-1),
                Err(_) => -2,
            };
            probe.store(code, Ordering::Release);
        }
        let _ = reaped;
    }
}

#[cfg(test)]
#[path = "native_tests.rs"]
mod tests;
