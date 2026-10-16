//! Exact real-runtime demand and held-role transactions. Correlation is copied
//! from the same private State/runtime; it cannot be submitted independently.
use super::*;
use hex_stream::{ProtectionRole, ReadinessProgress, ReadinessStamp};
pub(crate) struct StreamInstall<'a> {
    pub(crate) domains: &'a mut [Option<DomainEnvelope>; 2],
    pub(crate) pending_play: bool,
    pub(crate) command_prefix: u64,
    pub(crate) frames: usize,
}
fn envelope(
    domains: &mut [Option<DomainEnvelope>; 2],
    token: AudioCandidateDomainToken,
) -> Result<&mut DomainEnvelope, hex_engine::Invalid> {
    domains
        .get_mut(token.slot)
        .and_then(Option::as_mut)
        .filter(|pages| pages.inner.token == token && pages.inner.loan.is_some())
        .ok_or(hex_engine::Invalid("exact stream page loan is unavailable"))
}
impl Workspace {
    fn insert(
        &mut self,
        index: usize,
        extent: StreamPageExtent,
    ) -> Result<(), hex_engine::Invalid> {
        let rows = &mut self.extents[index];
        match rows.binary_search_by_key(&(extent.source, extent.page_index), |row| {
            (row.source, row.page_index)
        }) {
            Ok(index) => {
                rows[index].start = rows[index].start.min(extent.start);
                rows[index].end = rows[index].end.max(extent.end);
            }
            Err(index) if rows.len() < self.limit => rows.insert(index, extent),
            Err(_) => return Err(hex_engine::Invalid("exact stream demand capacity exceeded")),
        }
        Ok(())
    }
    fn cancel_pending(
        &mut self,
        refs: [Option<AudioCandidateDomainToken>; 2],
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        if let Some(role) = self.role.take() {
            for token in refs.into_iter().flatten() {
                if let Some(stamp) = self.stamps[token.slot].take()
                    && let Ok(pages) = envelope(domains, token)
                    && let Some(loan) = pages.inner.loan.as_mut()
                {
                    let _ = loan.cancel(role, stamp.request_epoch);
                }
            }
        }
    }
}
impl State {
    pub(in crate::candidate_custody::eligible::publication) fn cancel_stream_pending(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        if let Some(workspace) = self.streaming.as_mut() {
            workspace.cancel_pending(self.domain_refs, domains);
        }
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_window(
        &mut self,
        previous: Option<&mut State>,
        request: &mut StreamInstall<'_>,
        role: ProtectionRole,
    ) -> Result<Option<hex_engine::StreamChunkWindow>, hex_engine::Invalid> {
        let refs = self.domain_refs;
        let workspace = self
            .streaming
            .as_mut()
            .filter(|w| w.valid())
            .ok_or(hex_engine::Invalid("stream demand workspace unavailable"))?;
        let runtime = self
            .runtime
            .as_mut()
            .ok_or(hex_engine::Invalid("stream runtime unavailable"))?;
        let context = runtime
            .source_context()
            .ok_or(hex_engine::Invalid("canonical stream context unavailable"))?;
        if context.project_generation != self.context_generation {
            return Err(hex_engine::Invalid("canonical stream context differs"));
        }
        let rate = runtime
            .sample_rate()
            .ok_or(hex_engine::Invalid("stream rate unavailable"))?;
        let predecessor = if role == ProtectionRole::Candidate {
            previous.as_ref().map(|old| old.stamp.owner)
        } else {
            workspace.predecessor
        };
        for rows in &mut workspace.extents {
            rows.clear();
        }
        let window = {
            let domains = &*request.domains;
            let mut visit = |extent: StreamPageExtent| {
                let index = refs
                    .iter()
                    .flatten()
                    .find_map(|token| {
                        domains
                            .get(token.slot)
                            .and_then(Option::as_ref)
                            .filter(|pages| pages.inner.token == *token)
                            .and_then(|pages| pages.inner.loan.as_ref())
                            .is_some_and(|loan| loan.covers_key(extent.source))
                            .then_some(token.slot)
                    })
                    .ok_or(hex_engine::Invalid(
                        "stream demand source lacks actual domain",
                    ))?;
                workspace.insert(index, extent)
            };
            if let Some(previous) = previous {
                runtime
                    .preview_stream_handoff(
                        previous
                            .runtime
                            .as_mut()
                            .ok_or(hex_engine::Invalid("stream predecessor unavailable"))?,
                        request.frames,
                        request.pending_play.then_some(true),
                        &mut visit,
                    )
                    .map_err(|_| hex_engine::Invalid("stream handoff demand refused"))?
            } else {
                runtime.visit_next_stream_chunk(
                    request.frames,
                    request.pending_play.then_some(true),
                    &mut visit,
                )?
            }
        };
        if request.pending_play && request.frames == 0 {
            return Ok(None);
        }
        if workspace.role.is_some_and(|old| old != role) {
            workspace.cancel_pending(refs, request.domains);
        }
        workspace.role = Some(role);
        let mut complete = true;
        for token in refs.into_iter().flatten() {
            let pages = envelope(request.domains, token)?;
            let rows = &workspace.extents[token.slot];
            let loan = pages
                .inner
                .loan
                .as_mut()
                .ok_or(hex_engine::Invalid("stream loan unavailable"))?;
            // Bounded by the caller's actual demand capacity. In particular a
            // worker CapacityExhausted receipt must release its pending key so
            // an old active window can retry after speculative pins clear.
            let mut failed_window = false;
            for _ in 0..workspace.limit {
                let Some(failure) = loan.take_failure() else {
                    break;
                };
                failed_window |= rows
                    .iter()
                    .any(|row| row.source == failure.key && row.page_index == failure.page_index);
                pages.inner.last_failure = Some(failure);
            }
            if failed_window {
                return Err(hex_engine::Invalid("stream worker readiness failed"));
            }
            if let Some(stamp) = workspace.stamps[token.slot] {
                let same = stamp.artifact == self.stamp.owner
                    && stamp.project_generation == context.project_generation
                    && stamp.editor_revision == context.editor_revision
                    && stamp.device_epoch == self.stamp.root
                    && stamp.device_rate == rate
                    && stamp.predecessor == predecessor
                    && stamp.admitted_command_prefix == request.command_prefix
                    && stamp.window == window;
                if !same || !loan.matches_intent(role, stamp, rows) {
                    let _ = loan.cancel(role, stamp.request_epoch);
                    workspace.stamps[token.slot] = None;
                }
            }
            if workspace.stamps[token.slot].is_none() {
                let epoch = pages
                    .inner
                    .epoch
                    .checked_add(1)
                    .ok_or(hex_engine::Invalid("stream request epoch exhausted"))?;
                let stamp = ReadinessStamp {
                    request_epoch: epoch,
                    artifact: self.stamp.owner,
                    project_generation: context.project_generation,
                    editor_revision: context.editor_revision,
                    device_epoch: self.stamp.root,
                    device_rate: rate,
                    predecessor,
                    admitted_command_prefix: request.command_prefix,
                    window,
                };
                loan.submit(role, stamp, rows)
                    .map_err(|_| hex_engine::Invalid("stream protection reservation refused"))?;
                pages.inner.epoch = epoch;
                workspace.stamps[token.slot] = Some(stamp);
            }
            let stamp = workspace.stamps[token.slot]
                .ok_or(hex_engine::Invalid("stream intent unavailable"))?;
            if !loan.is_protected(role, stamp, rows) {
                match loan
                    .service(role, stamp, workspace.limit, workspace.limit)
                    .map_err(|_| hex_engine::Invalid("stream readiness service refused"))?
                {
                    ReadinessProgress::Pending => complete = false,
                    ReadinessProgress::InstalledProtected => {
                        if !loan.acknowledge(role, stamp, rows) {
                            return Err(hex_engine::Invalid("stream readiness receipt differs"));
                        }
                    }
                }
            }
        }
        if !complete {
            return Ok(None);
        }
        for token in refs.into_iter().flatten() {
            let stamp = workspace.stamps[token.slot]
                .ok_or(hex_engine::Invalid("stream stamp unavailable"))?;
            if !envelope(request.domains, token)?
                .inner
                .loan
                .as_ref()
                .is_some_and(|loan| loan.can_promote(role, stamp, &workspace.extents[token.slot]))
            {
                return Err(hex_engine::Invalid("stream protected role cannot commit"));
            }
        }
        workspace.predecessor = predecessor;
        Ok(Some(window))
    }
    pub(in crate::candidate_custody::eligible::publication) fn prepare_stream_commit<'a>(
        &self,
        domains: &'a mut [Option<DomainEnvelope>; 2],
    ) -> Result<[Option<hex_stream::PreparedRolePromotion<'a>>; 2], hex_engine::Invalid> {
        let workspace = self
            .streaming
            .as_ref()
            .ok_or(hex_engine::Invalid("stream workspace unavailable"))?;
        let role = workspace
            .role
            .ok_or(hex_engine::Invalid("stream role unavailable"))?;
        let (left, right) = domains.split_at_mut(1);
        let prepare = |index: usize, pages: &'a mut Option<DomainEnvelope>| {
            let token = self
                .domain_refs
                .into_iter()
                .flatten()
                .find(|token| token.slot == index);
            let Some(token) = token else {
                return Ok(None);
            };
            let stamp =
                workspace.stamps[index].ok_or(hex_engine::Invalid("stream stamp unavailable"))?;
            let loan = pages
                .as_mut()
                .filter(|pages| pages.inner.token == token)
                .and_then(|pages| pages.inner.loan.as_mut())
                .ok_or(hex_engine::Invalid("stream exact loan unavailable"))?;
            loan.prepare_promotion(role, stamp, &workspace.extents[index])
                .map(Some)
                .ok_or(hex_engine::Invalid("stream protected commit refused"))
        };
        Ok([prepare(0, &mut left[0])?, prepare(1, &mut right[0])?])
    }
    pub(in crate::candidate_custody::eligible::publication) fn record_stream_commit(&mut self) {
        if let Some(workspace) = self.streaming.as_mut() {
            workspace.active = workspace.stamps;
            workspace.stamps = [None; 2];
            workspace.role = None;
        }
    }
}

impl State {
    pub(in crate::candidate_custody::eligible::publication) fn clear_stream_roles(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
    ) {
        self.cancel_stream_pending(domains);
        if let Some(workspace) = self.streaming.as_mut() {
            for token in self.domain_refs.into_iter().flatten() {
                if let Some(stamp) = workspace.active[token.slot].take()
                    && let Ok(pages) = envelope(domains, token)
                    && let Some(loan) = pages.inner.loan.as_mut()
                {
                    let _ = loan.cancel(ProtectionRole::Active, stamp.request_epoch);
                }
            }
        }
    }
}
struct PagePair<'a> {
    left: Option<hex_stream::ProtectedStreamPages<'a>>,
    right: Option<hex_stream::ProtectedStreamPages<'a>>,
}
impl hex_engine::MediaPages for PagePair<'_> {
    fn page(
        &mut self,
        key: hex_engine::StreamSourceKey,
        index: u64,
    ) -> Option<hex_engine::BorrowedMediaPage<'_>> {
        if let Some(page) = self.left.as_mut().and_then(|pages| pages.page(key, index)) {
            return Some(page);
        }
        self.right.as_mut()?.page(key, index)
    }
    fn request_page(&mut self, _key: hex_engine::StreamSourceKey, _index: u64) {}
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StreamProgress {
    Complete,
    Buffering { committed_frames: usize },
}
pub(crate) struct StreamOutcome {
    pub(crate) progress: StreamProgress,
    pub(crate) play_applied: bool,
    pub(crate) readiness_refused: bool,
    pub(crate) fault: Option<hex_engine::Invalid>,
}
impl Active {
    pub(crate) fn contains_streamed_sources(&self) -> bool {
        self.state
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.contains_streamed_sources())
    }
    pub(crate) fn clear_stream_roles(&mut self, domains: &mut [Option<DomainEnvelope>; 2]) {
        self.state.clear_stream_roles(domains);
    }
    pub(crate) fn render_streamed(
        &mut self,
        domains: &mut [Option<DomainEnvelope>; 2],
        pending_play: bool,
        command_prefix: u64,
        output: &mut [hex_engine::Frame],
        mut observer: Option<&mut hex_engine::PostNodeObserver<'_>>,
    ) -> StreamOutcome {
        let mut committed = 0;
        let mut play_applied = false;
        while committed < output.len() {
            let mut request = StreamInstall {
                domains,
                pending_play: pending_play && !play_applied,
                command_prefix,
                frames: output.len() - committed,
            };
            let planned =
                self.state
                    .prepare_stream_window(None, &mut request, ProtectionRole::Transition);
            let window = match planned {
                Ok(Some(window)) => window,
                result => {
                    let refused = result.is_err();
                    self.state.cancel_stream_pending(domains);
                    output[committed..].fill([0.0; 2]);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: refused,
                        fault: None,
                    };
                }
            };
            let mut promotions = match self.state.prepare_stream_commit(domains) {
                Ok(promotions) => promotions,
                Err(_) => {
                    output[committed..].fill([0.0; 2]);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: true,
                        fault: None,
                    };
                }
            };
            if pending_play && !play_applied {
                let play = self
                    .state
                    .runtime
                    .as_mut()
                    .ok_or(hex_engine::Invalid("stream current owner missing"))
                    .and_then(|runtime| {
                        runtime
                            .apply_action(hex_engine::PlaybackAction::Transport(
                                hex_engine::PlaybackCommand::Play,
                            ))
                            .map_err(|_| hex_engine::Invalid("ready stream Play refused"))
                    });
                if let Err(error) = play {
                    output[committed..].fill([0.0; 2]);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: false,
                        fault: Some(error),
                    };
                }
                play_applied = true;
            }
            if window.frames == 0 {
                output[committed..].fill([0.0; 2]);
            } else {
                let (left, right) = promotions.split_at_mut(1);
                let mut pages = PagePair {
                    left: left[0].as_mut().map(|p| p.pages()),
                    right: right[0].as_mut().map(|p| p.pages()),
                };
                let observed = observer.is_some();
                let mut observe = |identity, offset, frames: &[hex_engine::Frame]| {
                    if let Some(observer) = observer.as_deref_mut() {
                        observer(identity, committed + offset, frames);
                    }
                };
                let observer: Option<&mut hex_engine::PostNodeObserver<'_>> =
                    if observed { Some(&mut observe) } else { None };
                let rendered = self
                    .state
                    .runtime
                    .as_mut()
                    .ok_or(hex_engine::Invalid("stream current owner missing"))
                    .and_then(|runtime| {
                        runtime.render_with_pages_and_post_node_observer(
                            &mut output[committed..committed + window.frames],
                            &mut pages,
                            observer,
                        )
                    });
                if let Err(error) = rendered {
                    output[committed..].fill([0.0; 2]);
                    return StreamOutcome {
                        progress: StreamProgress::Buffering {
                            committed_frames: committed,
                        },
                        play_applied,
                        readiness_refused: false,
                        fault: Some(error),
                    };
                }
                committed += window.frames;
            }
            for promotion in promotions.into_iter().flatten() {
                promotion.commit();
            }
            self.state.record_stream_commit();
            if window.frames == 0 {
                break;
            }
        }
        StreamOutcome {
            progress: StreamProgress::Complete,
            play_applied,
            readiness_refused: false,
            fault: None,
        }
    }
}
