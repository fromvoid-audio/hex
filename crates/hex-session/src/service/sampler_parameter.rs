//! Captured edit intent validation. The native sink owns the callback fence and
//! sealed pending cell; canonical state/history commit stays with Session.
use super::*;
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
    let device = session
        .project()
        .track(edit.track)
        .and_then(|t| t.instrument())
        .filter(|d| d.id() == edit.device && !d.bypassed())
        .ok_or("sampler edit target is unavailable")?;
    hex_engine::patch_sampler_parameter_on_control(edit.parameter, device.state(), edit.value)
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// The sole pending native edit: one fence, serial and final completion.
#[derive(Debug)]
pub enum SamplerEdit {
    Parameter(SamplerParameterEdit),
    History(SamplerHistoryEdit),
    Replacement(SamplerReplacementEdit),
}
impl SamplerEdit {
    pub(super) fn request_id(&self) -> u64 {
        match self {
            Self::Parameter(e) => e.request_id,
            Self::History(e) => e.request_id,
            Self::Replacement(e) => e.request_id,
        }
    }
    pub(super) fn cancel(&self) -> &AtomicBool {
        match self {
            Self::Parameter(e) => &e.cancel,
            Self::History(e) => &e.cancel,
            Self::Replacement(e) => &e.cancel,
        }
    }
    pub(super) fn validate(
        &self,
        session: &Session,
        generation: u64,
        revision: u64,
    ) -> Result<bool, String> {
        match self {
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
                    .inspect_history(edit.redo, validate_history_pair)
                    .transpose()
                    .map(|edge| edge.is_some())
            }
        }
    }
    pub(super) fn prepare(
        &self,
        session: &mut Session,
        admission: sampler_export::Admission<'_>,
        context: hex_engine::AudioSourceContext,
        audio: &mut dyn WorkerAudioSink,
    ) -> Result<(Created, sampler_export::ServiceExportSnapshot), SessionError> {
        match self {
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
                session.replace_sampler_service_snapshot(decoded, edit, admission, context, audio)
            }
            Self::History(edit) => session.history_service_snapshot_with_native(
                edit.redo,
                admission,
                (context, audio, &edit.cancel),
            ),
        }
    }
}
pub(super) fn validate_history_pair(source: &Project, target: &Project) -> Result<(), String> {
    let (_, current, next) = source
        .instrument_state_history_delta(target)
        .ok_or("next history step is not a same-source sampler configuration edit")?;
    if current.bypassed()
        || !hex_engine::is_sampler_configuration_history_on_control(current.state(), next.state())
            .map_err(|e| e.to_string())?
    {
        return Err("next history step is not a supported sampler configuration edit".into());
    }
    Ok(())
}
