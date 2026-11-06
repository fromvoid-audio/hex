//! Actual Mac child control/worker custody. No process-global signal changes or
//! numeric PID signalling. The helper's pre-SDK guardian owns self-cancellation.
mod edits;
mod epoch;
mod live_capture;
mod parameter_page;
pub use epoch::Quiescent;
pub use parameter_page::AuthoredParameterMetadata;
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
    io::Cursor,
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
        })
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
    last_edit_frame: u64,
    parameter_page: Option<parameter_page::PendingPage>,
    parameter_page_sequence: u64,
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
            .stdin(Stdio::from(OwnedFd::from(child_data)))
            .stdout(Stdio::from(OwnedFd::from(child_cancel)))
            .stderr(Stdio::null());
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
            last_edit_frame: 0,
            parameter_page: None,
            parameter_page_sequence: 0,
        })
    }
}

impl Prepared {
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
        self.exchange_on_worker_with_metadata(block, None)
    }
    /// Deliver one authored input block to the same lease that observed the
    /// selected parameters. The SDK processing snapshot rechecks eligibility
    /// and steps before entering DSP. Normalized feedback cannot stale them.
    pub fn exchange_authored_on_worker(
        &mut self,
        block: Block,
        metadata: &AuthoredParameterMetadata,
    ) -> Result<Block> {
        if !metadata.matches(self) {
            return Err(Error::Protocol("authored parameter source differs"));
        }
        self.exchange_on_worker_with_metadata(block, Some(metadata))
    }
    fn exchange_on_worker_with_metadata(
        &mut self,
        block: Block,
        metadata: Option<&AuthoredParameterMetadata>,
    ) -> Result<Block> {
        if self.faulted || self.retired.is_some() {
            return Err(Error::Protocol("instance lease is retired or faulted"));
        }
        let result = self.exchange(block, metadata);
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
    ) -> Result<Block> {
        self.check_live_capture_deadline()?;
        self.check_parameter_page_deadline()?;
        self.config.validate_block(&block, self.sequence)?;
        let next = self
            .sequence
            .checked_add(1)
            .ok_or(Error::Protocol("instance sequence exhausted"))?;
        let mut bytes = [0; crate::transport::wire::MAX_PACKET_BYTES];
        let mut cursor = Cursor::new(bytes.as_mut_slice());
        if let Some(metadata) = metadata {
            crate::transport::wire::write_authored(&mut cursor, &block, &|id| {
                metadata.step_count(id)
            })
        } else {
            crate::transport::wire::write(&mut cursor, &block)
        }
        .map_err(Error::Io)?;
        let length = cursor.position() as usize;
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
        let output = protocol::pcm(self.packet.data(), &self.config, self.sequence)?;
        token.check()?;
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
