//! Captured sampler authoring intents for ordinary worker transactions and the explicit private
//! CURRENT path. Only the latter uses a stopped callback fence; Session owns canonical/history commit.
use super::*;

fn parameter_device<'a>(
    project: &'a hex_project::Project,
    edit: &SamplerParameterEdit,
    allow_bypassed: bool,
) -> Option<&'a hex_project::Device> {
    let device = match edit.owner {
        SamplerParameterOwner::Track(track) => project.track(track)?.instrument()?,
        SamplerParameterOwner::Source(source) => {
            if project
                .tracks()
                .iter()
                .any(|track| track.source_id() == Some(source))
            {
                return None;
            }
            project.native_source(source)?.instrument()
        }
    };
    (device.id() == edit.device && (allow_bypassed || !device.bypassed())).then_some(device)
}
pub(super) fn validate(
    session: &Session,
    generation: u64,
    revision: u64,
    edit: &SamplerParameterEdit,
) -> Result<(), String> {
    if edit.request_id == 0
        || edit.project_generation != generation
        || edit.revision != revision
        || edit.cancel.load(Ordering::Acquire)
    {
        return Err("sampler edit target changed or edit was cancelled".into());
    }
    let device = parameter_device(session.project(), edit, false)
        .ok_or("sampler edit target is unavailable")?;
    hex_engine::patch_sampler_parameter_on_control(edit.parameter, device.state(), edit.value)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The sole pending native edit: one fence, serial and final completion.
#[derive(Debug)]
pub enum SamplerEdit {
    Parameter(SamplerParameterEdit),
    Midi(SamplerMidiEdit),
    History(SamplerHistoryEdit),
    Replacement(SamplerReplacementEdit),
}
impl SamplerEdit {
    pub(super) fn request_id(&self) -> u64 {
        match self {
            Self::Parameter(e) => e.request_id,
            Self::Midi(e) => e.request_id,
            Self::History(e) => e.request_id,
            Self::Replacement(e) => e.request_id,
        }
    }
    pub(super) fn cancel(&self) -> &AtomicBool {
        match self {
            Self::Parameter(e) => &e.cancel,
            Self::Midi(e) => &e.cancel,
            Self::History(e) => &e.cancel,
            Self::Replacement(e) => &e.cancel,
        }
    }
    pub(super) fn validate(
        &mut self,
        session: &Session,
        generation: u64,
        revision: u64,
    ) -> Result<bool, String> {
        match self {
            Self::Midi(edit) => {
                if edit.request_id == 0
                    || edit.project_generation != generation
                    || edit.revision != revision
                    || edit.cancel.load(Ordering::Acquire)
                {
                    return Err("sampler midi target changed or edit was cancelled".into());
                }
                edit.command
                    .prepare_sampler_midi_edit(session.project())
                    .map_err(|e| e.to_string())
            }
            Self::Parameter(edit) => {
                validate(session, generation, revision, edit)?;
                let state = session
                    .project()
                    .device(edit.device)
                    .ok_or("sampler target is unavailable")?
                    .state();
                Ok(
                    hex_engine::sampler_parameter_value_on_control(edit.parameter, state)
                        .map_err(|e| e.to_string())?
                        != edit.value
                        && hex_engine::patch_sampler_parameter_on_control(
                            edit.parameter,
                            state,
                            edit.value,
                        )
                        .map_err(|e| e.to_string())?
                            != *state,
                )
            }
            Self::Replacement(edit) => {
                if edit.request_id == 0
                    || edit.project_generation != generation
                    || edit.revision != revision
                    || edit.cancel.load(Ordering::Acquire)
                {
                    return Err("sampler replacement target changed or was cancelled".into());
                }
                let target = session
                    .project()
                    .track(edit.track)
                    .and_then(|track| track.instrument())
                    .filter(|device| {
                        device.id() == edit.device
                            && !device.bypassed()
                            && device.state() == &edit.expected_state
                    })
                    .ok_or("sampler replacement target changed")?;
                hex_engine::validate_sampler_replacement_on_control(target.state())
                    .map_err(|e| e.to_string())?;
                Ok(true)
            }
            Self::History(edit) => {
                if edit.request_id == 0
                    || edit.project_generation != generation
                    || edit.revision != revision
                    || edit.cancel.load(Ordering::Acquire)
                {
                    return Err("sampler history changed or was cancelled".into());
                }
                session
                    .editor
                    .inspect_history(edit.redo, |source, target| {
                        validate_history_edge(
                            session.editor.next_midi_history_effect(edit.redo),
                            edit.redo,
                            source,
                            target,
                        )
                    })
                    .transpose()
                    .map(|edge| edge.is_some())
            }
        }
    }
    /// Ordinary worker publication uses the captured identity and module validation, without the
    /// private CURRENT fence or its history allowlist. Bypass keeps the authored device editable.
    pub(super) fn validate_resident(
        &mut self,
        session: &Session,
        generation: u64,
        revision: u64,
    ) -> Result<bool, String> {
        let (request, expected_generation, expected_revision) = match self {
            Self::Parameter(edit) => (edit.request_id, edit.project_generation, edit.revision),
            Self::Midi(edit) => (edit.request_id, edit.project_generation, edit.revision),
            Self::History(edit) => (edit.request_id, edit.project_generation, edit.revision),
            Self::Replacement(edit) => (edit.request_id, edit.project_generation, edit.revision),
        };
        if request == 0
            || expected_generation != generation
            || expected_revision != revision
            || self.cancel().load(Ordering::Acquire)
        {
            return Err("sampler edit target changed or was cancelled".into());
        }
        match self {
            Self::Parameter(edit) => {
                let device = parameter_device(session.project(), edit, true)
                    .ok_or("sampler edit target is unavailable")?;
                let patched = hex_engine::patch_sampler_parameter_on_control(
                    edit.parameter,
                    device.state(),
                    edit.value,
                )
                .map_err(|error| error.to_string())?;
                Ok(patched != *device.state())
            }
            Self::Replacement(edit) => {
                let device = session
                    .project()
                    .track(edit.track)
                    .and_then(|track| track.instrument())
                    .filter(|device| {
                        device.id() == edit.device && device.state() == &edit.expected_state
                    })
                    .ok_or("sampler replacement target changed")?;
                hex_engine::validate_sampler_replacement_on_control(device.state())
                    .map_err(|error| error.to_string())?;
                Ok(true)
            }
            Self::Midi(edit) => edit
                .command
                .prepare_sampler_midi_edit(session.project())
                .map_err(|error| error.to_string()),
            Self::History(edit) => Ok(session
                .editor
                .inspect_history(edit.redo, |_, _| ())
                .is_some()),
        }
    }
    pub(super) fn prepare(
        self,
        session: &mut Session,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
        audio: &mut dyn WorkerAudioSink,
    ) -> Result<(Created, sampler_export::ServiceExportSnapshot), SessionError> {
        match self {
            Self::Midi(edit) => session.midi_service_snapshot_with_native(
                edit.command,
                admission,
                (context, audio, &edit.cancel),
            ),
            Self::Parameter(edit) => {
                let original = session
                    .project()
                    .device(edit.device)
                    .ok_or(SessionError::Invalid("sampler target is unavailable"))?
                    .state();
                let state = hex_engine::patch_sampler_parameter_on_control(
                    edit.parameter,
                    original,
                    edit.value,
                )?;
                session.command_service_snapshot_with_native(
                    Command::ReplaceDeviceState {
                        device: edit.device,
                        state,
                    },
                    admission,
                    Some((context, audio, &edit.cancel)),
                )
            }
            Self::Replacement(edit) => {
                let allowance = admission.append_original_frame_allowance(session)?;
                let scan = session
                    .native_reservation
                    .ok_or(SessionError::Invalid(
                        "sampler replacement requires resident admission",
                    ))?
                    .max_scan_frames;
                let maximum = usize::try_from(allowance.min(scan))
                    .map_err(|_| SessionError::Invalid("source allowance exceeds frame range"))?
                    .min(IMPORT_FRAME_LIMIT);
                if maximum == 0 {
                    return Err(SessionError::Invalid(
                        "resident original append reservation is full",
                    ));
                }
                if edit.cancel.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid(
                        "sampler source replacement cancelled",
                    ));
                }
                let decoded = decode_wav_file(&edit.path, Limits::new(IMPORT_FILE_LIMIT, maximum))?;
                admission.inspect_appended_original(session, Some(decoded.audio()))?;
                if edit.cancel.load(Ordering::Acquire) || session.shutdown.load(Ordering::Acquire) {
                    return Err(SessionError::Invalid(
                        "sampler source replacement cancelled",
                    ));
                }
                session.replace_sampler_service_snapshot(decoded, &edit, admission, context, audio)
            }
            Self::History(edit) => session.history_service_snapshot_with_native(
                edit.redo,
                admission,
                (context, audio, &edit.cancel),
            ),
        }
    }
}
pub(super) fn validate_history_edge(
    effect: Option<hex_project::MidiHistoryEffect>,
    redo: bool,
    source: &Project,
    target: &Project,
) -> Result<(), String> {
    if let Some(effect) = effect {
        let valid = if redo {
            effect.validates_forward(source, target)
        } else {
            effect.validates_forward(target, source)
        };
        return valid
            .then_some(())
            .ok_or_else(|| "next history step is not an exact midi composition edit".into());
    }
    validate_history_pair(source, target)
}
pub(super) fn validate_history_pair(source: &Project, target: &Project) -> Result<(), String> {
    let delta = source
        .instrument_history_delta(target)
        .ok_or("next history step is not a supported sampler change")?;
    if delta.current.bypassed() {
        return Err("next history step is not an active sampler change".into());
    }
    match delta.assets {
        hex_project::InstrumentAssetHistoryDelta::Unchanged => {
            if !hex_engine::is_sampler_configuration_history_on_control(
                delta.current.state(),
                delta.target.state(),
            )
            .map_err(|e| e.to_string())?
            {
                return Err(
                    "next history step is not a supported sampler configuration edit".into(),
                );
            }
            Ok(())
        }
        hex_project::InstrumentAssetHistoryDelta::Added(new_asset) => {
            validate_replacement_history_forward(
                source,
                target,
                delta.current.state(),
                delta.target.state(),
                new_asset,
            )
        }
        hex_project::InstrumentAssetHistoryDelta::Removed(new_asset) => {
            validate_replacement_history_forward(
                target,
                source,
                delta.target.state(),
                delta.current.state(),
                new_asset,
            )
        }
    }
}
fn validate_replacement_history_forward(
    old_project: &Project,
    new_project: &Project,
    old: &hex_lattice_contract::NativeDeviceState,
    new: &hex_lattice_contract::NativeDeviceState,
    added: &hex_project::Asset,
) -> Result<(), String> {
    let frames = usize::try_from(added.source_frames())
        .map_err(|_| "sampler original frame count exceeds range")?;
    let (template, slot) = hex_engine::sampler_replacement_template_on_control(old, frames)
        .map_err(|e| e.to_string())?;
    if old_project.asset(added.id()).is_some() {
        return Err("next history step does not add a new sampler source".into());
    }
    if !hex_engine::is_empty_sampler_state_on_control(old) {
        let reference = old
            .asset_references()
            .first()
            .filter(|r| old.asset_references().len() == 1 && r.slot == slot)
            .ok_or("replacement history requires one original source binding")?;
        let old_asset = old_project
            .assets()
            .find(|asset| asset.id().value() == reference.asset.value())
            .ok_or("replacement history old source is unavailable")?;
        if old_asset.id() == added.id() || new_project.asset(old_asset.id()) != Some(old_asset) {
            return Err(
                "next history step does not preserve the old sampler catalog source".into(),
            );
        }
    }
    let expected = template
        .with_asset_references(vec![hex_lattice_contract::DeviceAssetReference {
            slot,
            asset: hex_lattice_contract::DeviceAssetKey::new(added.id().value())
                .map_err(|_| "invalid sampler history source identity")?,
        }])
        .map_err(|_| "invalid sampler history source binding")?;
    if expected != *new {
        return Err("next history step is not an exact classic source replacement".into());
    }
    Ok(())
}
