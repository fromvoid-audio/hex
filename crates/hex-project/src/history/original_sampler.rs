//! One pending candidate in the existing Editor. This is not audio admission.
//! The audited native caller must price clone/apply/backing growth BEFORE stage.
use super::*;
use std::{
    any::Any,
    cell::RefMut,
    panic::{AssertUnwindSafe, catch_unwind},
};

pub struct OriginalSamplerTransitionRequest {
    kind: Kind,
    command: Option<Command>,
}
impl OriginalSamplerTransitionRequest {
    pub fn command(command: Command) -> Self {
        Self {
            kind: Kind::Command,
            command: Some(command),
        }
    }
    pub fn undo() -> Self {
        Self {
            kind: Kind::Undo,
            command: None,
        }
    }
    pub fn redo() -> Self {
        Self {
            kind: Kind::Redo,
            command: None,
        }
    }
    pub fn command_value(&self) -> Option<&Command> {
        self.command.as_ref()
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Command,
    Undo,
    Redo,
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Requested,
    Preparing,
    Prepared,
    Refused,
    Committing,
    Committed,
    Cleaning,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OriginalSamplerTransitionResult {
    pub created: Option<Created>,
    pub changed: bool,
}
/// Nominal actual pending body. Private fields prevent arbitrary construction.
/// Borrowed children still require mandatory physical traversal; no field here
/// certifies cost, absence, constructor origin, retirement or publication.
pub struct OriginalSamplerPending {
    phase: Phase,
    kind: Kind,
    request: Option<OriginalSamplerTransitionRequest>,
    candidate: Option<Project>,
    created: Option<Created>,
    changed: bool,
    undo_replacement: Option<VecDeque<Project>>,
    redo_replacement: Option<Vec<Project>>,
    retired: Option<Project>,
    retired_redo: Option<Vec<Project>>,
    first_panic: Option<Box<dyn Any + Send>>,
    #[cfg(test)]
    cleanup_probe: Option<CleanupProbe>,
}
impl Default for OriginalSamplerPending {
    fn default() -> Self {
        Self {
            phase: Phase::Idle,
            kind: Kind::Command,
            request: None,
            candidate: None,
            created: None,
            changed: false,
            undo_replacement: None,
            redo_replacement: None,
            retired: None,
            retired_redo: None,
            first_panic: None,
            #[cfg(test)]
            cleanup_probe: None,
        }
    }
}
impl OriginalSamplerPending {
    pub fn request(&self) -> Option<&OriginalSamplerTransitionRequest> {
        self.request.as_ref()
    }
    pub fn candidate(&self) -> Option<&Project> {
        self.candidate.as_ref()
    }
    pub fn created(&self) -> &Option<Created> {
        &self.created
    }
    pub fn undo_replacement(&self) -> Option<&VecDeque<Project>> {
        self.undo_replacement.as_ref()
    }
    pub fn redo_replacement(&self) -> Option<&Vec<Project>> {
        self.redo_replacement.as_ref()
    }
    pub fn retired(&self) -> Option<&Project> {
        self.retired.as_ref()
    }
    pub fn retired_redo(&self) -> Option<&Vec<Project>> {
        self.retired_redo.as_ref()
    }
    pub fn inspectable(&self) -> bool {
        matches!(
            self.phase,
            Phase::Requested | Phase::Prepared | Phase::Committed
        ) && self.first_panic.is_none()
    }
    fn idle(&self) -> bool {
        self.phase == Phase::Idle
            && self.request.is_none()
            && self.candidate.is_none()
            && self.undo_replacement.is_none()
            && self.redo_replacement.is_none()
            && self.retired.is_none()
            && self.retired_redo.is_none()
            && self.created.is_none()
            && self.first_panic.is_none()
    }
    fn clean(&mut self) -> Result<(), Invalid> {
        if self.phase == Phase::Preparing {
            return Err(Invalid("original sampler transition is preparing"));
        }
        if matches!(
            self.phase,
            Phase::Committing | Phase::Cleaning | Phase::Failed
        ) || self.first_panic.is_some()
        {
            return Err(Invalid("original sampler transition cleanup is incomplete"));
        }
        self.phase = Phase::Cleaning;
        // Each consumed field stays consumed after unwind. Remaining fields
        // stay discoverable in this SAME parent; a failed cleanup is not retried.
        let result = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(test)]
            drop(self.cleanup_probe.take());
            drop(self.request.take());
            drop(self.candidate.take());
            drop(self.retired.take());
            drop(self.retired_redo.take());
            drop(self.undo_replacement.take());
            drop(self.redo_replacement.take());
        }));
        match result {
            Ok(()) => {
                self.created = None;
                self.changed = false;
                self.phase = Phase::Idle;
                Ok(())
            }
            Err(payload) => {
                self.first_panic = Some(payload);
                self.phase = Phase::Failed;
                Err(Invalid("original sampler transition cleanup failed"))
            }
        }
    }
}

pub struct OriginalSamplerEditorTransition<'a> {
    editor: &'a Editor,
    pending: RefMut<'a, OriginalSamplerPending>,
}
impl Editor {
    /// Post-loan commit precondition from the actual still-rooted candidate.
    /// No owner or approval is returned; the borrow must end before commit.
    pub fn original_sampler_prepared_candidate_on_control(
        &mut self,
    ) -> Result<(&Project, OriginalSamplerTransitionResult), Invalid> {
        let pending = self.original_sampler_pending.get_mut();
        if pending.phase != Phase::Prepared || pending.first_panic.is_some() {
            return Err(Invalid("original sampler candidate is not prepared"));
        }
        Ok((
            pending.candidate.as_ref().unwrap_or(&self.project),
            OriginalSamplerTransitionResult {
                changed: pending.changed,
                created: pending.created,
            },
        ))
    }
    /// Acquire before Audio guards. Moves an EXISTING request only: no clone,
    /// apply, reserve, new owner or destructor. On refusal producer is unchanged.
    pub fn original_sampler_transition_on_control(
        &self,
        request: &mut Option<OriginalSamplerTransitionRequest>,
    ) -> Result<OriginalSamplerEditorTransition<'_>, Invalid> {
        let mut pending = self
            .original_sampler_pending
            .try_borrow_mut()
            .map_err(|_| Invalid("original sampler transition is busy"))?;
        if !pending.idle() || request.is_none() {
            return Err(Invalid("original sampler transition is unavailable"));
        }
        let Some(actual) = request.take() else {
            return Err(Invalid("original sampler transition request is missing"));
        };
        pending.kind = actual.kind;
        pending.request = Some(actual);
        pending.phase = Phase::Requested;
        Ok(OriginalSamplerEditorTransition {
            editor: self,
            pending,
        })
    }
    /// Legacy visitors cannot infer pending absence from current/history alone.
    /// A live pending guard or any nonvacant/failed state refuses that subset.
    pub fn require_no_original_sampler_pending_on_control(&self) -> Result<(), Invalid> {
        let pending = self
            .original_sampler_pending
            .try_borrow()
            .map_err(|_| Invalid("original sampler pending inspection needs its actual guard"))?;
        if !pending.idle() {
            return Err(Invalid(
                "original sampler pending fields require inspection",
            ));
        }
        Ok(())
    }
    pub(super) fn require_original_sampler_idle(&mut self) -> Result<(), Invalid> {
        if !self.original_sampler_pending.get_mut().idle() {
            return Err(Invalid("original sampler transition is pending"));
        }
        Ok(())
    }
    /// Audited Session only, AFTER actual native construction and after all
    /// immutable projections/writers/native guards end. No allocation/drop of
    /// owning metadata occurs during these fixed model/history moves.
    pub fn commit_original_sampler_transition_on_control(
        &mut self,
    ) -> Result<OriginalSamplerTransitionResult, Invalid> {
        let pending = self.original_sampler_pending.get_mut();
        if pending.phase != Phase::Prepared || pending.first_panic.is_some() {
            return Err(Invalid("original sampler transition is not prepared"));
        }
        let result = OriginalSamplerTransitionResult {
            created: pending.created,
            changed: pending.changed,
        };
        if !pending.changed {
            pending.phase = Phase::Committed;
            return Ok(result);
        }
        let capacity_ok = match pending.kind {
            Kind::Command | Kind::Redo => {
                self.undo.capacity() >= (self.undo.len() + 1).min(self.capacity)
                    || pending
                        .undo_replacement
                        .as_ref()
                        .is_some_and(|v| v.capacity() >= (self.undo.len() + 1).min(self.capacity))
            }
            Kind::Undo => {
                self.redo.capacity() > self.redo.len()
                    || pending
                        .redo_replacement
                        .as_ref()
                        .is_some_and(|v| v.capacity() > self.redo.len())
            }
        };
        if !capacity_ok
            || pending.candidate.is_none()
            || pending.retired.is_some()
            || pending.retired_redo.is_some()
            || (pending.kind == Kind::Undo && self.undo.is_empty())
            || (pending.kind == Kind::Redo
                && (self.redo.is_empty() || self.undo.len() == self.capacity))
        {
            return Err(Invalid("original sampler transition destinations differ"));
        }
        pending.phase = Phase::Committing;
        let Some(candidate) = pending.candidate.take() else {
            pending.phase = Phase::Failed;
            return Err(Invalid("original sampler transition candidate is missing"));
        };
        // Backings were reserved in pending custody. Old empty backing remains
        // rooted there for later cleanup, rather than dropping during commit.
        if let Some(replacement) = &mut pending.undo_replacement {
            std::mem::swap(&mut self.undo, replacement);
            while let Some(project) = replacement.pop_front() {
                self.undo.push_back(project);
            }
        }
        if let Some(replacement) = &mut pending.redo_replacement {
            std::mem::swap(&mut self.redo, replacement);
            while let Some(project) = replacement.pop() {
                self.redo.push(project);
            }
            self.redo.reverse();
        }
        match pending.kind {
            Kind::Command => {
                if self.undo.len() == self.capacity {
                    pending.retired = self.undo.pop_front();
                }
                self.undo
                    .push_back(std::mem::replace(&mut self.project, candidate));
                pending.retired_redo = Some(std::mem::take(&mut self.redo));
            }
            Kind::Undo => {
                pending.retired = self.undo.pop_back();
                self.redo
                    .push(std::mem::replace(&mut self.project, candidate));
            }
            Kind::Redo => {
                pending.retired = self.redo.pop();
                self.undo
                    .push_back(std::mem::replace(&mut self.project, candidate));
            }
        }
        pending.phase = Phase::Committed;
        Ok(result)
    }
    /// Must run outside native guards. Committed model state is not rolled back
    /// by metadata cleanup; consumed failed cleanup remains rooted/nonretryable.
    pub fn clear_original_sampler_transition_on_control(&mut self) -> Result<(), Invalid> {
        self.original_sampler_pending.get_mut().clean()
    }
}
impl OriginalSamplerEditorTransition<'_> {
    pub fn prepared_result_on_control(&self) -> Result<OriginalSamplerTransitionResult, Invalid> {
        if self.pending.phase != Phase::Prepared || self.pending.first_panic.is_some() {
            return Err(Invalid("original sampler candidate is not prepared"));
        }
        Ok(OriginalSamplerTransitionResult {
            created: self.pending.created,
            changed: self.pending.changed,
        })
    }
    /// Actual clone source selected by the held transition, never a snapshot.
    pub fn clone_source_on_control(&self) -> Option<&Project> {
        match self.pending.kind {
            Kind::Command => Some(&self.editor.project),
            Kind::Undo => self.editor.undo.back(),
            Kind::Redo => self.editor.redo.last(),
        }
    }
    /// Required element counts of the two existing history destinations. These
    /// are structural fields for the closed cost walk, never byte/admission
    /// claims; old capacity and all child allocations remain mandatory.
    pub fn replacement_elements_on_control(&self) -> Result<(usize, usize), Invalid> {
        if self.clone_source_on_control().is_none() {
            return Ok((0, 0));
        }
        match self.pending.kind {
            Kind::Command | Kind::Redo => {
                if self.pending.kind == Kind::Redo && self.editor.undo.len() == self.editor.capacity
                {
                    return Err(Invalid("original sampler redo history is full"));
                }
                let count = self
                    .editor
                    .undo
                    .len()
                    .checked_add(1)
                    .ok_or(Invalid("original sampler history overflow"))?
                    .min(self.editor.capacity);
                Ok((
                    if self.editor.undo.capacity() < count {
                        count
                    } else {
                        0
                    },
                    0,
                ))
            }
            Kind::Undo => {
                let count = self
                    .editor
                    .redo
                    .len()
                    .checked_add(1)
                    .ok_or(Invalid("original sampler history overflow"))?;
                Ok((
                    0,
                    if self.editor.redo.capacity() < count {
                        count
                    } else {
                        0
                    },
                ))
            }
        }
    }
    /// Failed acquisition of a later Session writer returns ONLY an untouched
    /// requested command. No prepared candidate or consumed cleanup can retry.
    pub fn return_unprepared_request_on_control(
        &mut self,
        destination: &mut Option<OriginalSamplerTransitionRequest>,
    ) -> Result<(), Invalid> {
        if self.pending.phase != Phase::Requested
            || destination.is_some()
            || self.pending.request.is_none()
            || self.pending.candidate.is_some()
            || self.pending.created.is_some()
            || self.pending.undo_replacement.is_some()
            || self.pending.redo_replacement.is_some()
            || self.pending.retired.is_some()
            || self.pending.retired_redo.is_some()
            || self.pending.first_panic.is_some()
        {
            return Err(Invalid("original sampler request has advanced"));
        }
        *destination = self.pending.request.take();
        self.pending.phase = Phase::Idle;
        Ok(())
    }
    pub fn editor(&self) -> &Editor {
        self.editor
    }
    pub fn storage(&self) -> &OriginalSamplerPending {
        &self.pending
    }
    /// Every actual retained Project under this SAME pending guard, without
    /// snapshots or allocated observation lists. Command/Created and backings
    /// remain separate mandatory child observations.
    pub fn retained_projects_on_control(&self) -> Result<impl Iterator<Item = &Project>, Invalid> {
        if !self.pending.inspectable() {
            return Err(Invalid("original sampler pending state is incomplete"));
        }
        Ok(self
            .editor
            .retained_projects_on_control()
            .chain(self.pending.candidate.iter())
            .chain(self.pending.retired.iter())
            .chain(self.pending.retired_redo.iter().flat_map(|v| v.iter()))
            .chain(self.pending.undo_replacement.iter().flat_map(|v| v.iter()))
            .chain(self.pending.redo_replacement.iter().flat_map(|v| v.iter())))
    }
    /// This is a mutation entry, NEVER proof of a prefix admission. Repository
    /// policy permits native calls only inside the closed Engine prefix/driver.
    pub fn stage_original_sampler_after_prefix_on_control(&mut self) -> Result<(), Invalid> {
        if self.pending.phase != Phase::Requested {
            return Err(Invalid("original sampler transition cannot be staged"));
        }
        self.pending.phase = Phase::Preparing;
        let result = catch_unwind(AssertUnwindSafe(|| self.stage()));
        match result {
            Ok(Ok(())) => {
                self.pending.phase = Phase::Prepared;
                Ok(())
            }
            Ok(Err(error)) => {
                self.pending.phase = Phase::Refused;
                Err(error)
            }
            Err(payload) => {
                self.pending.first_panic = Some(payload);
                self.pending.phase = Phase::Failed;
                Err(Invalid("original sampler transition preparation failed"))
            }
        }
    }
    fn stage(&mut self) -> Result<(), Invalid> {
        let target = match self.pending.kind {
            Kind::Command => Some(&self.editor.project),
            Kind::Undo => self.editor.undo.back(),
            Kind::Redo => self.editor.redo.last(),
        };
        let Some(target) = target else {
            return Ok(());
        };
        let (undo, redo) = self.replacement_elements_on_control()?;
        if undo != 0 {
            let mut next = VecDeque::new();
            next.try_reserve_exact(undo)
                .map_err(|_| Invalid("could not reserve original sampler undo history"))?;
            self.pending.undo_replacement = Some(next);
        }
        if redo != 0 {
            let mut next = Vec::new();
            next.try_reserve_exact(redo)
                .map_err(|_| Invalid("could not reserve original sampler redo history"))?;
            self.pending.redo_replacement = Some(next);
        }
        self.pending.candidate = Some(target.clone());
        let pending = &mut *self.pending;
        let Some(candidate) = &mut pending.candidate else {
            return Err(Invalid("original sampler candidate is missing"));
        };
        match pending.kind {
            Kind::Command => {
                let Some(OriginalSamplerTransitionRequest {
                    command: Some(command),
                    ..
                }) = pending.request.take()
                else {
                    return Err(Invalid("original sampler command is missing"));
                };
                pending.created = Some(candidate.apply(command)?);
                candidate.validate_device_asset_references()?;
                candidate.validate_playable_routes()?;
            }
            Kind::Undo | Kind::Redo => {
                candidate.next_id = candidate.next_id.max(self.editor.project.next_id);
                candidate.retain_gain_touch_epochs_from(&self.editor.project);
            }
        }
        pending.changed = pending.kind != Kind::Command || candidate != &self.editor.project;
        Ok(())
    }
}

#[cfg(test)]
struct CleanupProbe(std::sync::Arc<std::sync::atomic::AtomicUsize>);
#[cfg(test)]
impl Drop for CleanupProbe {
    fn drop(&mut self) {
        self.0.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        panic!("actual pending cleanup field fault");
    }
}
#[cfg(test)]
mod tests;
