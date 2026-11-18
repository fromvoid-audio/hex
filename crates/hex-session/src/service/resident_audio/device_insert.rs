//! Keep a validated device edit private until the callback installs its exact graph.
use super::*;

/// A one-shot decision for a staged insert. It grants no direct audio access.
#[derive(Clone, Debug)]
pub struct DeviceInsertConfirmation {
    submission: ResidentGraphSubmission,
    deadline: Instant,
    decision: Arc<std::sync::atomic::AtomicU8>,
}

impl DeviceInsertConfirmation {
    fn new(submission: ResidentGraphSubmission, deadline: Instant) -> Self {
        Self {
            submission,
            deadline,
            decision: Arc::default(),
        }
    }

    pub fn submission(&self) -> ResidentGraphSubmission {
        self.submission
    }

    pub fn is_pending(&self) -> bool {
        self.progress().is_none()
    }

    pub fn proceed(&self) -> bool {
        Instant::now() < self.deadline && self.decide(1)
    }

    pub fn cancel(&self) -> bool {
        self.decide(2)
    }

    fn decide(&self, decision: u8) -> bool {
        self.decision
            .compare_exchange(0, decision, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn progress(&self) -> Option<Progress> {
        match self.decision.load(Ordering::Acquire) {
            2 => Some(Progress::Terminal(DeviceInsertAudioState::Cancelled)),
            _ if Instant::now() >= self.deadline => {
                Some(Progress::Terminal(DeviceInsertAudioState::Expired))
            }
            1 => Some(Progress::Proceed),
            _ => None,
        }
    }
}

pub(in crate::service) struct Pending {
    edit: hex_project::StagedProjectEdit,
    snapshot: Snapshot,
    receipt: Receipt,
    predecessor: ResidentRuntimeLoop,
    confirmation: Option<DeviceInsertConfirmation>,
    pub(in crate::service) document_intent: u64,
    pub(in crate::service) plugin: Option<plugin_effect::Pending>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(in crate::service) enum Progress {
    Proceed,
    Terminal(DeviceInsertAudioState),
}

struct Receipt(Box<dyn DeviceInsertAudioReceipt>);

impl Drop for Receipt {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

impl Pending {
    pub(in crate::service) fn submission(&self) -> ResidentGraphSubmission {
        self.receipt.0.submission()
    }

    pub(in crate::service) fn awaiting_confirmation(&self) -> bool {
        self.confirmation.is_some()
    }

    pub(in crate::service) fn cancel_confirmation(&self) {
        if let Some(offer) = self.confirmation.as_ref() {
            offer.cancel();
        }
    }

    pub(in crate::service) fn poll(&self, cancel: bool) -> Option<Progress> {
        let cancel = cancel
            || self
                .plugin
                .as_ref()
                .is_some_and(|plugin| plugin.insertion.control.is_cancelled());
        if let Some(offer) = self.confirmation.as_ref() {
            if cancel {
                offer.cancel();
                return Some(Progress::Terminal(DeviceInsertAudioState::Cancelled));
            }
            return offer.progress();
        }
        if cancel {
            // If the callback already claimed admission, its terminal owns the edit.
            self.receipt.0.cancel();
        }
        match self.receipt.0.state() {
            DeviceInsertAudioState::Pending | DeviceInsertAudioState::Applying => None,
            terminal => Some(Progress::Terminal(terminal)),
        }
    }

    pub(in crate::service) fn offer(
        &mut self,
        sink: &mut dyn WorkerAudioSink,
    ) -> Option<DeviceInsertConfirmation> {
        if self.confirmation.is_some() {
            return None;
        }
        let deadline = sink.offer_device_insert_consent(self.submission())?;
        if self.plugin.as_ref().is_some_and(|plugin| {
            !plugin
                .insertion
                .control
                .release_refused_insert_receipt(self.submission())
        }) {
            return None;
        }
        let offer = DeviceInsertConfirmation::new(self.submission(), deadline);
        self.confirmation = Some(offer.clone());
        Some(offer)
    }

    pub(in crate::service) fn proceed(
        &mut self,
        session: &mut Session,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
    ) -> Result<(), SessionError> {
        let refused = self.submission();
        let inputs = session.resident_seek_inputs(admission, current, None)?;
        let metadata = canonical_metadata(&self.snapshot, refused.context);
        inputs.publish_tagged_with_policy(
            &mut self.snapshot,
            self.edit.project(),
            sink,
            metadata,
            ResidentPublicationPolicy::DeviceInsert {
                predecessor: self.predecessor,
                hosted: self.plugin.is_some(),
                confirmation: Some(refused),
            },
        )?;
        self.receipt = Receipt(insert_receipt(&self.snapshot, sink)?);
        self.confirmation = None;
        Ok(())
    }

    pub(in crate::service) fn commit(
        self,
        session: &mut Session,
        previous: Option<&Snapshot>,
    ) -> Result<(Created, Snapshot), SessionError> {
        let created = session.editor.commit_staged_edit(self.edit)?;
        if let Some(previous) = previous {
            self.snapshot.inputs.cancel_retired_from(&previous.inputs);
        }
        Ok((created, self.snapshot))
    }
}

fn insert_receipt(
    snapshot: &Snapshot,
    sink: &mut dyn WorkerAudioSink,
) -> Result<Box<dyn DeviceInsertAudioReceipt>, SessionError> {
    snapshot
        .submission
        .and_then(|submission| sink.device_insert_receipt(submission))
        .filter(|receipt| Some(receipt.submission()) == snapshot.submission)
        .ok_or(SessionError::Invalid(
            "device insertion receipt unavailable",
        ))
}

pub(in crate::service) fn is_insert(command: &Command) -> bool {
    match command {
        Command::InitializeDevice(initialization) => is_insert(initialization.command()),
        Command::InsertDevice { .. }
        | Command::InsertInstrument { .. }
        | Command::InsertMainDevice { .. }
        | Command::InsertReturnDevice { .. }
        | Command::InsertTrackWithDevice { .. }
        | Command::InsertDefaultTrackWithDevice { .. }
        | Command::InsertDefaultTrackWithDeviceAndGain { .. }
        | Command::InsertNestedDevice { .. } => true,
        _ => false,
    }
}

impl Session {
    #[allow(clippy::too_many_arguments)] // Exact editor, audio and worker request inputs.
    pub(in crate::service) fn stage_resident_device_insert(
        &mut self,
        command: Command,
        context: hex_engine::AudioSourceContext,
        sink: &mut dyn WorkerAudioSink,
        admission: sampler_export::Admission<'_>,
        current: Option<&Snapshot>,
        plugin: Option<&PluginEffectControl>,
        document_intent: u64,
    ) -> Result<Pending, SessionError> {
        let staged = (|| {
            crate::validate_command_media(&command)?;
            let edit = self.editor.stage_edit(command)?;
            let installed = sink
                .resident_runtime_loop(context)
                .map_err(SessionError::NativeAudio)?
                .filter(|actual| {
                    // A delayed open may install the canonical offline graph
                    // on a fresh output without updating the Session submission.
                    // The sink supplies and revalidates the actual output/revision.
                    current.is_some_and(|snapshot| {
                        actual.audio_revision != 0
                            && actual.source_context == Some(snapshot.prepared_context)
                            && matches!(
                                actual.source_origin,
                                ResidentSourceOrigin::Unknown
                                    | ResidentSourceOrigin::CanonicalCandidate
                            )
                    })
                })
                .ok_or(SessionError::Invalid(
                    "device insertion needs the current installed audio graph",
                ))?;
            let context = hex_engine::AudioSourceContext {
                editor_revision: context.editor_revision.saturating_add(1),
                ..context
            };
            let inputs = Inputs {
                preparing_edit: plugin.is_none(),
                ..self.resident_seek_inputs(admission, current, None)?
            };
            let mut snapshot = inputs.prepare_transition(
                self.project(),
                edit.project(),
                false,
                sink,
                &self.media_bank(),
                self.sine_table(),
                &self.audio_transforms,
                &self.audio_clip_waveforms,
                context,
            )?;
            snapshot.source_origin = ResidentSourceOrigin::CanonicalCandidate;
            if plugin.is_some_and(PluginEffectControl::is_cancelled) {
                return Err(SessionError::Invalid("vst3 insertion was cancelled"));
            }
            let metadata = canonical_metadata(&snapshot, context);
            inputs.publish_tagged_with_policy(
                &mut snapshot,
                edit.project(),
                sink,
                metadata,
                ResidentPublicationPolicy::DeviceInsert {
                    predecessor: installed,
                    hosted: plugin.is_some(),
                    confirmation: None,
                },
            )?;
            let receipt = insert_receipt(&snapshot, sink)?;
            Ok(Pending {
                edit,
                snapshot,
                receipt: Receipt(receipt),
                predecessor: installed,
                confirmation: None,
                document_intent,
                plugin: None,
            })
        })();
        if staged.is_err() {
            sink.finish_resident_effect_edit(
                hex_engine::AudioSourceContext {
                    editor_revision: context.editor_revision.saturating_add(1),
                    ..context
                },
                false,
            );
        }
        staged
    }
}

#[cfg(test)]
mod tests;
