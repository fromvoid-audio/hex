//! Actual Mac child control/worker custody. No process-global signal changes or
//! numeric PID signalling. The helper's pre-SDK guardian owns self-cancellation.
mod control;
pub use control::ControlClient;
mod deadline;
pub use deadline::MAX_DEADLINE_PAIRS;
mod edits;
mod epoch;
mod exchange;
mod lease;
mod pending;
mod source;
pub(super) use lease::duration_to_ms_u32;
use lease::{ChildCustody, Lease, wait};
pub use pending::Pending;
use source::measure_revalidation;
pub use source::{
    Cancellation, Limits, Options, Preparation, PreparationCancellation, Retirement, Source,
    StartupTimings, TrustUserCode,
};
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
    io::Read,
    os::{fd::OwnedFd, unix::net::UnixStream},
    path::Path,
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

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
    control: Option<Box<ControlClient>>,
    grouped_editor_callbacks: bool,
    offline: bool,
}
impl Prepared {
    /// Detach the dedicated out-of-band control channel for joining control
    /// custody while the real-time audio lane runs independently.
    pub fn take_control_client_on_control(&mut self) -> Option<Box<ControlClient>> {
        self.control.take()
    }

    /// Re-attach the dedicated control channel after joining the audio lane.
    pub fn restore_control_client_on_control(&mut self, client: Box<ControlClient>) {
        if client.is_faulted() {
            self.faulted = true;
        }
        self.control = Some(client);
    }

    pub fn has_pending_control_capture_on_control(&self) -> bool {
        self.control
            .as_deref()
            .is_some_and(ControlClient::has_pending_live_capture)
    }

    pub fn has_pending_control_controller_on_control(&self) -> bool {
        self.control
            .as_deref()
            .is_some_and(ControlClient::has_pending_controller_request)
    }

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
            ChildCustody::Owned(child, _) => Some(child.id()),
            _ => None,
        }
    }
    /// The PCM period reserved for this exact native child and joining
    /// transport. The permit itself stays with the waitable child.
    pub fn deadline_period_on_worker(&self) -> Option<Duration> {
        match &self.lease.child {
            ChildCustody::Owned(_, permit) => permit.as_ref().map(|permit| permit.period),
            _ => None,
        }
    }
    /// Bind an optional aligned stereo sidechain key slice for one worker
    /// exchange without allocating or mutating sticky state.
    pub fn with_sidechain_on_worker<'a>(
        &'a mut self,
        sidechain: Option<&'a [[f32; 2]]>,
    ) -> SidechainExchange<'a> {
        SidechainExchange {
            instance: self,
            sidechain,
        }
    }

    /// Only on the existing joining transport worker. A failed operation
    /// permanently faults this lease; no foreign DSP retry/replay is possible.
    pub fn exchange_on_worker(&mut self, block: Block) -> Result<Block> {
        self.exchange_on_worker_with_metadata(block, None, None, false, None, None, None)
            .map(|reply| reply.block)
    }

    /// Return the original PCM reply and its helper-side diagnostic timing.
    pub fn exchange_timed_on_worker(&mut self, block: Block) -> Result<protocol::EditorPcm> {
        self.exchange_on_worker_with_metadata(block, None, None, false, None, None, None)
    }

    /// Opt this exchange into facts about the effective editor input. This
    /// does not drain or acknowledge the ordered editor receipt reader.
    pub fn exchange_editor_on_worker(&mut self, block: Block) -> Result<protocol::EditorPcm> {
        self.exchange_on_worker_with_metadata(block, None, None, true, None, None, None)
    }
    /// Apply an eligible unlaned editor touch with the original normalized base.
    /// The caller must bind this plan to the actual canonical source.
    pub fn exchange_editor_touch_on_worker(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_editor_touch_with_sidechain(block, plan, None)
    }
    fn exchange_editor_touch_with_sidechain(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
        sidechain: Option<&[[f32; 2]]>,
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
            sidechain,
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
        self.exchange_authored_with_manual_on_worker(block, metadata, None, false, None, None)
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
        self.exchange_on_worker_with_metadata(
            block,
            Some(metadata),
            None,
            false,
            None,
            Some(dense),
            None,
        )
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
        self.exchange_armed_dense_with_sidechain(
            block,
            dense,
            manual_parameter,
            observe_editor,
            None,
        )
    }
    fn exchange_armed_dense_with_sidechain(
        &mut self,
        block: Block,
        dense: crate::transport::DenseParameterLane<'_>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        sidechain: Option<&[[f32; 2]]>,
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
                sidechain,
            )
        } else {
            Err(Error::Protocol("authored parameter source differs"))
        };
        self.authored_parameter_metadata = Some(metadata);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn exchange_authored_with_manual_on_worker(
        &mut self,
        block: Block,
        metadata: &AuthoredParameterMetadata,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
        sidechain: Option<&[[f32; 2]]>,
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
            sidechain,
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
        self.exchange_armed_with_mode(block, manual_parameter, false, None, None)
            .map(|reply| reply.block)
    }

    /// Observe editor input alongside the exact source-bound authored quantum.
    pub fn exchange_armed_authored_with_editor_on_worker(
        &mut self,
        block: Block,
        manual_parameter: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_armed_with_mode(block, manual_parameter, true, None, None)
    }

    /// Return the original authored PCM reply and its helper-side diagnostic timing.
    pub fn exchange_armed_authored_timed_on_worker(
        &mut self,
        block: Block,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_armed_with_mode(block, manual, false, None, None)
    }

    /// A scoped editor gesture may temporarily replace one authored parameter.
    /// Other authored values retain their existing per-sample SDK contract.
    pub fn exchange_armed_authored_with_editor_touch_on_worker(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.exchange_armed_authored_with_editor_touch_and_sidechain(block, plan, manual, None)
    }
    fn exchange_armed_authored_with_editor_touch_and_sidechain(
        &mut self,
        block: Block,
        plan: protocol::EditorTouchPlan,
        manual: Option<crate::transport::ParameterPoint>,
        sidechain: Option<&[[f32; 2]]>,
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
        self.exchange_armed_with_mode(
            block,
            manual,
            true,
            Some(EditorTouchMode::Single(plan)),
            sidechain,
        )
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
        self.exchange_armed_authored_with_editor_group_touch_and_sidechain(
            block, plan, manual, None,
        )
    }
    fn exchange_armed_authored_with_editor_group_touch_and_sidechain(
        &mut self,
        block: Block,
        plan: &protocol::EditorTouchPlanSet,
        manual: Option<crate::transport::ParameterPoint>,
        sidechain: Option<&[[f32; 2]]>,
    ) -> Result<protocol::EditorPcm> {
        self.validate_editor_group(&block, plan, manual)?;
        self.exchange_armed_with_mode(
            block,
            manual,
            true,
            Some(EditorTouchMode::Group(plan)),
            sidechain,
        )
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
        sidechain: Option<&[[f32; 2]]>,
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
            sidechain,
        );
        self.authored_parameter_metadata = Some(metadata);
        result
    }

    #[allow(clippy::too_many_arguments)]
    fn exchange_on_worker_with_metadata(
        &mut self,
        block: Block,
        metadata: Option<&AuthoredParameterMetadata>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
        dense: Option<crate::transport::DenseParameterLane<'_>>,
        sidechain: Option<&[[f32; 2]]>,
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
            sidechain,
        );
        if result.is_err() {
            self.faulted = true;
            self.lease.cancel_native();
        }
        result
    }
    #[allow(clippy::too_many_arguments)]
    fn exchange(
        &mut self,
        block: Block,
        metadata: Option<&AuthoredParameterMetadata>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
        editor_touch: Option<EditorTouchMode<'_>>,
        dense: Option<crate::transport::DenseParameterLane<'_>>,
        sidechain: Option<&[[f32; 2]]>,
    ) -> Result<protocol::EditorPcm> {
        self.check_live_capture_deadline()?;
        self.check_parameter_page_deadline()?;
        self.config.validate_block_fast(&block, self.sequence)?;
        if let Some(aux) = sidechain
            && (aux.len() != usize::from(block.frames)
                || !aux.iter().flatten().all(|sample| sample.is_finite()))
        {
            return Err(Error::Protocol("sidechain key samples refused"));
        }
        let next = self
            .sequence
            .checked_add(1)
            .ok_or(Error::Protocol("instance sequence exhausted"))?;
        let bytes = self.packet.request_buffer()?;
        let mut length = match (metadata, dense) {
            (Some(metadata), Some(dense)) => {
                crate::transport::wire::encode_validated_dense_authored_into_slice(
                    bytes,
                    &block,
                    dense,
                    manual_parameter,
                    &|id| metadata.step_count(id),
                )
            }
            (Some(metadata), None) => crate::transport::wire::encode_validated_authored_into_slice(
                bytes,
                &block,
                manual_parameter,
                &|id| metadata.step_count(id),
            ),
            (None, None) => crate::transport::wire::encode_validated_into_slice(bytes, &block),
            (None, Some(_)) => return Err(Error::Protocol("dense parameter metadata is missing")),
        }
        .map_err(Error::Io)?;
        if let Some(aux) = sidechain {
            let sidechain_bytes = aux.len() * 8;
            let end = length + sidechain_bytes;
            let dst = bytes
                .get_mut(length..end)
                .ok_or(Error::Protocol("sidechain packet extent exceeds its bound"))?;
            let mut at = 0;
            for frame in aux {
                dst[at..at + 4].copy_from_slice(&frame[0].to_le_bytes());
                dst[at + 4..at + 8].copy_from_slice(&frame[1].to_le_bytes());
                at += 8;
            }
            length = end;
            bytes[5] |= (protocol::SIDECHAIN_PCM_BIT >> 8) as u8;
        }
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
        self.lease.channel.send_pcm(
            &bytes[..length],
            self.lease.shm.as_deref_mut(),
            until,
            &mut || token.check(),
        )?;
        self.lease.channel.read_with_shm(
            &mut self.packet,
            self.lease.shm.as_deref_mut(),
            until,
            &mut || token.check(),
        )?;
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
        if self.parameter_page.is_some() || self.has_pending_control_controller_on_control() {
            return Err(Error::Protocol("controller page is still pending"));
        }
        if self.live_capture.is_some() || self.has_pending_control_capture_on_control() {
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
            if let Some(shm) = &self.lease.shm {
                shm.set_fault();
            }
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
/// Scoped worker-lane exchange handle carrying an optional aligned stereo
/// sidechain key slice for one PCM exchange.
pub struct SidechainExchange<'a> {
    instance: &'a mut Prepared,
    sidechain: Option<&'a [[f32; 2]]>,
}
impl SidechainExchange<'_> {
    pub fn exchange_timed_on_worker(self, block: Block) -> Result<protocol::EditorPcm> {
        self.instance.exchange_on_worker_with_metadata(
            block,
            None,
            None,
            false,
            None,
            None,
            self.sidechain,
        )
    }
    pub fn exchange_editor_on_worker(self, block: Block) -> Result<protocol::EditorPcm> {
        self.instance.exchange_on_worker_with_metadata(
            block,
            None,
            None,
            true,
            None,
            None,
            self.sidechain,
        )
    }
    pub fn exchange_editor_touch_on_worker(
        self,
        block: Block,
        plan: protocol::EditorTouchPlan,
    ) -> Result<protocol::EditorPcm> {
        self.instance
            .exchange_editor_touch_with_sidechain(block, plan, self.sidechain)
    }
    pub fn exchange_armed_dense_on_worker(
        self,
        block: Block,
        dense: crate::transport::DenseParameterLane<'_>,
        manual_parameter: Option<crate::transport::ParameterPoint>,
        observe_editor: bool,
    ) -> Result<protocol::EditorPcm> {
        self.instance.exchange_armed_dense_with_sidechain(
            block,
            dense,
            manual_parameter,
            observe_editor,
            self.sidechain,
        )
    }
    pub fn exchange_armed_authored_with_editor_on_worker(
        self,
        block: Block,
        manual_parameter: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.instance
            .exchange_armed_with_mode(block, manual_parameter, true, None, self.sidechain)
    }
    pub fn exchange_armed_authored_timed_on_worker(
        self,
        block: Block,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.instance
            .exchange_armed_with_mode(block, manual, false, None, self.sidechain)
    }
    pub fn exchange_armed_authored_with_editor_touch_on_worker(
        self,
        block: Block,
        plan: protocol::EditorTouchPlan,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.instance
            .exchange_armed_authored_with_editor_touch_and_sidechain(
                block,
                plan,
                manual,
                self.sidechain,
            )
    }
    pub fn exchange_armed_authored_with_editor_group_touch_on_worker(
        self,
        block: Block,
        plan: &protocol::EditorTouchPlanSet,
        manual: Option<crate::transport::ParameterPoint>,
    ) -> Result<protocol::EditorPcm> {
        self.instance
            .exchange_armed_authored_with_editor_group_touch_and_sidechain(
                block,
                plan,
                manual,
                self.sidechain,
            )
    }
}

#[cfg(test)]
#[path = "native/unit_tests.rs"]
mod tests;

crate::include_vst3_sdk_tests! {
    #[cfg(all(test, target_os = "macos", target_arch = "aarch64"))]
    #[path = "../tests/native_vst3_sdk/main.rs"]
    mod native_vst3_sdk;
}
