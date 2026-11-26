//! Effect-preparation, plugin-parameter, and resident-publication contract
//! types shared across `WorkerAudioSink`.
use super::super::*;
use std::time::Instant;

/// Synchronous read-only identities in the exact candidate project. Native
/// state validation stays in the borrowed compiler; consumers receive neither
/// project editing access nor a registry or native owner.
pub struct ResidentEffectPreparation<'a> {
    pub(in crate::service) project: &'a Project,
    pub(in crate::service) context: hex_engine::AudioSourceContext,
    pub(in crate::service) restoring_plugin_state: bool,
    pub(in crate::service) preparing_edit: bool,
    pub(in crate::service) reload_plugin_device: Option<u64>,
    pub(in crate::service) power_on: Option<ResidentPowerOn>,
    pub(in crate::service) running_power_off: Option<ResidentRunningPowerOff>,
    pub(in crate::service) hosted_move: Option<ResidentHostedMove>,
}

/// Copied original source/binding for one explicit running hosted Off command.
/// This is not custody; only Session can attach it to compiler preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentRunningPowerOff {
    pub owner: hex_engine::ProcessorChainOwner,
    pub device: u64,
    pub source: ResidentGraphSubmission,
    pub binding: hex_lattice_contract::ForeignProcessorBinding,
}

/// The exact canonical receiver of a staged Resident command, with the
/// original installed source stamp. This grants no native source custody.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentHostedInstrumentMove {
    pub donor: hex_project::TrackId,
    pub receiver: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub source: hex_project::SourceId,
    pub predecessor: ResidentRuntimeLoop,
}

/// An exact running direct Track effect move staged from the installed source.
/// Desktop must prove the same physical worker before using this observation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentHostedTrackEffectMove {
    pub donor: hex_project::TrackId,
    pub receiver: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub predecessor: ResidentRuntimeLoop,
}

/// The authored endpoint of an exact direct reorder and its running predecessor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentHostedTrackEffectReorder {
    pub track: hex_project::TrackId,
    pub device: hex_project::DeviceId,
    pub predecessor: ResidentRuntimeLoop,
}

#[derive(Clone, Copy)]
pub(in crate::service) enum ResidentHostedMove {
    Instrument(ResidentHostedInstrumentMove),
    TrackEffect(ResidentHostedTrackEffectMove),
    Reorder(ResidentHostedTrackEffectReorder),
}

/// The explicit PowerDevice command's nonowning Off predecessor.
/// Desktop revalidates the observed interval before fresh SDK preparation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentPowerOn {
    pub(in crate::service) device: u64,
    pub(in crate::service) interval: ResidentPowerOnInterval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentPowerOnInterval {
    Stopped {
        owner: hex_engine::ProcessorChainOwner,
        context: hex_engine::AudioSourceContext,
        fence: ResidentFreshRuntimeFence,
    },
    Running {
        owner: hex_engine::ProcessorChainOwner,
        source: ResidentGraphSubmission,
    },
}
impl ResidentPowerOn {
    pub fn device(&self) -> u64 {
        self.device
    }
    pub fn owner(&self) -> hex_engine::ProcessorChainOwner {
        match self.interval {
            ResidentPowerOnInterval::Stopped { owner, .. }
            | ResidentPowerOnInterval::Running { owner, .. } => owner,
        }
    }
    pub fn source_context(&self) -> hex_engine::AudioSourceContext {
        match self.interval {
            ResidentPowerOnInterval::Stopped { context, .. } => context,
            ResidentPowerOnInterval::Running { source, .. } => source.context,
        }
    }
    pub fn interval(&self) -> ResidentPowerOnInterval {
        self.interval
    }
}

/// One ordered plugin gesture request and its exact canonical baseline. This
/// is borrowed DATA, never native source/endpoint custody or SDK admission.
pub struct PluginParameterGestureInput<'a> {
    pub(in crate::service) context: hex_engine::AudioSourceContext,
    pub(in crate::service) operation: crate::device_gesture::PluginGesturePreparation,
    pub(in crate::service) canonical_state: &'a NativeDeviceState,
    pub(in crate::service) reading_class: Option<[u8; 16]>,
    pub(in crate::service) isolated_class: Option<[u8; 16]>,
    pub(in crate::service) instrument: bool,
    pub(in crate::service) confirm_delivery: bool,
    pub(in crate::service) record_touch: bool,
}
impl<'a> PluginParameterGestureInput<'a> {
    /// Recorded touches require a playing reading lane and return admission
    /// before input, then exclusive restoration instead of an opaque edit.
    pub fn with_touch_recording(mut self) -> Self {
        self.confirm_delivery = true;
        self.record_touch = true;
        self
    }
    pub fn records_touch(&self) -> bool {
        self.record_touch
    }

    /// Request actual final-output point receipts. This copied request grants
    /// no source/SDK authority; the adapter must admit its exact loaded worker.
    pub fn with_delivery_confirmation(mut self) -> Self {
        self.confirm_delivery = true;
        self
    }
    pub fn requires_delivery_confirmation(&self) -> bool {
        self.confirm_delivery
    }

    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    pub fn operation(&self) -> crate::device_gesture::PluginGesturePreparation {
        self.operation
    }
    /// Exact class of the reading Arrangement lane, for the ordinary processor.
    pub fn reading_class(&self) -> Option<[u8; 16]> {
        self.reading_class
    }
    /// A retained lane on this exact processor requires an isolated manual base,
    /// including when the edited parameter has no lane of its own.
    pub fn isolated_class(&self) -> Option<[u8; 16]> {
        self.isolated_class
    }
    /// Canonical slot role only; the host validates its actual SDK profile.
    pub fn is_instrument(&self) -> bool {
        self.instrument
    }
    pub fn canonical_state(&self) -> &'a NativeDeviceState {
        self.canonical_state
    }
}
/// Canonical target selected for one original editor take. Desktop must bind
/// these copied facts to the exact playing worker and its frozen base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PluginEditorRecordingInput {
    pub key: crate::device_gesture::GestureKey,
    pub context: hex_engine::AudioSourceContext,
    pub target: super::PluginEditorRecordingTarget,
}

/// Control-side observation copied only after matching the complete private
/// final-output receipt. Persist `application.origin`, not the later delivery
/// processing frame. A caller requesting these records owns their retention.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PluginParameterDelivery {
    pub key: crate::device_gesture::GestureKey,
    pub context: hex_engine::AudioSourceContext,
    pub target: super::automation_draft::Vst3CaptureTarget,
    pub source: super::DeviceAutomationSource,
    pub sequence: u64,
    pub application: super::automation_draft::Vst3ParameterApplication,
    pub delivery_performance_frame: u64,
}
/// A confirmed point or a conservative interruption with no invented clock.
#[derive(Clone, Copy, Debug, PartialEq)]
#[allow(clippy::large_enum_variant)] // A whole physical group remains one control observation.
pub enum PluginParameterObservation {
    Grouped(super::Vst3EditorGroupObservation),
    Ready {
        key: crate::device_gesture::GestureKey,
        context: hex_engine::AudioSourceContext,
        target: super::automation_draft::Vst3CaptureTarget,
        source: super::DeviceAutomationSource,
    },
    Delivered(PluginParameterDelivery),
    Restored(PluginParameterDelivery),
    /// No point was admitted; no audio clock is invented for this no-op.
    Unchanged {
        key: crate::device_gesture::GestureKey,
    },
    Interrupted {
        key: crate::device_gesture::GestureKey,
        context: hex_engine::AudioSourceContext,
        target: super::automation_draft::Vst3CaptureTarget,
        source: super::DeviceAutomationSource,
        sequence: u64,
    },
}
pub enum PluginParameterGestureResult {
    /// Validated Begin or completed Preview/Restore, not physical delivery or
    /// an assertion that a plugin returned the exact requested normalized value.
    /// SDK readback remains separate from the client's lossless intent.
    Applied,
    /// Actual host capture. The host retains its private matching receipt and
    /// staged compiler permission until finish_plugin_parameter_commit.
    Captured(Box<PluginParameterCapture>),
    /// A manual base prepared from the exact canonical plug-in state in
    /// isolation from the live authored processor. This does not authorize
    /// changing another parameter's read selection or using live DSP state
    /// as the Undo preimage.
    OverrideCaptured(Box<PluginParameterOverride>),
}
pub struct PluginParameterOverride {
    pub source: hex_engine::AudioSourceContext,
    pub class: [u8; 16],
    pub parameter: u32,
    pub base: NativeDeviceState,
}
pub struct PluginParameterCapture {
    pub before: NativeDeviceState,
    pub after: NativeDeviceState,
}
#[derive(Debug)]
pub enum PluginParameterGestureError {
    Refused(String),
    SourceLost(String),
}
impl<'a> ResidentEffectPreparation<'a> {
    fn sole_played_main(&self) -> Option<&'a hex_project::Device> {
        let [device] = self.project.main_devices() else {
            return None;
        };
        if !is_plugin_effect(device)
            || !device.automation().is_empty()
            || !device.modulation().sources().is_empty()
            || !device.modulation().routes().is_empty()
        {
            return None;
        }
        let mut played = playback_plugin_devices(self.project);
        let (destination, actual) = played.next()?;
        (played.next().is_none()
            && destination.owner() == hex_engine::ProcessorChainOwner::Main
            && actual.id() == device.id())
        .then_some(device)
    }
    /// One saved Off Main on a playback route, without automation or modulation.
    /// This declaration grants no worker, endpoint or applied-source authority.
    pub fn powered_off_main(&self) -> Option<(u64, &'a NativeDeviceState)> {
        let device = self.sole_played_main().filter(|device| device.bypassed())?;
        Some((device.id().value(), device.state()))
    }
    /// The sole hosted Return authored Off on a playback route. This
    /// declaration grants no preparation or installation authority.
    pub fn powered_off_return(
        &self,
    ) -> Option<(hex_engine::ProcessorChainOwner, u64, &'a NativeDeviceState)> {
        let (owner, device) = sole_hosted_return(self.project)?;
        device
            .bypassed()
            .then_some((owner, device.id().value(), device.state()))
    }
    /// Only a staged running PowerDevice may replace the exact On source by Off.
    pub fn running_power_off(&self) -> Option<ResidentRunningPowerOff> {
        let intent = self.running_power_off?;
        let device = match intent.owner {
            hex_engine::ProcessorChainOwner::Main => self.sole_played_main()?,
            hex_engine::ProcessorChainOwner::Return(_) => {
                let (owner, device) = sole_hosted_return(self.project)?;
                if owner != intent.owner {
                    return None;
                }
                device
            }
            _ => return None,
        };
        (self.preparing_edit
            && device.bypassed()
            && device.id().value() == intent.device
            && intent.source.context.project_generation == self.context.project_generation
            && intent.source.context.editor_revision.checked_add(1)
                == Some(self.context.editor_revision))
        .then_some(intent)
    }
    /// Only the explicit power command can prepare a fresh On worker.
    /// Generic history/moves and changed owner, bypass or context cannot use it.
    pub fn power_on(&self) -> Option<ResidentPowerOn> {
        let intent = self.power_on?;
        let device = match intent.owner() {
            hex_engine::ProcessorChainOwner::Main => self.sole_played_main()?,
            hex_engine::ProcessorChainOwner::Return(_) => {
                let (owner, device) = sole_hosted_return(self.project)?;
                if owner != intent.owner() {
                    return None;
                }
                device
            }
            _ => return None,
        };
        let source = intent.source_context();
        (self.preparing_edit
            && !device.bypassed()
            && device.id().value() == intent.device
            && source.project_generation == self.context.project_generation
            && source.editor_revision.checked_add(1) == Some(self.context.editor_revision))
        .then_some(intent)
    }
    pub fn hosted_instrument_move(&self) -> Option<&ResidentHostedInstrumentMove> {
        match self.hosted_move.as_ref()? {
            ResidentHostedMove::Instrument(route) => Some(route),
            ResidentHostedMove::TrackEffect(_) | ResidentHostedMove::Reorder(_) => None,
        }
    }
    pub fn hosted_track_effect_move(&self) -> Option<&ResidentHostedTrackEffectMove> {
        match self.hosted_move.as_ref()? {
            ResidentHostedMove::Instrument(_) | ResidentHostedMove::Reorder(_) => None,
            ResidentHostedMove::TrackEffect(route) => Some(route),
        }
    }
    pub fn hosted_track_effect_reorder(&self) -> Option<&ResidentHostedTrackEffectReorder> {
        match self.hosted_move.as_ref()? {
            ResidentHostedMove::Reorder(route) => Some(route),
            ResidentHostedMove::Instrument(_) | ResidentHostedMove::TrackEffect(_) => None,
        }
    }
    pub fn reload_plugin_device(&self) -> Option<u64> {
        self.preparing_edit.then_some(self.reload_plugin_device?)
    }
    pub fn sample_rate(&self) -> u32 {
        self.project.sample_rate()
    }
    pub fn context(&self) -> hex_engine::AudioSourceContext {
        self.context
    }
    /// Only the canonical Editor's prepared transition enables native source
    /// changes. Preview, seek and document-restoration queries cannot do so.
    pub fn preparing_edit(&self) -> bool {
        self.preparing_edit
    }
    /// Only the unchanged canonical fresh-output continuation sets this bit.
    /// Ordinary edits and transient audition requests cannot consume its sources.
    pub fn restoring_plugin_state(&self) -> bool {
        self.restoring_plugin_state
    }
    /// Canonical bypass state of a plug-in on a playback route. An unbound
    /// instrument can retain authored state without having a playback worker.
    /// This observation does not admit a native source.
    pub fn plugin_device_bypassed(&self, id: u64) -> Option<bool> {
        playback_plugin_devices(self.project)
            .map(|(_, device)| device)
            .find(|device| device.id().value() == id)
            .map(|device| device.bypassed())
    }
    /// Whether the saved document needs the fixed-quantum plugin output profile.
    pub fn requires_plugin_profile(&self) -> bool {
        requires_plugin_profile(self.project)
    }
    /// Borrow all saved raw targets, including lanes currently set to Base. The
    /// native source owner still validates the class and exact SDK capabilities.
    pub fn authored_plugin_targets(
        &self,
        device: u64,
    ) -> impl Iterator<Item = (u32, Option<[u8; 16]>)> + '_ {
        self.project
            .automation_devices()
            .find(|candidate| candidate.id().value() == device && is_plugin_effect(candidate))
            .into_iter()
            .flat_map(|device| device.automation().iter())
            .filter_map(|lane| match lane.target {
                hex_lattice_contract::DeviceAutomationTarget::Vst3 { parameter } => {
                    Some((parameter, lane.vst3_class))
                }
                _ => None,
            })
    }

    /// Canonical raw target for this effect's requested dense reservation.
    /// The host independently validates the actual original source metadata.
    pub fn modulated_plugin_target(&self, device: u64) -> Result<Option<(u32, [u8; 16])>, String> {
        let Some(candidate) = self
            .project
            .automation_devices()
            .find(|candidate| candidate.id().value() == device && is_plugin_effect(candidate))
        else {
            return Ok(None);
        };
        let selected = hex_engine::project_vst3_modulation_target(candidate)
            .map_err(|error| error.to_string())?;
        if selected.is_some()
            && !self
                .project
                .device_location(candidate.id())
                .is_some_and(|location| {
                    location.class == hex_project::DeviceUnitClass::Effect
                        && !matches!(location.owner, hex_project::DeviceOwner::Source(_))
                })
        {
            return Err("vst3 modulation requires a track, return or main effect".into());
        }
        Ok(selected)
    }

    /// Whether the authored program can stay on its original native worker.
    /// Effect loop activation and deactivation use the prepared origin budget
    /// for delayed audio and partial worker quanta. Instruments also need to
    /// preserve their transport and MIDI gate continuity.
    pub fn same_authored_plugin_program(
        &self,
        previous: &Self,
        device: u64,
        quantum_frames: Option<u16>,
    ) -> bool {
        let mut current = authored_plugin_lanes(self.project, device).peekable();
        let mut old = authored_plugin_lanes(previous.project, device).peekable();
        if current.peek().is_none() && old.peek().is_none() {
            return true;
        }
        let next_loop = self.project.loop_region();
        let previous_loop = previous.project.loop_region();
        let same_loop_budget = next_loop == previous_loop || {
            let instrument = self
                .project
                .native_sources()
                .iter()
                .any(|source| source.instrument().id().value() == device);
            if instrument {
                quantum_frames.is_some_and(|frames| {
                    hex_engine::foreign_automation::QuantumProfile::keeps_instrument_epoch(
                        frames,
                        previous_loop,
                        next_loop,
                    )
                })
            } else {
                hex_engine::foreign_automation::QuantumProfile::keeps_effect_epoch(
                    quantum_frames,
                    previous_loop,
                    next_loop,
                )
            }
        };
        self.project.tempo_map() == previous.project.tempo_map()
            && same_loop_budget
            && current.eq(old)
    }

    /// A pure recompile of an authored effect's time coordinates can
    /// retain the original processor. Runtime admission still proves the
    /// compatible graph and exact delayed-clock history at the stopped/live cut.
    pub fn retimes_existing_authored_effect(&self, previous: &Self, device: u64) -> bool {
        let mut current = authored_plugin_lanes(self.project, device).peekable();
        !self
            .project
            .tempo_map()
            .proves_same_clock(previous.project.tempo_map())
            && current.peek().is_some()
            && current.eq(authored_plugin_lanes(previous.project, device))
    }

    pub fn visit_plugin_devices(
        &self,
        mut visit: impl FnMut(PluginEffectDestination, u64, &'a NativeDeviceState) -> Result<(), String>,
    ) -> Result<(), String> {
        for (destination, device) in playback_plugin_devices(self.project) {
            visit(destination, device.id().value(), device.state())?;
        }
        Ok(())
    }
}

/// One private native bounce owner. It supplies exact compile facts before
/// rendering and is dropped before Session adopts any of the output media.
pub trait OfflineBounceSource: hex_export::ForeignBounceRenderer + Send {
    fn retained_data_bytes(&self) -> usize;
    /// Finalize this private source against the one compiled graph before any
    /// render or publication. Failure aborts the export/bounce without adoption.
    fn finalize_on_control(
        &mut self,
        revision: &hex_engine::PreparedRevision,
        control: &hex_export::ExportControl,
    ) -> Result<(), String>;
    fn prepare(
        &self,
        compile: &mut dyn FnMut(
            &hex_engine::AudioCandidateEffects<'_, '_>,
        )
            -> Result<hex_engine::PreparedRevision, hex_engine::Invalid>,
    ) -> Result<hex_engine::PreparedRevision, String>;
}

fn authored_plugin_lanes(
    project: &Project,
    device: u64,
) -> impl Iterator<Item = &hex_project::DeviceAutomation> {
    project
        .automation_devices()
        .find(|candidate| candidate.id().value() == device && is_plugin_effect(candidate))
        .into_iter()
        .flat_map(hex_project::Device::reading_automation)
        .filter(|lane| {
            matches!(
                lane.target,
                hex_lattice_contract::DeviceAutomationTarget::Vst3 { .. }
            )
        })
}

fn is_plugin_effect(device: &hex_project::Device) -> bool {
    device.state().type_id() == hex_lattice_contract::DeviceTypeId::from_static("hex.vst3")
}

pub(in crate::service) fn sole_hosted_return(
    project: &Project,
) -> Option<(hex_engine::ProcessorChainOwner, &hex_project::Device)> {
    let [bus] = project.return_tracks() else {
        return None;
    };
    let [device] = bus.devices() else { return None };
    let mut played = playback_plugin_devices(project);
    let (destination, actual) = played.next()?;
    let owner = hex_engine::ProcessorChainOwner::Return(bus.id().value());
    (destination.owner() == owner
        && actual.id() == device.id()
        && played.next().is_none()
        && device.automation().is_empty()
        && device.modulation().sources().is_empty()
        && device.modulation().routes().is_empty())
    .then_some((owner, device))
}

fn playback_plugin_devices(
    project: &Project,
) -> impl Iterator<Item = (PluginEffectDestination, &hex_project::Device)> {
    project
        .tracks()
        .iter()
        .flat_map(|track| {
            track
                .instrument()
                .filter(|_| track.receives_source_audio())
                .map(|device| (PluginEffectDestination::Instrument(track.id()), device))
                .into_iter()
                .chain(
                    track
                        .devices()
                        .iter()
                        .map(|device| (PluginEffectDestination::Track(track.id()), device)),
                )
        })
        .chain(project.return_tracks().iter().flat_map(|bus| {
            bus.devices()
                .iter()
                .map(|device| (PluginEffectDestination::Return(bus.id()), device))
        }))
        .chain(
            project
                .main_devices()
                .iter()
                .map(|device| (PluginEffectDestination::Main, device)),
        )
        .filter(|(_, device)| is_plugin_effect(device))
}

pub(in crate::service) fn requires_plugin_profile(project: &Project) -> bool {
    playback_plugin_devices(project).next().is_some()
}

/// Reported preparation origin, not proof of canonical state or source custody.
/// A canonical candidate may be prepared and applied before its edit commits;
/// unknown also covers preparations whose context was subsequently rebased.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentSourceOrigin {
    Unknown,
    CanonicalCandidate,
    DevicePreview,
    TuningPreview,
    ClipMovePreview,
    ClipStretchPreview,
    ClipPlayStartPreview,
    ClipGainPreview,
}

/// Scalar identity for one worker-private ordinary resident graph. Neither preview correlation
/// nor a requested freshness fence grants source custody or acknowledges callback application.
#[derive(Clone, Copy, Debug)]
pub struct ResidentPublicationMetadata {
    /// Producer-reported origin; this is not an admission or source proof.
    pub source_origin: ResidentSourceOrigin,
    pub context: hex_engine::AudioSourceContext,
    /// Identity of a clip-move preview, not proof of callback application.
    pub clip_preview: Option<ClipMovePreviewIdentity>,
    pub device_preview: Option<DevicePreviewIdentity>,
    pub tuning_preview: Option<TuningPreviewIdentity>,
    pub seek_request: Option<u64>,
    pub stopped_restore: bool,
    pub computer_keyboard_request: Option<u64>,
    pub gain_recording_track: Option<hex_project::TrackId>,
    pub requires_preroll: bool,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentFreshRuntimeFence {
    pub output_identity: u64,
    pub active_audio_revision: u64,
    pub route_generation: u64,
    pub admitted_command_ticket: u64,
    /// Last output-fault epoch observed before the seek was prepared.
    pub output_fault_epoch: u64,
    /// Callback-observed priority Stop generation of the captured audio fence.
    pub consumed_stop_generation: u64,
}
/// Nonowning scalar witness for the actual installed graph's audition loop.
/// No PCM, prepared graph, lease or callback runtime is borrowed or transferred.
/// `source_context == None` describes a cold output at audio revision zero.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentRuntimeLoop {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub source_context: Option<hex_engine::AudioSourceContext>,
    /// Origin reported for this exact submission, not authority to use its source.
    pub source_origin: ResidentSourceOrigin,
    pub section_loop: Option<hex_project::LoopRegion>,
}
/// Read-only, output-bound observation of the one admitted resident graph.
/// This is not a publication receipt, lease, or permission to mutate a graph.
/// `publication_in_flight` is a snapshot of the publication reservation, not
/// an admission guarantee: other owners and subsequent races can still refuse.
/// Applied or refused revisions can be visible before that reservation clears.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentPublicationReadiness {
    NoOutput,
    Idle {
        output_identity: u64,
        applied_revision: u64,
        publication_in_flight: bool,
    },
    Pending {
        submission: ResidentRuntimeLoop,
        applied_revision: u64,
        publication_in_flight: bool,
    },
    Applied {
        submission: ResidentRuntimeLoop,
        publication_in_flight: bool,
    },
    Refused {
        submission: ResidentRuntimeLoop,
        applied_revision: u64,
        publication_in_flight: bool,
    },
}

/// One callback-observed source and transport witness under the arrangement gate.
#[derive(Clone, Copy, Debug)]
pub struct ArrangementEditPosition {
    pub frame: u64,
    pub playing: bool,
    pub section_loop: Option<hex_project::LoopRegion>,
}
impl ArrangementEditPosition {
    /// Validate control-side scalar observations. This grants no fence or resource custody.
    pub fn from_frames(
        frame: u64,
        playing: bool,
        section_loop: Option<(u64, u64)>,
    ) -> Result<Self, String> {
        let section_loop = section_loop
            .map(|(start, end)| {
                hex_project::LoopRegion::new(start, end, true).map_err(|e| e.to_string())
            })
            .transpose()?;
        Ok(Self {
            frame,
            playing,
            section_loop,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationPolicy {
    /// Offline inheritance or an output without a transient audition loop.
    InheritRuntime,
    /// Preserve this exact applied graph's loop through ordinary inheritance.
    /// The native owner revalidates it before admission/enqueue; it grants no custody.
    InheritRuntimeAt(ResidentRuntimeLoop),
    /// Install a staged device candidate against the exact current graph.
    /// This policy requires a cancellable callback receipt and live output.
    DeviceInsert {
        predecessor: ResidentRuntimeLoop,
        hosted: bool,
        /// A stopped-only edit must retain this original settled Stop interval.
        stopped_fence: Option<ResidentFreshRuntimeFence>,
        /// Exact seamless refusal whose interruption the user approved.
        confirmation: Option<ResidentGraphSubmission>,
        /// This attempt expires at the callback, even if preparation queued in time.
        preparation_deadline: Instant,
    },
    /// The sink owns the shared arrangement gate through exact callback installation.
    /// A playing witness inherits the actual boundary runtime; a stopped witness
    /// installs its prepared musical cursor.
    ArrangementEdit,
    /// Existing explicit fresh installation used after priority Stop restoration.
    InstallFreshRuntime,
    /// Only the same-context fresh-output plugin continuation may replace its
    /// admitted unpublished graph after actual native/endpoint readiness.
    RestorePluginState,
    /// New/Open only: admit the exact successor before canonical replacement.
    /// Matching rates may retain the graph for the existing deferred reopen.
    /// A rate mismatch requires a fresh output and graph installation before adoption.
    ReplaceDocument {
        requires_plugin_profile: bool,
    },
    FreshIfUnchanged(ResidentFreshRuntimeFence),
    QuantizedSeekWhenReady {
        fence: ResidentFreshRuntimeFence,
        quantization: hex_engine::LaunchQuantization,
    },
}
/// Successful worker acceptance is either an actual queue submission or explicit offline custody.
/// Only the callback's exact output/revision observation establishes application.
#[derive(Clone, Copy, Debug)]
pub enum ResidentPublicationAcceptance {
    Submitted {
        output_identity: u64,
        audio_revision: u64,
    },
    OfflineRetained,
}
/// A control-side receipt for one exact prepared graph submission. It does not
/// acknowledge callback application, retain a graph, or grant a source fence.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResidentGraphSubmission {
    pub output_identity: u64,
    pub audio_revision: u64,
    pub context: hex_engine::AudioSourceContext,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceInsertAudioState {
    Pending,
    Applying,
    Installed,
    Cancelled,
    Expired,
    Refused(hex_engine::PlaybackHandoffRejection),
    OutputClosed,
}

/// The worker retains this exact receipt after enqueue. Cancellation is
/// nonblocking, and the terminal remains readable across output replacement.
/// The receipt owns the admission deadline even while the worker is busy.
pub trait DeviceInsertAudioReceipt: Send + std::fmt::Debug {
    fn submission(&self) -> ResidentGraphSubmission;
    fn state(&self) -> DeviceInsertAudioState;
    fn cancel(&self) -> bool;
}
/// Bind a control preparation to the client's observed resident graph and
/// stopped transport. The output callback checks these facts again at activation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PausedInputMonitorRequest {
    pub source: ResidentGraphSubmission,
    pub transport_epoch: u64,
    pub track: hex_project::TrackId,
}
/// Exact resident graph and reservation inputs for one automatic note-monitor
/// poll. Only the output worker can observe or advance the installed pause.
pub struct PausedNoteGraph<'a> {
    pub source: ResidentGraphSubmission,
    pub revision: &'a Arc<PreparedRevision>,
    pub resources: &'a hex_engine::native_device_resources::PreparedDeviceResources,
    pub cancellation: &'a super::RequestCancellation,
}
impl ResidentGraphSubmission {
    pub(in crate::service) fn matches_runtime_loop(self, submission: ResidentRuntimeLoop) -> bool {
        submission.output_identity == self.output_identity
            && submission.audio_revision == self.audio_revision
            && submission.source_context == Some(self.context)
    }

    /// Compare with the callback's correlated publication observation. A pending
    /// successor is not a stable source for a stopped edit. The caller must still
    /// establish and hold the exact fence before using this as a predecessor.
    pub fn matches_applied(self, readiness: ResidentPublicationReadiness) -> bool {
        matches!(readiness, ResidentPublicationReadiness::Applied {
            submission, publication_in_flight: false
        } if self.matches_runtime_loop(submission))
    }

    /// Compare with the sink's latest publication observation to see whether
    /// `self` is installed (including after a later candidate was refused) or
    /// still in flight awaiting installation. The sink must still verify exact
    /// stopped installation and hold the fence before pruning foreign history.
    pub fn matches_stopped_history_predecessor(
        self,
        readiness: ResidentPublicationReadiness,
    ) -> bool {
        match readiness {
            ResidentPublicationReadiness::Applied { submission, .. }
            | ResidentPublicationReadiness::Pending { submission, .. } => {
                self.matches_runtime_loop(submission)
            }
            ResidentPublicationReadiness::Refused {
                submission:
                    ResidentRuntimeLoop {
                        output_identity, ..
                    },
                applied_revision,
                ..
            } => output_identity == self.output_identity && applied_revision == self.audio_revision,
            ResidentPublicationReadiness::NoOutput | ResidentPublicationReadiness::Idle { .. } => {
                false
            }
        }
    }
}

/// Borrow the compiled clock associated with the exact outgoing graph.
/// The sink observes its running transport before selecting a cut.
#[derive(Clone, Copy)]
pub struct ResidentTempoEdit<'a> {
    pub source: ResidentGraphSubmission,
    pub previous: &'a Arc<PreparedRevision>,
    /// Exact missed publication whose predecessor fence must still hold.
    pub retry_of: Option<ResidentGraphSubmission>,
}

/// A missed cut may be retried on the original running source.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentTempoEditOutcome {
    Confirmed,
    MissedCut,
}

/// Optional inputs for the ordinary prepared-transition publisher.
#[derive(Default)]
pub struct ResidentPublicationInputs<'a> {
    pub files: Option<ResidentFilePublication<'a>>,
    pub tempo_edit: Option<ResidentTempoEdit<'a>>,
    /// Control-only built-in insertion intent, bound before callback admission.
    pub device_insert_control: Option<&'a super::PluginEffectControl>,
}
/// Reason category for a refused resident publication. The unavailable
/// category describes source custody only; it does not imply a deadline fault.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResidentPublicationFailure {
    NativeSourceUnavailable,
    Other,
}

pub struct ResidentPublicationRefusal {
    pub kind: ResidentPublicationFailure,
    pub playback: Playback,
    pub error: String,
}
impl std::fmt::Debug for ResidentPublicationRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResidentPublicationRefusal")
            .field("kind", &self.kind)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}
