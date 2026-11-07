//! One pending stopped sampler setting update. Old CURRENT remains installed through all
//! preparation refusal paths; a committed edit drains/adopts this exact cell.
use super::*;
use hex_audio::prepared::{StoppedEditFence, StoppedEditFenceState};
use std::time::{Duration, Instant};

pub(in crate::native_audio) struct EditPending {
    old: AudioSourceContext,
    fence: StoppedEditFence,
    deadline: Instant,
    candidate: Option<Sampler>,
    cleanup_failed: bool,
}
impl NativeAudioSink {
    pub(in crate::native_audio) fn poll_edit_timeout(&mut self) {
        if let Some(pending) = self.sampler_parameter.as_ref()
            && !pending.cleanup_failed
            && Instant::now() >= pending.deadline
            && let Some(control) = self.output.as_mut().and_then(|o| o.publication.control())
        {
            // Native polling continues even behind canonical Event backpressure.
            // Keep the intent for its eventual error response, but do not strand
            // transport exclusion while no UI reader can consume that response.
            control.release_stopped_edit_fence(pending.fence);
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::native_audio) fn expire_gain_fence_for_test(&mut self) {
        if let Some(pending) = self.sampler_parameter.as_mut() {
            pending.deadline = Instant::now();
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::native_audio) fn pending_gain_cell_identity(
        &mut self,
    ) -> Option<hex_engine::AudioCandidateIdentity> {
        self.sampler_parameter
            .as_mut()?
            .candidate
            .as_mut()?
            .cell
            .as_mut()?
            .owner_on_control()
            .map(|o| o.identity())
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::native_audio) fn draining_gain_cell_identity(
        &mut self,
    ) -> Option<hex_engine::AudioCandidateIdentity> {
        self.sampler
            .as_mut()?
            .cell
            .as_mut()?
            .owner_on_control()
            .map(|o| o.identity())
    }
    pub(in crate::native_audio) fn edit_hold(&self) -> Hold {
        self.sampler_parameter
            .as_ref()
            .and_then(|g| g.candidate.as_ref())
            .map_or(Hold::default(), |s| s.hold)
    }
    pub(in crate::native_audio) fn begin_edit(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        if self.sampler_parameter.is_some() {
            return Err("sampler edit is busy or cleanup is incomplete".into());
        }
        if !self
            .sampler
            .as_ref()
            .is_some_and(|s| s.context == context && s.phase == Phase::Submitted)
        {
            return Err("sampler edit requires current sampler audio".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|o| o.publication.control())
            .ok_or("sampler edit output is unavailable")?;
        let source = control
            .status
            .applied_audio_source()
            .filter(|s| s.context == context)
            .ok_or("sampler edit audio is not current")?;
        let fence = control
            .request_sampler_edit_fence(source)
            .map_err(str::to_owned)?;
        self.sampler_parameter = Some(EditPending {
            old: context,
            fence,
            deadline: Instant::now() + Duration::from_secs(3),
            candidate: None,
            cleanup_failed: false,
        });
        Ok(())
    }
    pub(in crate::native_audio) fn edit_ready(&mut self) -> Option<Result<(), String>> {
        let pending = self.sampler_parameter.as_ref()?;
        if pending.cleanup_failed {
            return None;
        }
        if Instant::now() >= pending.deadline {
            return Some(Err(
                "sampler edit timed out waiting for stopped audio".into()
            ));
        }
        let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) else {
            return Some(Err("sampler edit output closed".into()));
        };
        match control.stopped_edit_fence_state(pending.fence) {
            StoppedEditFenceState::Pending => None,
            StoppedEditFenceState::Ready => Some(Ok(())),
            // The editor stops before a streamed sampler edit and resumes
            // after it, so a refusal means playback changed meanwhile.
            StoppedEditFenceState::Refused => {
                Some(Err("sampler edit audio changed · try again".into()))
            }
        }
    }
    pub(in crate::native_audio) fn prepare_edit(
        &mut self,
        input: NativeSamplerInput<'_>,
    ) -> Result<(), String> {
        self.edit_ready()
            .ok_or("sampler edit is waiting for audio")??;
        let old = self
            .sampler_parameter
            .as_ref()
            .ok_or("sampler edit was cancelled")?
            .old;
        let context = input.context();
        if context.project_generation != old.project_generation
            || old.editor_revision.checked_add(1) != Some(context.editor_revision)
        {
            return Err("sampler edit candidate is stale".into());
        }
        let policy = *self.policy.as_ref().map_err(Clone::clone)?;
        let (quote, storage, sources, frames) = input.quote()?;
        let hold = Hold {
            retained_bytes: storage
                .max_prepared_data_bytes
                .checked_add(policy.control_backing_quote)
                .ok_or("sampler edit retained quote overflow")?,
            construction_bytes: storage
                .max_construction_data_bytes
                .checked_add(policy.control_backing_quote)
                .ok_or("sampler edit peak quote overflow")?,
            frames,
            current_context: Some(context),
        };
        input.admit_additional(hold, self.sampler_hold())?;
        let catalog = self.next_catalog;
        let next = catalog
            .checked_add(1)
            .ok_or("sampler source identity exhausted")?;
        let pending = self
            .sampler_parameter
            .as_mut()
            .ok_or("sampler edit was cancelled")?;
        if PREFLIGHT
            .compare_exchange(false, true, AcqRel, Acquire)
            .is_err()
        {
            return Err("native sampler preparation is busy or cleanup is incomplete".into());
        }
        // Persistent cleanup owner exists before any fallible constructor/copy.
        pending.candidate = Some(Sampler {
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
        self.next_catalog = next;
        let state = pending
            .candidate
            .as_mut()
            .ok_or("sampler edit preparation missing")?;
        state.cell = Some(
            AudioCandidatePreparation::try_new_on_control(sources, storage)
                .map_err(|e| format!("sampler edit construction refused: {e:?}").to_lowercase())?,
        );
        let cell = state.cell.as_mut().ok_or("sampler edit cell missing")?;
        input
            .prepare(catalog, cell)
            .map_err(|e| format!("sampler edit preparation refused: {e:?}").to_lowercase())?;
        cell.owner_on_control()
            .ok_or("sampler edit owner missing")?
            .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 2,
                max_retained_bytes: storage.max_prepared_data_bytes,
                max_construction_peak_bytes: storage.max_construction_data_bytes,
            })
            .map_err(|e| format!("sampler edit sealing refused: {e:?}").to_lowercase())?;
        let actual_frames = cell
            .current_sampler_pcm_capacity_frames()
            .map_err(|_| "sampler edit pcm inventory is unavailable")?;
        if actual_frames > frames {
            return Err("sampler edit actual pcm capacity exceeds preflight".into());
        }
        // Recheck the still-live fence after potentially costly control preparation.
        self.edit_ready()
            .ok_or("sampler edit fence disappeared")??;
        Ok(())
    }
    pub(in crate::native_audio) fn commit_edit(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        let pending = self
            .sampler_parameter
            .as_ref()
            .ok_or("sampler edit preparation missing")?;
        if !pending
            .candidate
            .as_ref()
            .is_some_and(|s| s.context == context && s.cell.is_some())
        {
            return Err("sampler edit candidate changed".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|o| o.publication.control())
            .ok_or("sampler edit output closed after commit")?;
        control
            .commit_sampler_edit_fence(pending.fence)
            .map_err(str::to_owned)?;
        let mut pending = self
            .sampler_parameter
            .take()
            .ok_or("sampler edit was cancelled")?;
        let mut candidate = pending
            .candidate
            .take()
            .ok_or("sampler edit candidate missing")?;
        let old_output_identity = self.output_identity;
        if !self.cancel_sampler() {
            // Submitted old CURRENT has no unpublished cell. Keep the new owner
            // explicit even if an unexpected old cleanup fault is reported.
            pending.candidate = Some(candidate);
            self.sampler_parameter = Some(pending);
            return Err("sampler edit old output cleanup is incomplete".into());
        }
        candidate.phase = Phase::Draining;
        self.sampler = Some(candidate);
        if let Some(old_output_identity) = old_output_identity {
            self.pending_open_event = Some(EditorAudioEvent::RelinquishClient {
                old_output_identity,
                target: context,
            });
        }
        let rate = self.sampler.as_ref().map(|s| s.rate);
        self.request_open(rate, None);
        Ok(())
    }
    pub(in crate::native_audio) fn cancel_edit(&mut self) {
        let Some(mut pending) = self.sampler_parameter.take() else {
            return;
        };
        if pending.cleanup_failed {
            self.sampler_parameter = Some(pending);
            return;
        }
        if let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) {
            control.release_stopped_edit_fence(pending.fence);
        }
        if let Some(candidate) = pending.candidate.as_mut() {
            if candidate
                .cell
                .as_mut()
                .is_some_and(|c| c.cleanup_on_control().is_err())
            {
                pending.cleanup_failed = true;
                self.sampler_parameter = Some(pending);
                return;
            }
            drop(candidate.cell.take());
            if candidate.preflight_slot {
                PREFLIGHT.store(false, Release);
                candidate.preflight_slot = false;
            }
        }
    }
    pub(in crate::native_audio) fn shutdown_edit(&mut self) {
        self.cancel_edit();
        if let Some(pending) = self.sampler_parameter.as_mut()
            && let Some(candidate) = pending.candidate.as_mut()
        {
            let mut quarantine = FAILED_PREFLIGHT
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if quarantine.is_none() {
                *quarantine = candidate.cell.take();
            }
        }
    }
}
