//! One pending stopped gain update. Old CURRENT remains installed through all
//! preparation refusal paths; a committed edit drains/adopts this exact cell.
use super::*;
use hex_audio::prepared::{SamplerEditFence, SamplerEditFenceState};
use std::time::{Duration, Instant};

pub(in crate::native_audio) struct GainPending {
    old: AudioSourceContext,
    fence: SamplerEditFence,
    deadline: Instant,
    candidate: Option<Sampler>,
    cleanup_failed: bool,
}
impl NativeAudioSink {
    pub(in crate::native_audio) fn poll_gain_timeout(&mut self) {
        if let Some(pending) = self.sampler_gain.as_ref()
            && !pending.cleanup_failed
            && Instant::now() >= pending.deadline
            && let Some(control) = self.output.as_mut().and_then(|o| o.publication.control())
        {
            // Native polling continues even behind canonical Event backpressure.
            // Keep the intent for its eventual error response, but do not strand
            // transport exclusion while no UI reader can consume that response.
            control.release_sampler_edit_fence(pending.fence);
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::native_audio) fn expire_gain_fence_for_test(&mut self) {
        if let Some(pending) = self.sampler_gain.as_mut() {
            pending.deadline = Instant::now();
        }
    }
    #[cfg(all(test, feature = "allocation-probe"))]
    pub(in crate::native_audio) fn pending_gain_cell_identity(
        &mut self,
    ) -> Option<hex_engine::AudioCandidateIdentity> {
        self.sampler_gain
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
    pub(in crate::native_audio) fn gain_hold(&self) -> Hold {
        self.sampler_gain
            .as_ref()
            .and_then(|g| g.candidate.as_ref())
            .map_or(Hold::default(), |s| s.hold)
    }
    pub(in crate::native_audio) fn begin_gain(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        if self.sampler_gain.is_some() {
            return Err("sampler gain is busy or cleanup is incomplete".into());
        }
        if !self
            .sampler
            .as_ref()
            .is_some_and(|s| s.context == context && s.phase == Phase::Submitted)
        {
            return Err("sampler gain requires current sampler audio".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|o| o.publication.control())
            .ok_or("sampler gain output is unavailable")?;
        let source = control
            .status
            .applied_audio_source()
            .filter(|s| s.context == context)
            .ok_or("sampler gain audio is not current")?;
        let fence = control
            .request_sampler_edit_fence(source)
            .map_err(str::to_owned)?;
        self.sampler_gain = Some(GainPending {
            old: context,
            fence,
            deadline: Instant::now() + Duration::from_secs(3),
            candidate: None,
            cleanup_failed: false,
        });
        Ok(())
    }
    pub(in crate::native_audio) fn gain_ready(&mut self) -> Option<Result<(), String>> {
        let pending = self.sampler_gain.as_ref()?;
        if pending.cleanup_failed {
            return None;
        }
        if Instant::now() >= pending.deadline {
            return Some(Err(
                "sampler gain timed out waiting for stopped audio".into()
            ));
        }
        let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) else {
            return Some(Err("sampler gain output closed".into()));
        };
        match control.sampler_edit_fence_state(pending.fence) {
            SamplerEditFenceState::Pending => None,
            SamplerEditFenceState::Ready => Some(Ok(())),
            SamplerEditFenceState::Refused => {
                Some(Err("stop playback before editing sampler gain".into()))
            }
        }
    }
    pub(in crate::native_audio) fn prepare_gain(
        &mut self,
        input: NativeSamplerInput<'_>,
    ) -> Result<(), String> {
        self.gain_ready()
            .ok_or("sampler gain is waiting for audio")??;
        let old = self
            .sampler_gain
            .as_ref()
            .ok_or("sampler gain was cancelled")?
            .old;
        let context = input.context();
        if context.project_generation != old.project_generation
            || old.editor_revision.checked_add(1) != Some(context.editor_revision)
        {
            return Err("sampler gain candidate is stale".into());
        }
        let policy = *self.policy.as_ref().map_err(Clone::clone)?;
        let (quote, storage, sources, frames) = input.quote()?;
        let hold = Hold {
            retained_bytes: storage
                .max_prepared_data_bytes
                .checked_add(policy.control_backing_quote)
                .ok_or("sampler gain retained quote overflow")?,
            construction_bytes: storage
                .max_construction_data_bytes
                .checked_add(policy.control_backing_quote)
                .ok_or("sampler gain peak quote overflow")?,
            frames,
            current_context: Some(context),
        };
        input.admit_additional(hold, self.sampler_hold())?;
        let catalog = self.next_catalog;
        let next = catalog
            .checked_add(1)
            .ok_or("sampler source identity exhausted")?;
        let pending = self
            .sampler_gain
            .as_mut()
            .ok_or("sampler gain was cancelled")?;
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
            .ok_or("sampler gain preparation missing")?;
        state.cell = Some(
            AudioCandidatePreparation::try_new_on_control(sources, storage)
                .map_err(|e| format!("sampler gain construction refused: {e:?}").to_lowercase())?,
        );
        let cell = state.cell.as_mut().ok_or("sampler gain cell missing")?;
        input
            .prepare(catalog, cell)
            .map_err(|e| format!("sampler gain preparation refused: {e:?}").to_lowercase())?;
        cell.owner_on_control()
            .ok_or("sampler gain owner missing")?
            .seal_constructor_on_control(hex_engine::AudioStorageLedgerReservation {
                data_rows: quote.data_rows,
                handle_rows: quote.handle_rows,
                root_rows: 2,
                max_retained_bytes: storage.max_prepared_data_bytes,
                max_construction_peak_bytes: storage.max_construction_data_bytes,
            })
            .map_err(|e| format!("sampler gain sealing refused: {e:?}").to_lowercase())?;
        let actual_frames = cell
            .current_sampler_pcm_capacity_frames()
            .map_err(|_| "sampler gain pcm inventory is unavailable")?;
        if actual_frames > frames {
            return Err("sampler gain actual pcm capacity exceeds preflight".into());
        }
        // Recheck the still-live fence after potentially costly control preparation.
        self.gain_ready()
            .ok_or("sampler gain fence disappeared")??;
        Ok(())
    }
    pub(in crate::native_audio) fn commit_gain(
        &mut self,
        context: AudioSourceContext,
    ) -> Result<(), String> {
        let pending = self
            .sampler_gain
            .as_ref()
            .ok_or("sampler gain preparation missing")?;
        if !pending
            .candidate
            .as_ref()
            .is_some_and(|s| s.context == context && s.cell.is_some())
        {
            return Err("sampler gain candidate changed".into());
        }
        let control = self
            .output
            .as_mut()
            .and_then(|o| o.publication.control())
            .ok_or("sampler gain output closed after commit")?;
        control
            .commit_sampler_edit_fence(pending.fence)
            .map_err(str::to_owned)?;
        let mut pending = self
            .sampler_gain
            .take()
            .ok_or("sampler gain was cancelled")?;
        let mut candidate = pending
            .candidate
            .take()
            .ok_or("sampler gain candidate missing")?;
        let old_output_identity = self.output_identity;
        if !self.cancel_sampler() {
            // Submitted old CURRENT has no unpublished cell. Keep the new owner
            // explicit even if an unexpected old cleanup fault is reported.
            pending.candidate = Some(candidate);
            self.sampler_gain = Some(pending);
            return Err("sampler gain old output cleanup is incomplete".into());
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
    pub(in crate::native_audio) fn cancel_gain(&mut self) {
        let Some(mut pending) = self.sampler_gain.take() else {
            return;
        };
        if pending.cleanup_failed {
            self.sampler_gain = Some(pending);
            return;
        }
        if let Some(control) = self.output.as_mut().and_then(|o| o.publication.control()) {
            control.release_sampler_edit_fence(pending.fence);
        }
        if let Some(candidate) = pending.candidate.as_mut() {
            if candidate
                .cell
                .as_mut()
                .is_some_and(|c| c.cleanup_on_control().is_err())
            {
                pending.cleanup_failed = true;
                self.sampler_gain = Some(pending);
                return;
            }
            drop(candidate.cell.take());
            if candidate.preflight_slot {
                PREFLIGHT.store(false, Release);
                candidate.preflight_slot = false;
            }
        }
    }
    pub(in crate::native_audio) fn shutdown_gain(&mut self) {
        self.cancel_gain();
        if let Some(pending) = self.sampler_gain.as_mut()
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
