//! Verified-streaming audio candidate domain, preparation, and submission state
//! owned by `NativeAudioSink`.
use super::{
    NativeAudioSink, resident,
    verified_clip_move::VerifiedClipControl,
    verified_gain::{PreviewWatermark, VerifiedKey, VerifiedSource},
    verified_graph_edit,
};
use crate::{
    audio_policy::{AudioPolicy, MAX_RESIDENT_FRAMES, RUNTIME_CELLS},
    native_sources::DomainSources,
};
use hex_audio::{
    AudioCandidateDomainToken, AudioCandidatePreparationToken, AudioCandidateTicket,
    WorkerPreparedOutput,
};
use hex_session::{AudioSourceBinding, service::CanonicalAudioSourceSnapshot};
use hex_ui::EditorPublicationState;
use std::sync::{Arc, atomic::Ordering::Acquire};

pub(super) struct Domain {
    pub(super) token: AudioCandidateDomainToken,
    pub(super) sources: DomainSources,
    pub(super) ready: bool,
    pub(super) retiring: bool,
}

impl Domain {
    pub(super) fn request_retirement(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
    ) {
        // Local cleanup intent survives Root's transition into Cleaning or
        // Complete (including verification failure before our first request).
        // Only exact report recovery removes this adapter record.
        self.retiring = true;
        let _ = control.retire_audio_candidate_domain(self.token);
    }
}

pub(crate) struct Submitted {
    pub(super) resident_copy_frames: u64,
    // Correlate the actual install ACK with this exact submitted project and
    // overlay. The weak identity does not retain its media or project payload.
    pub(super) source: hex_audio::prepared::AppliedAudioSource,
    pub(super) canonical: std::sync::Weak<CanonicalAudioSourceSnapshot>,
    pub(crate) key: VerifiedKey,
    pub(super) ticket: AudioCandidateTicket,
    pub(super) domain: Option<AudioCandidateDomainToken>,
}

pub(crate) struct Preparation {
    pub(super) resident_copy_frames: u64,
    pub(super) builtin_effects: bool,
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    pub(super) hosted_effects: Vec<hex_audio::plugin_effects::EffectBinding>,
    pub(crate) token: AudioCandidatePreparationToken,
    pub(crate) snapshot: VerifiedSource,
    pub(super) catalog: u64,
    pub(crate) domain: Option<AudioCandidateDomainToken>,
    pub(crate) bound: bool,
    pub(crate) built: bool,
    pub(super) cancelling: bool,
}

pub(crate) struct VerifiedStreamingHost {
    pub(super) preview: Option<VerifiedSource>,
    pub(super) edit: Option<verified_graph_edit::Edit>,
    pub(super) clip_control: VerifiedClipControl,
    pub(super) preview_watermark: Option<PreviewWatermark>,
    pub(super) last: Option<VerifiedKey>,
    pub(crate) blocked: Option<VerifiedKey>,
    pub(super) next_catalog: u64,
    pub(super) domains: [Option<Domain>; 2],
    pub(crate) preparation: Option<Preparation>,
    pub(crate) submitted: [Option<Submitted>; RUNTIME_CELLS],
    pub(super) pcm_frames: u64,
}

impl Default for VerifiedStreamingHost {
    fn default() -> Self {
        Self {
            preview: None,
            edit: None,
            clip_control: Default::default(),
            preview_watermark: None,
            last: None,
            blocked: None,
            next_catalog: 1,
            domains: std::array::from_fn(|_| None),
            preparation: None,
            submitted: std::array::from_fn(|_| None),
            pcm_frames: 0,
        }
    }
}

impl VerifiedStreamingHost {
    pub(super) fn allocate_catalog(&mut self) -> Option<u64> {
        let current = self.next_catalog;
        self.next_catalog = current.checked_add(1)?;
        Some(current)
    }

    pub(super) fn clear_for_relinquish(&mut self) {
        if matches!(
            self.preview,
            Some(VerifiedSource::ArrangementGain(_) | VerifiedSource::ArrangementPlayStart(_))
        ) {
            self.preview = None;
        }
        if let Some(insert) = self.edit.take() {
            insert
                .attempt
                .refused("audio output closed · restart audio to recover".into());
        }
        self.preparation = None;
        self.domains = std::array::from_fn(|_| None);
        self.submitted = std::array::from_fn(|_| None);
        self.last = None;
        self.blocked = None;
    }

    pub(super) fn is_quiescent(&self) -> bool {
        self.preparation.is_none()
    }
}

impl NativeAudioSink {
    pub(super) fn maintain_returns(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
    ) {
        control.reclaim();
        for submitted in &mut self.verified.submitted {
            if submitted
                .as_ref()
                .is_some_and(|s| control.recover_audio_candidate(&s.ticket).is_some())
            {
                if let Some(done) = submitted.as_ref() {
                    self.verified.pcm_frames = self
                        .verified
                        .pcm_frames
                        .checked_sub(done.resident_copy_frames)
                        .unwrap_or(u64::MAX);
                }
                *submitted = None; // only nonowning ticket; actual bank dropped here on worker
            }
        }
        for domain in &mut self.verified.domains {
            if domain.as_ref().is_some_and(|d| {
                d.retiring && control.recover_audio_candidate_domain(d.token).is_some()
            }) {
                *domain = None;
            }
        }
    }

    pub(crate) fn drive_verified(&mut self, output: &mut WorkerPreparedOutput) {
        let Some(control) = output.publication.control() else {
            return;
        };
        self.settle_verified_reopen(control);
        self.maintain_returns(control);
        let edit = self.verified.edit.clone();
        if edit.as_ref().is_some_and(|edit| !edit.attempt.preparing()) {
            // The editor owns the terminal. Keep the old desired source from
            // republishing across the receipt while it decides or commits.
            return;
        }
        let Some(snapshot) = edit
            .as_ref()
            .map(|edit| VerifiedSource::Canonical(Arc::clone(edit.attempt.source())))
            .or_else(|| self.verified.preview.clone())
            .or_else(|| {
                self.desired
                    .as_ref()
                    .map(|source| VerifiedSource::Canonical(Arc::clone(source)))
            })
        else {
            return;
        };
        let context = snapshot.context();
        let key = snapshot.key();
        if !control.audio_candidate_claims_available() {
            if let Some(edit) = edit.as_ref() {
                edit.attempt.refused(
                    "audio ownership cleanup is incomplete · save project and restart hex".into(),
                );
                return;
            }
            self.plugins.finish_reopen(context, false);
            self.observe(
                context,
                "incomplete",
                EditorPublicationState::Fault(
                    "audio ownership cleanup is incomplete; save project and restart hex".into(),
                ),
            );
            return;
        }
        if control.status.device_fault.load(Acquire) || control.status.render_fault.load(Acquire) {
            if let Some(edit) = edit.as_ref() {
                edit.attempt
                    .refused("audio output closed · restart audio to recover".into());
                return;
            }
            self.plugins.finish_reopen(context, false);
            self.observe(
                context,
                "fault",
                EditorPublicationState::Fault("audio unavailable; restart audio to retry".into()),
            );
            return;
        }
        // An output fault must be visible even while a fresh VST3 endpoint
        // is waiting for its callback loan.
        if self.plugins.hold_reopen(context) && self.plugins.reopen_ready() != Some(context) {
            return;
        }
        // A queued, uninstalled older canonical request is obsolete. TooLate
        // preserves a genuinely installed predecessor and its normal archive.
        for submitted in self.verified.submitted.iter().flatten() {
            if submitted.key != key {
                control.cancel_audio_candidate(&submitted.ticket);
            }
        }
        if self
            .verified
            .preparation
            .as_ref()
            .is_some_and(|p| p.snapshot.key() != key)
            && let Some(p) = self.verified.preparation.as_mut()
        {
            control.cancel_audio_candidate_preparation(p.token);
            p.cancelling = true;
            if let Some(token) = p.domain
                && let Some(domain) = self
                    .verified
                    .domains
                    .iter_mut()
                    .flatten()
                    .find(|d| d.token == token && !d.ready)
            {
                domain.request_retirement(control);
            }
        }
        if self
            .verified
            .preparation
            .as_ref()
            .is_some_and(|p| p.cancelling)
        {
            let token = self.verified.preparation.as_ref().map(|p| p.token);
            if token.is_some_and(|token| {
                matches!(
                    control.prepare_audio_candidate_on_control(token, |_| Ok(())),
                    Err(reason) if reason != "candidate preparation is busy"
                )
            }) {
                self.verified.preparation = None;
            } else {
                if self.verified.blocked != Some(key) {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                }
                return;
            }
        }
        if edit.is_none() && (self.verified.blocked == Some(key) || self.verified.last == Some(key))
        {
            return;
        }
        let Ok(policy) = self.policy.as_ref().copied() else {
            return;
        };
        if self.verified.preparation.is_none() {
            if self.verified.submitted.iter().all(Option::is_some) {
                self.observe(context, "busy", EditorPublicationState::Busy);
                return;
            }
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let hosted_effects = match self.plugins.verified_effect_bindings(
                snapshot.canonical().effect_preparation(),
                edit.as_ref()
                    .and_then(|edit| edit.attempt.proposal_control()),
                edit.as_ref()
                    .is_some_and(|edit| edit.attempt.prepares_hosted_sources()),
            ) {
                Ok(bindings) => bindings,
                Err(error) => {
                    self.refuse_verified_source(&snapshot, error);
                    return;
                }
            };
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let hosted_quote = (!hosted_effects.is_empty()).then(|| {
                control.with_native_effect_definition(&hosted_effects, |definition| {
                    hex_engine::AudioCandidateEffects::with_host_definitions(
                        &[definition],
                        |effects| Ok(snapshot.quote_with_effects(effects)),
                    )
                    .map_err(|error| error.to_string())?
                    .map(|quote| (quote, true))
                    .map_err(|error| format!("this audio revision is unsupported: {error:?}"))
                })
            });
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let quote_result = hosted_quote.unwrap_or_else(|| {
                snapshot
                    .candidate_reservation_quote()
                    .map_err(|error| format!("this audio revision is unsupported: {error:?}"))
            });
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let quote_result = snapshot
                .candidate_reservation_quote()
                .map_err(|error| format!("this audio revision is unsupported: {error:?}"));
            let (quote, builtin_effects) = match quote_result {
                Ok(quote) => quote,
                Err(error)
                    if matches!(
                        error.as_str(),
                        "native effect preparation is busy" | "native effect selection is busy"
                    ) =>
                {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                }
                Err(error) => {
                    self.refuse_verified_source(&snapshot, error);
                    return;
                }
            };
            let input_bytes =
                snapshot
                    .canonical()
                    .required_sources()
                    .try_fold(0usize, |n, (_, source)| {
                        let bytes = match source {
                            AudioSourceBinding::Resident(audio) => audio
                                .frames()
                                .len()
                                .checked_mul(std::mem::size_of::<hex_engine::Frame>())?,
                            _ => 0,
                        };
                        n.checked_add(bytes)
                    });
            let storage = input_bytes.and_then(|bytes| {
                let maximum = usize::try_from(MAX_RESIDENT_FRAMES)
                    .ok()?
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())?;
                let prepared = usize::try_from(quote.prepared_pcm_frames)
                    .ok()?
                    .checked_mul(std::mem::size_of::<hex_engine::Frame>())?;
                (bytes.checked_add(prepared)? <= maximum).then_some(())?;
                quote.storage_reservation(bytes).ok()
            });
            let Some(storage) = storage else {
                self.refuse_verified_source(
                    &snapshot,
                    "audio preparation reservation exceeded".into(),
                );
                return;
            };
            let existing = self.verified.domains.iter().flatten().find(|d| {
                d.ready && !d.retiring && d.sources.covers(snapshot.canonical(), d.token, control)
            });
            let domain = existing.map(|d| d.token);
            let catalog = if let Some(d) = existing {
                d.sources.catalog_generation()
            } else {
                let Some(current) = self.verified.allocate_catalog() else {
                    self.refuse_verified_source(&snapshot, "source generation exhausted".into());
                    return;
                };
                current
            };
            let domains = domain.into_iter().collect();
            match control.reserve_audio_candidate_preparation(
                hex_audio::AudioCandidatePreparationReservation {
                    source_capacity: quote.source_capacity,
                    storage,
                    member_rows: quote.member_rows,
                    transfer_rows: quote.transfer_rows,
                    handle_rows: quote.handle_rows,
                },
                domains,
            ) {
                Ok(token) => {
                    let copied_frames = u64::try_from(
                        input_bytes.unwrap_or(usize::MAX)
                            / std::mem::size_of::<hex_engine::Frame>(),
                    )
                    .ok()
                    .and_then(|frames| frames.checked_add(quote.prepared_pcm_frames))
                    .unwrap_or(u64::MAX);
                    self.verified.pcm_frames =
                        self.verified.pcm_frames.saturating_add(copied_frames);
                    self.verified.preparation = Some(Preparation {
                        resident_copy_frames: copied_frames,
                        builtin_effects,
                        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
                        hosted_effects,
                        token,
                        snapshot: snapshot.clone(),
                        catalog,
                        domain,
                        bound: domain.is_some(),
                        built: false,
                        cancelling: false,
                    })
                }
                Err(_) => {
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                }
            }
        }
        self.advance_preparation(control, policy, edit.as_ref());
    }

    fn advance_preparation(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        policy: AudioPolicy,
        edit: Option<&verified_graph_edit::Edit>,
    ) {
        let Some(mut pending) = self.verified.preparation.take() else {
            return;
        };
        let context = pending.snapshot.context();
        if !pending.bound {
            if pending.domain.is_none() {
                let destination = self.verified.domains.iter().position(Option::is_none);
                let Some(index) = destination else {
                    // Prefer a cached domain with no submitted/preparing user.
                    // Root remains authoritative: retiring never drops a live use.
                    if let Some(domain) = self.verified.domains.iter_mut().flatten().find(|d| {
                        !d.retiring
                            && !self
                                .verified
                                .submitted
                                .iter()
                                .flatten()
                                .any(|s| s.domain == Some(d.token))
                    }) {
                        domain.request_retirement(control);
                    }
                    self.verified.preparation = Some(pending);
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                };
                let projection = DomainSources::capture(
                    pending.snapshot.canonical(),
                    pending.catalog,
                )
                .and_then(|sources| {
                    sources
                        .prepare_for_snapshot(
                            pending.snapshot.canonical(),
                            policy.stream,
                            policy.granular,
                            || match edit {
                                Some(edit) => {
                                    edit.cancellation.as_ref().is_some_and(|c| c.is_cancelled())
                                }
                                // The desired snapshot already committed. Document
                                // intent supersedes request-owned work; replacement of
                                // this source is driven by the next committed snapshot.
                                None => self
                                    .request_cancellation
                                    .as_ref()
                                    .is_some_and(|c| c.is_shutting_down()),
                            },
                        )
                        .map(|(definitions, granular)| (sources, definitions, granular))
                });
                let (sources, definitions, granular) = match projection {
                    Ok(pair) => pair,
                    Err(error) => {
                        self.fail_preparation(control, pending, error);
                        return;
                    }
                };
                let started = match granular {
                    Some(bank) => control.begin_audio_candidate_granular_domain_preparation(
                        policy.stream,
                        definitions,
                        bank,
                        hex_stream::StreamReadinessReservation {
                            // The existing pool retains one ordinary free page and
                            // the granular worker may hold one original-pool loan.
                            extents_per_role: policy
                                .readiness
                                .extents_per_role
                                .min(policy.stream.page_buffers.saturating_sub(2)),
                        },
                    ),
                    None => self.begin_verified_file_domain(control, policy, definitions),
                };
                match started {
                    Ok(token) => {
                        pending.domain = Some(token);
                        self.verified.domains[index] = Some(Domain {
                            token,
                            sources,
                            ready: false,
                            retiring: false,
                        });
                    }
                    Err(hex_audio::AudioCandidateDomainStartError::Busy(_)) => {
                        self.verified.preparation = Some(pending);
                        self.observe(context, "busy", EditorPublicationState::Busy);
                        return;
                    }
                    Err(error) => {
                        self.fail_preparation(
                            control,
                            pending,
                            format!("source preparation refused: {error:?}"),
                        );
                        return;
                    }
                }
            }
            let Some(token) = pending.domain else {
                self.fail_preparation(control, pending, "source association missing".into());
                return;
            };
            match control.poll_audio_candidate_domain_preparation(token) {
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Ready) => {
                    if let Some(record) = self
                        .verified
                        .domains
                        .iter_mut()
                        .flatten()
                        .find(|d| d.token == token)
                    {
                        record.ready = true;
                    }
                    match control
                        .bind_audio_candidate_preparation_domains(pending.token, Vec::from([token]))
                    {
                        Ok(()) => pending.bound = true,
                        Err(refused) => {
                            drop(refused.domains);
                            self.verified.preparation = Some(pending);
                            self.observe(context, "busy", EditorPublicationState::Busy);
                            return;
                        }
                    }
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying)
                | Err("candidate publication queue is full") => {
                    self.verified.preparation = Some(pending);
                    self.observe(context, "preparing", EditorPublicationState::Preparing);
                    return;
                }
                Ok(other) => {
                    self.fail_preparation(
                        control,
                        pending,
                        format!("source preparation unavailable: {other:?}"),
                    );
                    return;
                }
                Err(error) => {
                    self.fail_preparation(control, pending, error.into());
                    return;
                }
            }
        }
        if !pending.built {
            let record = pending.domain.and_then(|token| {
                self.verified
                    .domains
                    .iter()
                    .flatten()
                    .find(|d| d.token == token)
            });
            let prepare =
                |cell: &mut hex_engine::AudioCandidatePreparation,
                 effects: Option<&hex_engine::AudioCandidateEffects<'_, '_>>| {
                    for (asset, source) in pending.snapshot.canonical().required_sources() {
                        match source {
                            AudioSourceBinding::Resident(audio) => {
                                cell.copy_resident_source_on_control(asset, audio)?
                            }
                            AudioSourceBinding::VerifiedFile(_) => {
                                let metadata = record
                                    .and_then(|d| {
                                        d.sources.metadata_for(hex_engine::StreamSourceKey::new(
                                            pending.catalog,
                                            asset,
                                        ))
                                    })
                                    .ok_or(hex_engine::AudioStorageError::SourceMismatch)?;
                                cell.add_streamed_source_on_control(metadata.clone())?;
                            }
                        }
                    }
                    if let Some(domain) = record {
                        domain
                            .sources
                            .add_required_derived(pending.snapshot.canonical(), cell)?;
                    } else if pending
                        .snapshot
                        .canonical()
                        .required_plain_pitch_sources()
                        .next()
                        .is_some()
                        || pending
                            .snapshot
                            .canonical()
                            .required_repitch_sources()
                            .next()
                            .is_some()
                        || pending
                            .snapshot
                            .canonical()
                            .required_granular_roots(pending.catalog)
                            .next()
                            .is_some()
                    {
                        return Err(hex_engine::AudioStorageError::SourceMismatch);
                    }
                    if let Some(effects) = effects {
                        pending
                            .snapshot
                            .prepare_with_effects(pending.catalog, cell, effects)?;
                    } else {
                        pending.snapshot.prepare_candidate_on_control(
                            pending.catalog,
                            cell,
                            pending.builtin_effects,
                        )?;
                    }
                    if cell.resident_pcm_capacity_frames()? > pending.resident_copy_frames {
                        return Err(hex_engine::AudioStorageError::ReservationExceeded);
                    }
                    Ok(())
                };
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            let prepared = if pending.hosted_effects.is_empty() {
                control
                    .prepare_audio_candidate_on_control(pending.token, |cell| prepare(cell, None))
            } else {
                control.prepare_audio_candidate_with_native_sources_on_control(
                    pending.token,
                    &pending.hosted_effects,
                    |cell, effects| prepare(cell, Some(effects)),
                )
            };
            #[cfg(not(all(target_os = "macos", target_arch = "aarch64")))]
            let prepared = control
                .prepare_audio_candidate_on_control(pending.token, |cell| prepare(cell, None));
            match prepared {
                Ok(hex_audio::AudioCandidatePreparationProgress::Ready) => pending.built = true,
                Err("candidate preparation is busy" | "native effect selection is busy") => {
                    self.verified.preparation = Some(pending);
                    self.observe(context, "busy", EditorPublicationState::Busy);
                    return;
                }
                other => {
                    self.fail_preparation(
                        control,
                        pending,
                        format!("audio preparation refused: {other:?}"),
                    );
                    return;
                }
            }
        }
        let Some(index) = self.verified.submitted.iter().position(Option::is_none) else {
            self.verified.preparation = Some(pending);
            self.observe(context, "busy", EditorPublicationState::Busy);
            return;
        };
        let Some(output_identity) = control.output_identity() else {
            self.fail_preparation(
                control,
                pending,
                "verified audio output is unavailable".into(),
            );
            return;
        };
        let submitted = match edit.as_ref() {
            Some(edit) => edit
                .stage(control, pending.token, context)
                .map(|(ticket, receipt)| (ticket, Some(receipt))),
            None => control
                .stage_audio_candidate_preparation(pending.token, context.project_generation)
                .map(|ticket| (ticket, None)),
        };
        match submitted {
            Ok((ticket, receipt)) => {
                let revision = control.submitted_revision();
                self.verified.submitted[index] = Some(Submitted {
                    resident_copy_frames: pending.resident_copy_frames,
                    source: hex_audio::prepared::AppliedAudioSource {
                        context,
                        audio_revision: revision,
                        output_identity,
                    },
                    canonical: Arc::downgrade(pending.snapshot.canonical_arc()),
                    key: pending.snapshot.key(),
                    ticket,
                    domain: pending.domain,
                });
                if let Some((edit, receipt)) = edit.as_ref().zip(receipt) {
                    if edit.attempt.needs_tempo_cut() {
                        edit.attempt
                            .submitted(Box::new(resident::InsertReceipt::new(
                                hex_session::service::ResidentGraphSubmission {
                                    context,
                                    audio_revision: revision,
                                    output_identity: edit.predecessor.output_identity,
                                },
                                receipt,
                            )));
                    }
                    return;
                }
                self.last_context = Some(context);
                self.verified.last = Some(pending.snapshot.key());
                let state = match &pending.snapshot {
                    VerifiedSource::ArrangementGain(_) => {
                        EditorPublicationState::VerifiedArrangementGainSubmitted {
                            audio_revision: revision,
                        }
                    }
                    VerifiedSource::ArrangementPlayStart(_) => {
                        EditorPublicationState::VerifiedArrangementPlayStartSubmitted {
                            audio_revision: revision,
                        }
                    }
                    snapshot => match snapshot.preview_identity() {
                        Some(identity) => EditorPublicationState::VerifiedPreviewSubmitted {
                            audio_revision: revision,
                            identity,
                        },
                        None => EditorPublicationState::Submitted {
                            audio_revision: revision,
                        },
                    },
                };
                self.observe(context, "submitted", state);
            }
            // Transient admission states keep the built candidate for a later poll.
            // Streamed or native source construction may still be in flight or
            // retiring when the newer canonical state is ready to publish.
            Err(
                "playback publication already pending"
                | "candidate publication queue is full"
                | "candidate preparation is busy"
                | "source construction is not sealed"
                | "native source construction is not sealed",
            ) => {
                self.verified.preparation = Some(pending);
                self.observe(context, "busy", EditorPublicationState::Busy);
            }
            Err(error) => self.fail_preparation(control, pending, error.into()),
        }
    }

    fn begin_verified_file_domain(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        policy: AudioPolicy,
        definitions: Vec<hex_stream::SourceDefinition>,
    ) -> Result<AudioCandidateDomainToken, hex_audio::AudioCandidateDomainStartError> {
        #[cfg(all(test, feature = "allocation-probe"))]
        if let Some(definition) = definitions.first()
            && let Some(hold) = self.hooks.verified_page_hold.take()
        {
            let (gate, entered, release) =
                hex_stream::test_support::PageGate::new(hex_stream::PageKey {
                    source: definition.key,
                    page_index: hold.index,
                });
            if hold.armed.try_send((entered, release)).is_ok() {
                return control.begin_audio_candidate_domain_preparation_with_gate(
                    policy.stream,
                    definitions,
                    policy.readiness,
                    gate,
                );
            }
        }
        control.begin_audio_candidate_domain_preparation(
            policy.stream,
            definitions,
            policy.readiness,
        )
    }

    fn fail_preparation(
        &mut self,
        control: &mut hex_audio::prepared::PublicationControl,
        mut pending: Preparation,
        error: String,
    ) {
        control.cancel_audio_candidate_preparation(pending.token);
        pending.cancelling = true;
        if let Some(token) = pending.domain
            && let Some(domain) = self
                .verified
                .domains
                .iter_mut()
                .flatten()
                .find(|d| d.token == token && !d.ready)
        {
            domain.request_retirement(control);
        }
        self.refuse_verified_source(&pending.snapshot, error);
        self.verified.preparation = Some(pending);
    }

    fn refuse_verified_source(&mut self, source: &VerifiedSource, error: String) {
        if let Some(edit) = self.verified.edit.as_ref()
            && Arc::ptr_eq(edit.attempt.source(), source.canonical_arc())
        {
            edit.attempt.refused(error);
            return;
        }
        let context = source.context();
        self.plugins.finish_reopen(context, false);
        self.verified.blocked = Some(source.key());
        let state = match source {
            VerifiedSource::ArrangementGain(_) => {
                self.verified.preview = None;
                EditorPublicationState::VerifiedArrangementGainRefused(error)
            }
            VerifiedSource::ArrangementPlayStart(_) => {
                self.verified.preview = None;
                EditorPublicationState::VerifiedArrangementPlayStartRefused(error)
            }
            _ => {
                self.blocked = Some(context);
                EditorPublicationState::Refused(error)
            }
        };
        self.observe(context, "refused", state);
    }

    fn settle_verified_reopen(&mut self, control: &hex_audio::prepared::PublicationControl) {
        let Some(context) = self.plugins.reopen_ready() else {
            return;
        };
        let Some(snapshot) = self
            .desired
            .as_ref()
            .filter(|source| source.context() == context)
        else {
            return;
        };
        let key = VerifiedKey::canonical(context);
        let Some(submission) = self.verified.submitted.iter().flatten().find(|submission| {
            submission.key == key
                && control.output_identity() == Some(submission.source.output_identity)
                && std::ptr::eq(submission.canonical.as_ptr(), Arc::as_ptr(snapshot))
        }) else {
            return;
        };
        if control.status.applied_audio_source() == Some(submission.source) {
            self.plugins.finish_reopen(context, true);
        } else if let Some((revision, reason)) = control.status.rejected_revision()
            && revision == submission.source.audio_revision
        {
            let source = VerifiedSource::Canonical(Arc::clone(snapshot));
            self.refuse_verified_source(&source, reason.message().into());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_engine::AudioSourceContext;

    #[test]
    fn catalog_generation_is_monotonic_and_overflow_guarded() {
        let mut host = VerifiedStreamingHost::default();
        assert!(host.is_quiescent());
        assert_eq!(host.allocate_catalog(), Some(1));
        assert_eq!(host.allocate_catalog(), Some(2));
        host.next_catalog = u64::MAX;
        assert_eq!(host.allocate_catalog(), None);
        assert_eq!(host.next_catalog, u64::MAX);
    }

    #[test]
    fn clear_for_relinquish_resets_tracking_keys_and_remains_quiescent() {
        let mut host = VerifiedStreamingHost::default();
        let key = VerifiedKey::canonical(AudioSourceContext {
            project_generation: 2,
            editor_revision: 5,
        });
        host.last = Some(key);
        host.blocked = Some(key);
        host.clear_for_relinquish();
        assert!(host.last.is_none());
        assert!(host.blocked.is_none());
        assert!(host.is_quiescent());
    }
}
