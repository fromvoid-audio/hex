//! One preflighted, fresh-output resident sampler CURRENT, driven on Session's worker.
use super::*;
use hex_engine::AudioCandidatePreparation;
use hex_session::service::{NativeSamplerInput, NativeSamplerReservationHold as Hold};
use std::sync::{
    Mutex,
    atomic::Ordering::{AcqRel, Release},
};
mod edit;
pub(super) use edit::EditPending;

// One native output already has exclusive lifecycle custody. Its single
// unpublished preflight has one persistent cleanup owner, including on failure.
static PREFLIGHT: AtomicBool = AtomicBool::new(false);
static FAILED_PREFLIGHT: Mutex<Option<AudioCandidatePreparation>> = Mutex::new(None);
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Preparing,
    Draining,
    Adopted,
    Submitted,
    Refused,
    Failed,
}
pub(super) struct Sampler {
    context: AudioSourceContext,
    rate: u32,
    cell: Option<AudioCandidatePreparation>,
    reservation: hex_audio::AudioCandidatePreparationReservation,
    hold: Hold,
    token: Option<AudioCandidatePreparationToken>,
    ticket: Option<AudioCandidateTicket>,
    phase: Phase,
    preflight_slot: bool,
}
impl Sampler {
    pub(super) fn context(&self) -> AudioSourceContext {
        self.context
    }
}
fn pcm_hold(frames: u64) -> Hold {
    let bytes = usize::try_from(frames)
        .ok()
        .and_then(|n| n.checked_mul(std::mem::size_of::<hex_engine::Frame>()))
        .unwrap_or(usize::MAX);
    Hold {
        retained_bytes: bytes,
        construction_bytes: bytes,
        frames,
        ..Hold::default()
    }
}
fn add(a: Hold, b: Hold) -> Hold {
    Hold {
        // Saturation is a refusing reservation sentinel, never freed capacity.
        retained_bytes: a.retained_bytes.saturating_add(b.retained_bytes),
        construction_bytes: a.construction_bytes.saturating_add(b.construction_bytes),
        frames: a.frames.saturating_add(b.frames),
        current_context: b.current_context.or(a.current_context),
    }
}
impl NativeAudioSink {
    pub(super) fn reset_refused_sampler_for_retry(&mut self) -> Result<(), String> {
        if self
            .sampler
            .as_ref()
            .is_some_and(|s| s.phase != Phase::Refused)
        {
            return Err(
                "sampler audio retry requires a refused output; current audio is unchanged".into(),
            );
        }
        if self.sampler.is_some() && !self.cancel_sampler() {
            return Err(
                "sampler output cleanup is incomplete; save project and restart hex".into(),
            );
        }
        Ok(())
    }
    pub(super) fn sampler_hold(&self) -> Hold {
        if let Some(ordinary) = self.resident_runtime_hold() {
            return add(ordinary, self.retired_sampler_hold);
        }
        let held = add(
            add(
                add(self.retired_sampler_hold, self.output_hold),
                pcm_hold(self.ordinary_pcm_frames),
            ),
            self.sampler.as_ref().map_or(Hold::default(), |s| s.hold),
        );
        add(held, self.edit_hold())
    }
    pub(super) fn opened_output(&mut self, mut output: WorkerPreparedOutput) {
        self.plugins.fresh_output();
        // Both production and headless factories must acquire the existing
        // exclusive Lease. Successful fresh construction proves old retirement;
        // neither a cleared Option nor UI endpoint acknowledgement does so.
        self.retired_sampler_hold = Hold::default();
        self.resident.retired = None;
        self.ordinary_pcm_frames = 0;
        self.verified_pcm_frames = 0;
        self.suspended_output = None;
        let preflighted = self
            .sampler
            .as_ref()
            .is_some_and(|s| s.phase == Phase::Draining);
        self.output_hold = if preflighted {
            Hold::default()
        } else {
            self.policy.as_ref().map_or(Hold::default(), |p| Hold {
                retained_bytes: p.legacy_hold_bytes,
                construction_bytes: p.legacy_hold_bytes,
                ..Hold::default()
            })
        };
        if self.worker_owned_resident_graphs
            && self.desired.as_ref().is_none_or(|source| {
                source.audio_mode() == hex_session::DocumentAudioMode::Resident
            })
        {
            // The actual fresh Lease has already closed the old teardown frontier. Configure
            // generic graph ownership before either initial publication or Client exposure;
            // a resource-free initial project must not later require another output mode.
            let configured = self
                .policy
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|policy| {
                    let control = output
                        .publication
                        .control()
                        .ok_or_else(|| "audio publication endpoint is closed".to_owned())?;
                    control
                        .configure_device_resource_publication_on_control(
                            policy.resident_native.storage,
                        )
                        .map_err(str::to_owned)?;
                    let domains = if self.plugins.wants_profile() {
                        policy.plugins.domains(policy.ordinary_domains)?
                    } else {
                        policy.ordinary_domains
                    };
                    control
                        .configure_ordinary_streaming_on_control(
                            domains,
                            policy.readiness.extents_per_role,
                            policy.ordinary_stream_metadata_bytes,
                        )
                        .map_err(str::to_owned)?;
                    if let Err(error) = self.plugins.configure(control, policy.plugins) {
                        self.plugins.refuse_profile(error);
                    }
                    Ok(())
                });
            if let Err(error) = configured {
                self.open_error = Some(error.into());
                return;
            }
        }
        self.output = Some(output);
    }
    pub(super) fn retain_relinquished_output(&mut self) {
        if self.retain_resident_output() {
            return;
        }
        if let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) {
            self.ordinary_pcm_frames = control
                .suspend_resident_forwarding_pcm()
                .and_then(|n| n.checked_add(self.verified_pcm_frames))
                .unwrap_or(u64::MAX);
        }
        self.retired_sampler_hold = add(
            self.retired_sampler_hold,
            add(self.output_hold, pcm_hold(self.ordinary_pcm_frames)),
        );
        self.ordinary_pcm_frames = 0;
        self.verified_pcm_frames = 0;
        self.suspended_output = None;
        self.output_hold = Hold::default();
    }
    pub(super) fn sampler_blocks_client(&self) -> bool {
        self.sampler
            .as_ref()
            .is_some_and(|s| s.phase != Phase::Submitted && s.phase != Phase::Refused)
    }
    fn resume_sampler_forwarding(&mut self) {
        if let Some(control) = self.output.as_mut().and_then(|o| o.publication.control())
            && control.output_identity() == self.suspended_output
        {
            control.resume_resident_forwarding();
        }
        self.suspended_output = None;
    }
    fn clean_preflight(&mut self) -> bool {
        let Some(state) = self.sampler.as_mut() else {
            return true;
        };
        if state.phase == Phase::Failed {
            return false;
        } // retain the first incomplete owner; never retry implicitly
        let was_draining = state.phase == Phase::Draining;
        if let Some(cell) = state.cell.as_mut() {
            if cell.cleanup_on_control().is_err() {
                state.phase = Phase::Failed;
                return false;
            }
            drop(state.cell.take());
        }
        if state.preflight_slot {
            PREFLIGHT.store(false, Release);
            state.preflight_slot = false;
        }
        if was_draining {
            if let Ok(policy) = &self.policy {
                let control = Hold {
                    retained_bytes: policy.control_backing_quote,
                    construction_bytes: policy.control_backing_quote,
                    ..Hold::default()
                };
                if self.output.is_some() {
                    self.output_hold = add(self.output_hold, control);
                } else if self.open_pending || self.opening.is_some() {
                    self.retired_sampler_hold = add(self.retired_sampler_hold, control);
                }
            }
        }
        state.hold = Hold::default();
        true
    }
    pub(super) fn cancel_sampler(&mut self) -> bool {
        self.cancel_edit();
        if !self.clean_preflight() {
            return false;
        }
        self.resume_sampler_forwarding();
        if self.output.is_some() && self.sampler.as_ref().is_some_and(|s| s.token.is_some()) {
            self.relinquish_output();
        }
        self.sampler = None;
        true
    }
    fn sampler_refusal_observation(&mut self, context: AudioSourceContext, error: String) {
        if self.output_identity.is_none() {
            self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                project_generation: context.project_generation,
                restart_request: self.restart_request,
                error: error.into(),
            });
        } else {
            self.observe(
                context,
                "sampler-refused",
                EditorPublicationState::Refused(error),
            );
        }
    }
    pub(super) fn report_sampler_cleanup_failure(&mut self, context: AudioSourceContext) {
        let error = "native sampler ownership cleanup is incomplete; restart hex".to_owned();
        if self.output_identity.is_some() {
            self.observe(
                context,
                "sampler-incomplete",
                EditorPublicationState::Fault(error),
            );
        } else {
            self.sampler_refusal_observation(context, error);
        }
    }
    fn sampler_refused(&mut self, error: String) {
        let context = self.sampler.as_ref().map(|s| s.context);
        let disturbed = self
            .sampler
            .as_ref()
            .is_some_and(|s| s.phase != Phase::Preparing);
        let clean = self.clean_preflight();
        if !clean {
            if let Some(context) = context {
                self.report_sampler_cleanup_failure(context);
            }
            return;
        }
        if clean && !disturbed {
            self.resume_sampler_forwarding();
        }
        if clean {
            if let Some(state) = self.sampler.as_mut() {
                state.phase = Phase::Refused;
            }
        }
        if disturbed {
            self.relinquish_output();
            if let Some(context) = context {
                self.pending_open_event = Some(EditorAudioEvent::Unavailable {
                    project_generation: context.project_generation,
                    restart_request: self.restart_request,
                    error: error.into(),
                });
            }
        } else if let Some(context) = context {
            self.sampler_refusal_observation(context, error);
        }
    }
    pub(super) fn advance_sampler(&mut self, input: NativeSamplerInput<'_>) {
        let context = input.context();
        if self.sampler.as_ref().is_some_and(|s| s.context != context) || !input.requires_sampler()
        {
            if !self.cancel_sampler() {
                return;
            }
            if !input.requires_sampler() {
                return;
            }
        }
        let retry_facts = input.retry_facts();
        if self.blocked == Some(context) {
            if self
                .sampler_budget_wait
                .is_some_and(|prior| prior != retry_facts)
            {
                self.blocked = None;
                self.sampler_budget_wait = None;
            } else {
                return;
            }
        }
        if self.sampler.is_none() {
            let policy = match self.policy.as_ref() {
                Ok(p) => *p,
                Err(_) => return,
            };
            if let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) {
                let Some(frames) = control.suspend_resident_forwarding_pcm() else {
                    return;
                };
                self.ordinary_pcm_frames = frames.saturating_add(self.verified_pcm_frames);
                self.suspended_output = control.output_identity();
            }
            let (quote, storage, sources, frames) = match input.quote() {
                Ok(v) => v,
                Err(error) => {
                    self.resume_sampler_forwarding();
                    self.blocked = Some(context);
                    self.sampler_budget_wait = None;
                    self.sampler_refusal_observation(context, error);
                    return;
                }
            };
            let Some(retained) = storage
                .max_prepared_data_bytes
                .checked_add(policy.control_backing_quote)
            else {
                self.resume_sampler_forwarding();
                return;
            };
            let Some(peak) = storage
                .max_construction_data_bytes
                .checked_add(policy.control_backing_quote)
            else {
                self.resume_sampler_forwarding();
                return;
            };
            let hold = Hold {
                retained_bytes: retained,
                construction_bytes: peak,
                frames,
                current_context: Some(context),
            };
            if let Err(error) = input.admit_additional(hold, self.sampler_hold()) {
                self.resume_sampler_forwarding();
                self.blocked = Some(context);
                self.sampler_budget_wait = Some(retry_facts);
                self.sampler_refusal_observation(context, error);
                return;
            }
            self.sampler_budget_wait = None;
            if PREFLIGHT
                .compare_exchange(false, true, AcqRel, Acquire)
                .is_err()
            {
                self.resume_sampler_forwarding();
                self.observe(
                    context,
                    "sampler-incomplete",
                    EditorPublicationState::Refused(
                        "native sampler preparation is busy or cleanup is incomplete".into(),
                    ),
                );
                return;
            }
            self.sampler = Some(Sampler {
                context,
                rate: input.sample_rate(),
                cell: None,
                reservation: hex_audio::AudioCandidatePreparationReservation {
                    source_capacity: sources,
                    storage,
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                },
                hold,
                token: None,
                ticket: None,
                phase: Phase::Preparing,
                preflight_slot: true,
            });
            match AudioCandidatePreparation::try_new_on_control(sources, storage) {
                Ok(cell) => {
                    if let Some(state) = self.sampler.as_mut() {
                        state.cell = Some(cell);
                    }
                }
                Err(e) => {
                    self.sampler_refused(
                        format!("sampler construction refused: {e:?}").to_lowercase(),
                    );
                    return;
                }
            }
            let catalog = self.next_catalog;
            let Some(next) = catalog.checked_add(1) else {
                self.sampler_refused("sampler source identity exhausted".into());
                return;
            };
            self.next_catalog = next;
            let result = self
                .sampler
                .as_mut()
                .and_then(|s| s.cell.as_mut())
                .ok_or(hex_engine::AudioStorageError::SourceMismatch)
                .and_then(|cell| input.prepare(catalog, cell))
                .and_then(|()| {
                    self.sampler
                        .as_mut()
                        .and_then(|s| s.cell.as_mut())
                        .and_then(|c| c.owner_on_control())
                        .ok_or(hex_engine::AudioStorageError::SourceMismatch)?
                        .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                            data_rows: quote.data_rows,
                            handle_rows: quote.handle_rows,
                            root_rows: 2,
                            max_retained_bytes: storage.max_prepared_data_bytes,
                            max_construction_peak_bytes: storage.max_construction_data_bytes,
                        })
                        .map(|_| ())
                });
            if let Err(error) = result {
                self.sampler_refused(
                    format!("sampler preparation refused: {error:?}").to_lowercase(),
                );
                return;
            }
            let actual_frames = self
                .sampler
                .as_ref()
                .and_then(|s| s.cell.as_ref())
                .and_then(|cell| cell.current_sampler_pcm_capacity_frames().ok());
            if actual_frames.is_none_or(|actual| actual > frames) {
                self.sampler_refused(
                    "actual sampler pcm capacity exceeds its preflight frame reservation".into(),
                );
                return;
            }
            let old_output_identity = self.output_identity;
            if let Some(state) = self.sampler.as_mut() {
                state.phase = Phase::Draining;
            }
            self.relinquish_output();
            if let Some(old_output_identity) = old_output_identity {
                self.pending_open_event = Some(EditorAudioEvent::RelinquishClient {
                    old_output_identity,
                    target: context,
                });
            }
            self.request_open(Some(input.sample_rate()), None);
        }
        let Some(state) = self.sampler.as_ref() else {
            return;
        };
        if state.context != context
            || matches!(
                state.phase,
                Phase::Failed | Phase::Refused | Phase::Submitted
            )
        {
            return;
        }
        if state.phase == Phase::Draining && self.output.is_none() {
            if let Some(error) = self.open_error.take() {
                self.sampler_refused(error.to_string());
            }
            return;
        }
        if state.phase == Phase::Draining {
            let policy = match self.policy.as_ref() {
                Ok(p) => *p,
                Err(_) => return,
            };
            let result: Result<(), &'static str> = (|| {
                let output = self.output.as_mut().ok_or("fresh output missing")?;
                let state = self.sampler.as_mut().ok_or("sampler preflight missing")?;
                if output.sample_rate != state.rate {
                    return Err("fresh sampler output rate differs; rebuild required");
                }
                let control = output
                    .publication
                    .control()
                    .ok_or("fresh publication closed")?;
                if state.token.is_none() {
                    control.configure_audio_candidate_publication(policy.resident_publication)?;
                    if control
                        .audio_candidate_control_backing_bytes()
                        .is_none_or(|n| n > policy.control_backing_quote)
                    {
                        return Err("actual native control backing exceeds its preflight quote");
                    }
                    state.token = Some(
                        control
                            .reserve_audio_candidate_adoption(state.reservation)
                            .map_err(|e| e.reason)?,
                    );
                }
                let token = state.token.ok_or("sampler destination missing")?;
                control.adopt_audio_candidate_preparation(token, context, &mut state.cell)?;
                state.phase = Phase::Adopted;
                self.output_hold = state.hold;
                state.hold = Hold::default();
                if state.preflight_slot {
                    PREFLIGHT.store(false, Release);
                    state.preflight_slot = false;
                }
                Ok(())
            })();
            if let Err(error) = result {
                self.sampler_refused(error.into());
                return;
            }
        }
        if self
            .sampler
            .as_ref()
            .is_some_and(|s| s.phase == Phase::Adopted)
        {
            let result: Result<(), &'static str> = (|| {
                let state = self.sampler.as_mut().ok_or("sampler state missing")?;
                let control = self
                    .output
                    .as_mut()
                    .and_then(|o| o.publication.control())
                    .ok_or("sampler publication closed")?;
                let token = state.token.ok_or("sampler destination missing")?;
                let ticket = control.stage_current_sampler_preparation(token, context)?;
                state.ticket = Some(ticket);
                state.phase = Phase::Submitted;
                Ok(())
            })();
            if let Err(error) = result {
                self.sampler_refused(error.into());
            }
        }
    }
    pub(super) fn poll_sampler_output(&mut self, output: &mut WorkerPreparedOutput) {
        let Some(control) = output.publication.control() else {
            return;
        };
        control.reclaim();
        let Some(state) = self.sampler.as_ref() else {
            return;
        };
        let context = state.context;
        if control.status.device_fault.load(Acquire)
            || control.status.render_fault.load(Acquire)
            || !control.audio_candidate_claims_available()
        {
            self.observe(
                context,
                "sampler-fault",
                EditorPublicationState::Fault(
                    "sampler audio failed; close or open a project to recover".into(),
                ),
            );
        } else if state.phase == Phase::Submitted {
            self.observe(
                context,
                "sampler-submitted",
                EditorPublicationState::Submitted {
                    audio_revision: control.submitted_revision(),
                },
            );
        }
    }
    pub(super) fn shutdown_sampler(&mut self) {
        self.shutdown_edit();
        if !self.clean_preflight() {
            if let Some(state) = self.sampler.as_mut() {
                // The exclusive preflight slot remains closed until process exit.
                // Failed owner cleanup is never retried by a destructor.
                let mut quarantine = FAILED_PREFLIGHT
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if quarantine.is_none() {
                    *quarantine = state.cell.take();
                }
            }
        }
    }
}
