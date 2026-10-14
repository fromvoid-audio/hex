use super::*;

impl Entry {
    fn restore(&mut self, originals: Vec<SourceDefinition>) {
        // Validate recovery mapping before any move. Unexpected private-state
        // mismatch keeps every owner in one bounded incomplete outcome.
        if originals.len() != self.input.paths.len()
            || self
                .input
                .paths
                .iter()
                .zip(&originals)
                .any(|(target, source)| target.0 != source.key || !target.1.as_os_str().is_empty())
        {
            self.incomplete = true;
            self.reason = Some("stream original mapping incomplete");
            self.unrestored = Some(originals);
            return;
        }
        for (target, source) in self.input.paths.iter_mut().zip(originals) {
            target.1 = source.path;
        }
    }
    pub(super) fn tick(&mut self, domains: &mut DomainIds) {
        let work = mem::replace(&mut self.work, Work::Outcome);
        match work {
            Work::Queued if self.canceled => {
                self.report.construction_cleanup = Some(ConstructionCleanup::NotStarted {
                    allocated_pages: 0,
                    reclaimed_pages: 0,
                });
            }
            Work::Queued => {
                let mut definitions = Vec::new();
                self.report.neutral_slot_bytes = match self
                    .input
                    .paths
                    .capacity()
                    .checked_mul(mem::size_of::<StreamedSourcePath>())
                {
                    Some(n) => n,
                    None => {
                        self.reason = Some("stream candidate capacity overflow");
                        return;
                    }
                };
                if before_definition_reserve().is_err()
                    || definitions
                        .try_reserve_exact(self.input.paths.len())
                        .is_err()
                {
                    self.reason = Some("stream candidate reserve failed");
                    return;
                }
                self.report.definition_slot_bytes = match definitions
                    .capacity()
                    .checked_mul(mem::size_of::<SourceDefinition>())
                {
                    Some(n) => n,
                    None => {
                        self.reason = Some("stream candidate capacity overflow");
                        return;
                    }
                };
                self.report.original_path_capacity_bytes = match self
                    .input
                    .paths
                    .iter()
                    .try_fold(0_usize, |n, p| n.checked_add(p.1.capacity()))
                {
                    Some(n) => n,
                    None => {
                        self.reason = Some("stream candidate capacity overflow");
                        return;
                    }
                };
                let minimum = self
                    .report
                    .neutral_slot_bytes
                    .checked_add(self.report.definition_slot_bytes)
                    .and_then(|n| n.checked_add(self.report.original_path_capacity_bytes))
                    .and_then(|n| n.checked_add(self.input.limits.compact.other_retained_bytes))
                    .and_then(|n| {
                        n.checked_add(self.input.limits.compact.other_construction_bytes)
                    });
                if minimum.is_none_or(|n| n > self.input.limits.max_construction_data_bytes) {
                    self.reason = Some("stream candidate capacity refused");
                    return;
                }
                let graph = self
                    .input
                    .playback
                    .as_ref()
                    .and_then(Playback::prepared_revision);
                // Validation occurred before root acceptance; immutable graph
                // and private path keys cannot change during queued ownership.
                for (key, path, limits) in &mut self.input.paths {
                    let metadata =
                        graph.and_then(|g| g.streamed_sources().find(|m| m.key() == *key));
                    if let Some(metadata) = metadata {
                        definitions.push(SourceDefinition::new(
                            mem::take(path),
                            metadata.clone(),
                            *limits,
                        ));
                    }
                }
                self.report.request_box_bytes = mem::size_of::<StreamConstructionRequest>();
                let mut input = CompactSourceInput::new(definitions);
                let mut reservation = self.input.limits.compact;
                let Some(retained) = reservation
                    .other_retained_bytes
                    .checked_add(self.report.neutral_slot_bytes)
                    .and_then(|n| n.checked_add(self.report.request_box_bytes))
                    .and_then(|n| n.checked_add(self.report.fixed_slot_bytes))
                else {
                    self.restore(input.into_originals());
                    self.reason = Some("stream candidate capacity overflow");
                    return;
                };
                reservation.other_retained_bytes = retained;
                if input
                    .try_compact(self.input.limits.stream, reservation)
                    .is_err()
                {
                    self.restore(input.into_originals());
                    self.reason = Some("stream candidate compact refusal");
                    return;
                }
                self.work = Work::Request(Box::new(StreamConstructionRequest::new(
                    input,
                    self.input.limits.stream,
                    self.input.limits.max_construction_data_bytes,
                )));
                if let Work::Request(request) = &mut self.work {
                    let _started = request.start();
                }
            }
            Work::Request(mut request) => {
                if self.canceled {
                    let _cancel = request.cancel();
                }
                let state = request.poll();
                self.report.construction = Some(request.capacity_report());
                if state == ConstructionState::Failed {
                    if let Ok(Some(recovered)) = request.try_recover() {
                        self.report.construction_cleanup = Some(recovered.cleanup);
                        self.restore(recovered.originals);
                        self.reason.get_or_insert("stream construction failed");
                    } else {
                        self.work = Work::Request(request);
                    }
                } else if state == ConstructionState::CatalogVerified && !self.canceled {
                    match request.try_finish() {
                        Ok(Some(finished)) => {
                            let (originals, runtime, capacity) = finished.into_parts();
                            self.restore(originals);
                            self.report.construction = Some(capacity);
                            self.work = Work::Runtime(runtime);
                        }
                        _ => self.work = Work::Request(request),
                    }
                } else {
                    self.work = Work::Request(request);
                }
            }
            Work::Runtime(runtime) => match runtime.split() {
                Ok((control, pages)) => self.work = Work::Pair(control, pages),
                Err(runtime) => {
                    self.canceled = true;
                    self.reason.get_or_insert("stream runtime split refused");
                    self.work = Work::Runtime(runtime);
                }
            },
            Work::Pair(control, pages) if self.canceled => self.clean_pair(control, pages),
            Work::Pair(control, pages) => {
                let lease = match control.catalog_lease() {
                    Ok(lease) => lease,
                    Err(_) => {
                        self.canceled = true;
                        self.reason = Some("stream catalog unavailable");
                        self.work = Work::Pair(control, pages);
                        return;
                    }
                };
                let endpoint = match VerifiedPageEndpoint::new(pages, lease) {
                    Ok(e) => e,
                    Err(error) => {
                        let (pages, lease) = error.into_parts();
                        drop(lease);
                        self.canceled = true;
                        self.reason = Some("stream endpoint pair mismatch");
                        self.work = Work::Pair(control, pages);
                        return;
                    }
                };
                let endpoint =
                    match PageReadiness::new(endpoint, self.input.limits.readiness_extents) {
                        Ok(e) => e,
                        Err(error) => {
                            let (endpoint, _, observation) = error.into_parts();
                            self.report.readiness_bytes = observation.bytes().unwrap_or(usize::MAX);
                            let (pages, lease) = endpoint.into_parts();
                            drop(lease);
                            self.canceled = true;
                            self.reason = Some("stream readiness reserve refused");
                            self.work = Work::Pair(control, pages);
                            return;
                        }
                    };
                self.report.readiness_bytes = endpoint
                    .allocation_footprint()
                    .bytes()
                    .and_then(|n| n.checked_add(mem::size_of::<MediaDomainInput>()))
                    .unwrap_or(usize::MAX);
                let bound = self
                    .report
                    .construction
                    .and_then(|r| r.finalized_data_bound_bytes())
                    .and_then(|n| n.checked_add(self.report.readiness_bytes));
                self.report.prepared_data_bound_bytes = bound;
                if bound.is_none_or(|n| n > self.input.limits.max_prepared_data_bytes) {
                    let (pages, lease) = endpoint.into_endpoint().into_parts();
                    drop(lease);
                    self.canceled = true;
                    self.reason = Some("stream prepared capacity refused");
                    self.work = Work::Pair(control, pages);
                    return;
                }
                let graph = self
                    .input
                    .playback
                    .as_ref()
                    .and_then(Playback::prepared_revision)
                    .map(Arc::clone);
                let verified = match graph.and_then(|graph| {
                    VerifiedStreamRevision::new(graph, endpoint.catalog().clone()).ok()
                }) {
                    Some(v) => v,
                    None => {
                        let (pages, lease) = endpoint.into_endpoint().into_parts();
                        drop(lease);
                        self.canceled = true;
                        self.reason = Some("stream graph catalog mismatch");
                        self.work = Work::Pair(control, pages);
                        return;
                    }
                };
                let id = match domains.advance() {
                    Ok(id) => id,
                    Err(_) => {
                        drop(verified);
                        let (pages, lease) = endpoint.into_endpoint().into_parts();
                        drop(lease);
                        self.canceled = true;
                        self.reason = Some("stream domain identity exhausted");
                        self.work = Work::Pair(control, pages);
                        return;
                    }
                };
                let Some(playback) = self.input.playback.take() else {
                    drop(verified);
                    let (pages, lease) = endpoint.into_endpoint().into_parts();
                    drop(lease);
                    self.canceled = true;
                    self.incomplete = true;
                    self.reason = Some("stream playback owner missing");
                    self.work = Work::Pair(control, pages);
                    return;
                };
                self.work = Work::Ready(StreamPublicationRequest {
                    playback,
                    verified,
                    source: StreamPublicationSource::New(Box::new(MediaDomainInput {
                        id,
                        control,
                        endpoint,
                        retained_leases: Vec::new(),
                    })),
                    policy: PublicationPolicy::InheritRuntime,
                    context: StreamingContext {
                        project_generation: self.input.identity.project_generation,
                        editor_revision: self.input.identity.editor_revision,
                        device_epoch: self.token.device_epoch,
                        timeline_epoch: 0,
                    },
                });
            }
            Work::Ready(request) if self.canceled => {
                let StreamPublicationRequest {
                    verified,
                    source,
                    playback,
                    ..
                } = request;
                drop((verified, playback));
                if let StreamPublicationSource::New(input) = source {
                    let MediaDomainInput {
                        control,
                        endpoint,
                        retained_leases,
                        ..
                    } = *input;
                    drop(retained_leases);
                    let (pages, lease) = endpoint.into_endpoint().into_parts();
                    drop(lease);
                    self.clean_pair(control, pages);
                }
            }
            other => self.work = other,
        }
    }
    fn clean_pair(&mut self, control: StreamControl, pages: StreamCallbackPages) {
        match control.shutdown_reported(pages) {
            Ok(report) => self.report.shutdown = Some(report),
            Err(error) => {
                if let Ok((control, pages)) = error.into_parts() {
                    self.work = Work::Pair(control, pages);
                } else {
                    self.incomplete = true;
                    self.reason = Some("stream shutdown incomplete");
                }
            }
        }
    }
}
