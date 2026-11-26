//! Ordinary resident preparation and final worker publication inside canonical transactions.
//! Original-bank owners, derived clip PCM and fixed factory/runtime allowances remain distinct.
//! A queue/refusal leaves authored state/history unchanged; accepted offline custody is explicit
//! and never reports a callback-applied revision. No source set or Playback is emitted to UI.
use super::audio_sink::native_sampler::NativeSamplerSources;
use super::*;
use crate::MediaBank;
use hex_engine::native_device_resources::{
    DeviceResourceAdmission, DeviceResourcePhase, PreparedDeviceResources,
};
use std::time::{Duration, Instant};

mod bounce;
pub(super) mod device_insert;
mod documents;
mod prepare;
mod publish;
mod runtime_inputs;
mod session_ops;
pub(super) use bounce::{bounce_arrangement, bounce_clip, bounce_history};
pub(super) use documents::{
    ImportPlayback, PreparedDocumentReplacement, adopt_document_snapshot, decode_relinks,
    import_file, import_frame_allowance, open_document, prepare_bounce_import,
    prepare_document_media, prepare_document_replacement, prepare_import, prepare_import_playback,
    prepare_private_export, relink_files,
};
pub(super) use publish::{execute_command, execute_command_timed, poll_pending};
use publish::{
    inherited_runtime, playback_with_loop, runtime_allowance, runtime_allowance_with_loop,
};
pub(super) use runtime_inputs::{RequiredInput, RuntimeInputs};

pub(super) struct Snapshot {
    /// Preparation origin is descriptive, not a source or installation proof.
    pub(super) source_origin: ResidentSourceOrigin,
    /// Exact context used to prepare this graph. A retained overlay based on an
    /// older editor revision may still need accounting while it is rebased.
    pub(super) prepared_context: hex_engine::AudioSourceContext,
    /// Latest submission of this exact prepared owner; never an application ACK.
    pub(super) submission: Option<ResidentGraphSubmission>,
    pub(super) prepared: Arc<PreparedRevision>,
    pub(super) resources: PreparedDeviceResources,
    pub(super) inputs: RuntimeInputs,
    pub(super) file_sources: Option<ResidentFileSet>,
}
impl Snapshot {
    pub(super) fn record_submission(
        &mut self,
        context: hex_engine::AudioSourceContext,
        acceptance: ResidentPublicationAcceptance,
    ) {
        let next = match acceptance {
            ResidentPublicationAcceptance::Submitted {
                output_identity,
                audio_revision,
            } if context == self.prepared_context
                && output_identity != 0
                && audio_revision != 0 =>
            {
                Some(ResidentGraphSubmission {
                    output_identity,
                    audio_revision,
                    context,
                })
            }
            _ => None,
        };
        if self.submission != next {
            self.submission = next;
        }
    }
    pub(super) fn include_file_sources<'a>(
        &'a self,
        view: DeviceResourceAdmission<'a>,
    ) -> DeviceResourceAdmission<'a> {
        self.file_sources
            .as_ref()
            .map_or(view, |files| files.include_requirements(view))
    }
    fn file_requirements(&self) -> DeviceResourceAdmission<'_> {
        self.include_file_sources(DeviceResourceAdmission::default())
    }
    pub(super) fn admission(&self) -> DeviceResourceAdmission<'_> {
        self.include_file_sources(DeviceResourceAdmission::prepared_on_control(
            &self.resources,
            &self.prepared,
        ))
    }
}

/// Borrowed media/waveform compilation inputs shared by resident snapshot preparation.
pub(super) struct CompileResources<'a> {
    pub(super) media: &'a MediaBank,
    pub(super) table: Arc<hex_engine::SineTable>,
    pub(super) transforms: &'a hex_engine::audio_transform::AudioTransformPreparation,
    pub(super) waveforms: &'a std::cell::RefCell<BTreeMap<hex_project::ClipId, AudioClipWaveform>>,
}

#[derive(Default)]
pub(super) struct PrepareOptions<'a> {
    pub(super) gain_recording: Option<hex_project::TrackId>,
    pub(super) tuning_source: Option<(&'a Project, bool)>,
    pub(super) offline: Option<&'a dyn super::OfflineBounceSource>,
}

/// Borrowed sink, audio context, and resource admission shared by resident transitions.
pub(super) struct ResidentSinkContext<'a> {
    pub(super) context: hex_engine::AudioSourceContext,
    pub(super) sink: &'a mut dyn WorkerAudioSink,
    pub(super) admission: sampler_export::Admission<'a>,
}

/// Publication metadata and policy paired for runtime publication calls.
#[derive(Clone, Copy)]
pub(super) struct PublicationDirective {
    pub(super) metadata: ResidentPublicationMetadata,
    pub(super) policy: ResidentPublicationPolicy,
}

/// Borrowed sink, admission, and publication directive for resident seek/input publication.
pub(super) struct ResidentPublishContext<'a> {
    pub(super) sink: &'a mut dyn WorkerAudioSink,
    pub(super) admission: sampler_export::Admission<'a>,
    pub(super) metadata: ResidentPublicationMetadata,
    pub(super) policy: ResidentPublicationPolicy,
}

/// Borrowed worker event sink and admission context for resident worker helpers.
pub(super) struct ResidentWorkerContext<'a> {
    pub(super) events: &'a mut WorkerEvents,
    pub(super) admission: sampler_export::Admission<'a>,
}

/// Document-opening parameters bundled for resident media preparation.
pub(super) struct OpenDocumentSpec<'a> {
    pub(super) history_capacity: usize,
    pub(super) source: &'a ProjectSource,
    pub(super) relinks: &'a BTreeMap<hex_project::AssetId, PathBuf>,
    pub(super) mode: DocumentAudioMode,
    pub(super) reservation: Option<SourcePlanReservation>,
}

/// Build an export-only graph from captured live plug-in state and private
/// processors. The canonical snapshot remains admitted and is never published
/// or mutated by this preparation.
fn canonical_metadata(
    snapshot: &Snapshot,
    context: hex_engine::AudioSourceContext,
) -> ResidentPublicationMetadata {
    ResidentPublicationMetadata {
        source_origin: ResidentSourceOrigin::Unknown,
        clip_preview: None,
        context,
        device_preview: None,
        tuning_preview: None,
        seek_request: None,
        stopped_restore: false,
        computer_keyboard_request: None,
        gain_recording_track: snapshot.prepared.gain_recording_track(),
        requires_preroll: snapshot.prepared.arrangement_seek_requires_preroll(),
    }
}
pub(super) struct Inputs<'a> {
    pub(super) native_parameter_input: Option<hex_engine::NativeParameterInputTarget>,
    pub(super) native: NativeSamplerSources<'a>,
    pub(super) limits: crate::ResidentNativeReservation,
    max_frames: u64,
    pub(super) shutdown: &'a AtomicBool,
    pub(super) cancel: Option<&'a AtomicBool>,
    pub(super) plugin_effect: Option<&'a PluginEffectControl>,
    pub(super) device_insert_control: Option<&'a PluginEffectControl>,
    pub(super) restoring_plugin_state: bool,
    pub(super) preparing_edit: bool,
    pub(super) reload_plugin_device: Option<u64>,
    pub(super) power_on: Option<ResidentPowerOn>,
    pub(super) running_power_off: Option<ResidentRunningPowerOff>,
    pub(super) hosted_move: Option<audio_sink::ResidentHostedMove>,
    pub(super) stopped_history: bool,
    pub(super) admission: sampler_export::Admission<'a>,
    pub(super) current: Option<&'a Snapshot>,
    pub(super) predecessor: Option<&'a Session>,
    legacy: Vec<&'a hex_engine::Audio>,
}
pub(super) enum PluginParameterCommit {
    Captured {
        device: hex_project::DeviceId,
        capture: Box<PluginParameterCapture>,
    },
    Override(Box<Command>),
}

enum Transition {
    Command(Box<Command>),
    DeviceAutomationTake {
        command: Box<Command>,
        target: super::native_automation_recording::TakePublication,
    },
    PluginParameter(PluginParameterCommit),
    PluginEffect {
        command: Box<Command>,
        control: Arc<PluginEffectControl>,
    },
    History {
        redo: bool,
    },
    MidiCommand(Box<Command>),
    SectionCommand {
        command: Box<Command>,
        cursor: Option<hex_project::BeatTime>,
        section_loop: Option<(hex_project::BeatTime, hex_project::BeatTime)>,
    },
    SectionHistory {
        redo: bool,
        cursor: Option<hex_project::BeatTime>,
        section_loop: Option<(hex_project::BeatTime, hex_project::BeatTime)>,
    },
    BounceHistory {
        redo: bool,
        position: ArrangementEditPosition,
    },
}

#[cfg(test)]
pub(super) mod tests;

fn project_has_vst3_devices(project: &Project) -> bool {
    let is_vst3 = |device: &hex_project::Device| {
        device.state().type_id() == hex_lattice_contract::VST3_DEVICE_TYPE_ID
    };
    project.main_devices().iter().any(is_vst3)
        || project
            .return_tracks()
            .iter()
            .any(|bus| bus.devices().iter().any(is_vst3))
        || project.tracks().iter().any(|track| {
            track.instrument().is_some_and(is_vst3) || track.devices().iter().any(is_vst3)
        })
}
