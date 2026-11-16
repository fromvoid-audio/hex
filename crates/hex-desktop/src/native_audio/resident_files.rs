//! Ordinary precommit file binding uses the existing two Root domain cells.
//! Polling, verification and cancellation stay on the Session worker. A sealed
//! catalog is insufficient: publication waits for actual callback delivery.
use super::*;
use hex_engine::playback::Playback;
use hex_session::service::{
    ResidentFileInput, ResidentFilePublication, ResidentFileSet, ResidentPublicationMetadata,
    ResidentPublicationPolicy,
};

impl NativeAudioSink {
    pub(super) fn prepare_resident_publication_files(
        &mut self,
        playback: &Playback,
        metadata: ResidentPublicationMetadata,
        policy: ResidentPublicationPolicy,
        files: Option<&ResidentFilePublication<'_>>,
    ) -> Result<(), String> {
        let Some(files) = files else {
            return if playback.contains_streamed_sources() {
                Err("ordinary file publication facts missing".into())
            } else {
                Ok(())
            };
        };
        // Authoring aliases may advance the revision while retaining these
        // exact dependencies. File capture counters are provenance, not a fence.
        let matches_sources = playback
            .streamed_sources_on_control()
            .is_ok_and(|mut sources| {
                sources.all(|source| files.sources.metadata().matches_source(source))
            });
        if files.sources.context().project_generation != metadata.context.project_generation
            || !matches_sources
        {
            return Err("ordinary file publication source scope differs".into());
        }
        let private_output = self.replacement_output.is_some();
        if self.output.is_none()
            || (!private_output
                && matches!(policy, ResidentPublicationPolicy::ReplaceDocument { .. }))
        {
            return Ok(());
        }
        loop {
            if files.shutdown.load(Acquire)
                || files.cancel.is_some_and(|cancel| cancel.load(Acquire))
                || (self.plugins.preparing_edit(metadata.context)
                    && self
                        .request_cancellation
                        .as_ref()
                        .is_some_and(hex_session::service::RequestCancellation::is_cancelled))
            {
                self.retire_resident_files(files.sources);
                return Err("ordinary file publication cancelled".into());
            }
            if private_output {
                self.check_replacement_deadline()?;
            }
            if self.poll_resident_files(files.sources)? {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }

    pub(super) fn poll_resident_files(&mut self, files: &ResidentFileSet) -> Result<bool, String> {
        let refs = files
            .files()
            .iter()
            .map(|(id, file)| (*id, file))
            .collect::<Vec<_>>();
        let input = ResidentFileInput {
            context: files.context(),
            semantic_source_epoch: files.semantic_source_epoch(),
            files: &refs,
            pitches: files.pitches(),
            repitches: files.repitches().requirements(),
        };
        let policy = self.policy.as_ref().map_err(Clone::clone)?;
        let control = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
            .ok_or("audio publication endpoint is closed")?;
        control.reclaim();
        if control.status.device_fault.load(Acquire)
            || control.status.render_fault.load(Acquire)
            || control.status.shutdown_complete.load(Acquire)
        {
            return Err("ordinary source preparation lost its audio output".into());
        }
        for row in &mut self.domains {
            if row.as_ref().is_some_and(|domain| {
                domain.retiring
                    && control
                        .recover_audio_candidate_domain(domain.token)
                        .is_some()
            }) {
                *row = None;
            }
        }
        let catalog = files.metadata().catalog_generation();
        let index = match self.domains.iter().position(|row| {
            row.as_ref().is_some_and(|domain| {
                !domain.retiring
                    && domain.sources.catalog_generation() == catalog
                    && domain.sources.matches_files(&input)
            })
        }) {
            Some(index) => index,
            None => {
                let Some(index) = self.domains.iter().position(Option::is_none) else {
                    // Retirement remains charged and cannot stop a domain while
                    // an actual ordinary graph retains a use. Never evict the
                    // newest cached domain while making room for its successor.
                    if !self.domains.iter().flatten().any(|domain| domain.retiring)
                        && let Some(domain) = self
                            .domains
                            .iter_mut()
                            .flatten()
                            .filter(|domain| {
                                control.audio_candidate_domain_has_no_graph_users(domain.token)
                            })
                            .min_by_key(|domain| domain.sources.catalog_generation())
                    {
                        domain.request_retirement(control);
                    }
                    return Ok(false);
                };
                let sources = DomainSources::capture_files(&input, catalog)?;
                let token = control
                    .begin_audio_candidate_domain_preparation(
                        policy.stream,
                        sources
                            .definitions_for_files(&input, policy.stream.max_kernel_table_bytes)?,
                        policy.readiness,
                    )
                    .map_err(|error| format!("ordinary source preparation refused: {error:?}"))?;
                self.domains[index] = Some(Domain {
                    token,
                    sources,
                    ready: false,
                    retiring: false,
                });
                index
            }
        };
        let domain = self.domains[index]
            .as_mut()
            .ok_or("ordinary source domain missing")?;
        if !domain.ready {
            match control.poll_audio_candidate_domain_preparation(domain.token) {
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Ready) => {
                    domain.ready = true
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Verifying)
                | Err("candidate publication queue is full") => return Ok(false),
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Retiring) => {
                    return Err("ordinary source verification is retiring".into());
                }
                Ok(hex_audio::AudioCandidateDomainPreparationProgress::Refused(error)) => {
                    domain.request_retirement(control);
                    return Err(format!("ordinary source verification refused: {error:?}"));
                }
                Err(error) => {
                    domain.request_retirement(control);
                    return Err(format!("ordinary source verification refused: {error}"));
                }
            }
        }
        if !domain.sources.covers_files(&input, domain.token, control) {
            domain.request_retirement(control);
            return Err("ordinary retained source catalog differs".into());
        }
        Ok(control.audio_candidate_domain_endpoint_is_installed(domain.token))
    }
    pub(super) fn retire_resident_files(&mut self, files: &ResidentFileSet) {
        let Some(control) = self
            .output
            .as_mut()
            .and_then(|output| output.publication.control())
        else {
            return;
        };
        for domain in self.domains.iter_mut().flatten().filter(|domain| {
            domain.sources.catalog_generation() == files.metadata().catalog_generation()
        }) {
            domain.request_retirement(control);
        }
    }
}
