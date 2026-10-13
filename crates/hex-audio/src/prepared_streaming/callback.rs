use super::*;

impl StreamingCallback {
    pub(in crate::prepared) fn install_publication(
        &mut self,
        publication: &mut Publication,
    ) -> bool {
        let Some(binding) = publication.stream_source.as_mut() else {
            return true;
        };
        let Some(batch) = self.batch.as_mut() else {
            return false;
        };
        if let Some(admission) = binding.admission.take() {
            if let Err(admission) = batch.install_domain(admission) {
                binding.admission = Some(admission);
                return false;
            }
            if let Some(index) = batch.domain_slot(binding.domain) {
                self.failures[index] = None;
            }
        }
        batch
            .readiness_mut(binding.domain)
            .is_some_and(|readiness| readiness.catalog().same_catalog(&binding.catalog))
    }
    pub(in crate::prepared) fn pump_returns(&mut self) {
        let Some(batch) = self.batch.as_mut() else {
            return;
        };
        for flag in self.return_requests.iter() {
            if let Ok(id) = hex_stream::DomainId::new(flag.load(Acquire)) {
                let _ = batch.try_return_domain(id);
            }
        }
    }
    fn check_worker_failure(&mut self, publication: &Publication) -> Result<(), Invalid> {
        let binding = publication.stream_source.as_ref().expect("stream binding");
        let batch = self
            .batch
            .as_mut()
            .ok_or(Invalid("stream batch unavailable"))?;
        let index = batch
            .domain_slot(binding.domain)
            .ok_or(Invalid("stream domain unavailable"))?;
        if self.failures[index].is_none() {
            if let Some(source) = publication
                .playback
                .prepared_revision()
                .and_then(|revision| revision.streamed_sources().next())
            {
                let mut failure = [hex_stream::StreamFailure {
                    key: source.key(),
                    page_index: 0,
                    kind: hex_stream::StreamFailureKind::Cancelled,
                }];
                if batch
                    .readiness_mut(binding.domain)
                    .expect("validated domain")
                    .pages()
                    .take_failures(&mut failure)
                    != 0
                {
                    self.failures[index] = Some(failure[0]);
                }
            }
        }
        if let Some(failure) = self.failures[index] {
            return Err(Invalid(match failure.kind {
                hex_stream::StreamFailureKind::SourceChanged => {
                    "stream source changed during verified page delivery"
                }
                hex_stream::StreamFailureKind::Cancelled => "stream page delivery was cancelled",
                hex_stream::StreamFailureKind::DecodeFailed => "stream page decoding failed",
                hex_stream::StreamFailureKind::UnknownSource => "stream page source is unknown",
                hex_stream::StreamFailureKind::CapacityExhausted => {
                    "stream page delivery capacity exhausted"
                }
            }));
        }
        Ok(())
    }
    fn context(publication: &Publication, command_prefix: u64) -> ReadinessStamp {
        let binding = publication.stream_source.as_ref().expect("stream binding");
        ReadinessStamp {
            request_epoch: 0,
            artifact: publication.revision,
            project_generation: binding.context.project_generation,
            editor_revision: binding.context.editor_revision,
            device_epoch: binding.context.device_epoch,
            project_rate: publication.playback.sample_rate(),
            timeline_epoch: binding.context.timeline_epoch,
            performance_frame: 0,
            arrangement_frame: 0,
            window_frames: 0,
            active_revision: publication.revision,
            route_generation: 0,
            admitted_command_prefix: command_prefix,
        }
    }
    pub(in crate::prepared) fn candidate_ready(
        &mut self,
        next: &mut Publication,
        previous: Option<&mut Publication>,
        prefix: u64,
    ) -> Result<bool, Invalid> {
        if next.stream_source.is_none() {
            return Ok(true);
        }
        if !self.install_publication(next) {
            return Err(Invalid("stream endpoint installation refused"));
        }
        self.check_worker_failure(next)?;
        self.candidate.clear();
        let predecessor_revision = previous.as_ref().map_or(0, |previous| previous.revision);
        let window = if matches!(next.policy, PublicationPolicy::InheritRuntime) {
            if let Some(previous) = previous {
                next.playback
                    .preview_stream_handoff(&mut previous.playback, 256, &mut |extent| {
                        self.candidate.visit(extent)
                    })
                    .map_err(|_| Invalid("stream handoff demand refused"))?
            } else {
                next.playback
                    .visit_next_stream_chunk(256, &mut |extent| self.candidate.visit(extent))?
            }
        } else {
            next.playback
                .visit_next_stream_chunk(256, &mut |extent| self.candidate.visit(extent))?
        };
        let mut context = Self::context(next, prefix);
        context.active_revision = predecessor_revision;
        let binding = next.stream_source.as_ref().expect("stream binding");
        let readiness = self
            .batch
            .as_mut()
            .and_then(|batch| batch.readiness_mut(binding.domain))
            .ok_or(Invalid("stream domain unavailable"))?;
        self.candidate.install(
            readiness,
            &binding.catalog,
            ProtectionRole::Candidate,
            &mut self.epochs,
            context,
            window,
        )
    }
    pub(in crate::prepared) fn render(
        &mut self,
        active: &mut Publication,
        output: &mut [Frame],
        mut observer: Option<&mut hex_engine::PostNodeObserver<'_>>,
        prefix: u64,
    ) -> Result<bool, Invalid> {
        self.check_worker_failure(active)?;
        let context = Self::context(active, prefix);
        let binding = active.stream_source.as_ref().expect("stream binding");
        let readiness = self
            .batch
            .as_mut()
            .and_then(|batch| batch.readiness_mut(binding.domain))
            .ok_or(Invalid("stream domain unavailable"))?;
        let mut offset = 0;
        while offset < output.len() {
            self.active.clear();
            let window = active
                .playback
                .visit_next_stream_chunk(output.len() - offset, &mut |extent| {
                    self.active.visit(extent)
                })?;
            if window.frames == 0 {
                output[offset..].fill([0.0; 2]);
                return Ok(false);
            }
            if !self.active.install(
                readiness,
                &binding.catalog,
                ProtectionRole::Active,
                &mut self.epochs,
                context,
                window,
            )? {
                output[offset..].fill([0.0; 2]);
                return Ok(true);
            }
            let end = offset
                .checked_add(window.frames)
                .ok_or(Invalid("stream chunk output overflow"))?;
            let mut observe = |identity, frame, samples: &[Frame]| {
                if let Some(observer) = observer.as_deref_mut() {
                    observer(identity, offset + frame, samples);
                }
            };
            active.playback.render_with_pages_and_post_node_observer(
                &mut output[offset..end],
                readiness.pages(),
                Some(&mut observe),
            )?;
            offset = end;
        }
        Ok(false)
    }
}
