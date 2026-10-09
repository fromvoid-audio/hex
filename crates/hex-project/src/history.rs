use crate::{Command, Created, Invalid, Project};
use std::collections::VecDeque;

/// Bounded snapshot history on the control thread. Oldest edits expire at capacity.
/// IDs never rewind across undo, redo or branching, so stale references stay stale.
pub struct Editor {
    project: Project,
    undo: VecDeque<Project>,
    redo: Vec<Project>,
    capacity: usize,
}
impl Editor {
    pub fn new(project: Project, capacity: usize) -> Result<Self, Invalid> {
        if !(1..=128).contains(&capacity) {
            return Err(Invalid("history capacity must be 1..128"));
        }
        Ok(Self {
            project,
            undo: VecDeque::new(),
            redo: Vec::new(),
            capacity,
        })
    }
    pub fn project(&self) -> &Project {
        &self.project
    }
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn execute(&mut self, command: Command) -> Result<Created, Invalid> {
        self.execute_validated(command, |_| Ok(()))
    }

    /// Validate a prepared candidate before committing it or changing history.
    /// Control-side callers can reject edits that cannot produce a playable
    /// graph without consuming IDs, clearing redo, or introducing an undo step.
    pub fn execute_validated<E: From<Invalid>>(
        &mut self,
        command: Command,
        validate: impl FnOnce(&Project) -> Result<(), E>,
    ) -> Result<Created, E> {
        self.execute_prepared(command, validate)
            .map(|(created, ())| created)
    }

    /// Prepare a control-side artifact from the candidate and return that exact
    /// artifact with the committed edit. Failure preserves project and history.
    pub fn execute_prepared<T, E: From<Invalid>>(
        &mut self,
        command: Command,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<(Created, T), E> {
        let mut candidate = self.project.clone();
        let result = candidate.apply(command)?;
        let prepared = prepare(&candidate)?;
        // No-op controls should neither add history nor erase a redo branch.
        if candidate == self.project {
            return Ok((result, prepared));
        }
        let previous = std::mem::replace(&mut self.project, candidate);
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo.push_back(previous);
        self.redo.clear();
        Ok((result, prepared))
    }
    pub fn undo(&mut self) -> bool {
        let Some(mut previous) = self.undo.pop_back() else {
            return false;
        };
        previous.next_id = previous.next_id.max(self.project.next_id);
        self.redo
            .push(std::mem::replace(&mut self.project, previous));
        true
    }

    /// Prepare the previous snapshot before moving it across history. Failure
    /// leaves the project, allocator high-water mark, and both history stacks
    /// unchanged.
    pub fn undo_prepared<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let Some(previous) = self.undo.back() else {
            return Ok(None);
        };
        let mut candidate = previous.clone();
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        self.redo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve redo history")))?;
        let prepared = prepare(&candidate)?;
        let Some(_) = self.undo.pop_back() else {
            return Err(E::from(Invalid("undo history changed during preparation")));
        };
        self.redo
            .push(std::mem::replace(&mut self.project, candidate));
        Ok(Some(prepared))
    }

    pub fn redo(&mut self) -> bool {
        let Some(mut next) = self.redo.pop() else {
            return false;
        };
        next.next_id = next.next_id.max(self.project.next_id);
        self.undo
            .push_back(std::mem::replace(&mut self.project, next));
        true
    }

    /// Prepare the next snapshot before moving it across history. Failure
    /// leaves the project, allocator high-water mark, and both history stacks
    /// unchanged.
    pub fn redo_prepared<T, E: From<Invalid>>(
        &mut self,
        prepare: impl FnOnce(&Project) -> Result<T, E>,
    ) -> Result<Option<T>, E> {
        let Some(next) = self.redo.last() else {
            return Ok(None);
        };
        let mut candidate = next.clone();
        candidate.next_id = candidate.next_id.max(self.project.next_id);
        self.undo
            .try_reserve(1)
            .map_err(|_| E::from(Invalid("could not reserve undo history")))?;
        let prepared = prepare(&candidate)?;
        let Some(_) = self.redo.pop() else {
            return Err(E::from(Invalid("redo history changed during preparation")));
        };
        if self.undo.len() == self.capacity {
            self.undo.pop_front();
        }
        self.undo
            .push_back(std::mem::replace(&mut self.project, candidate));
        Ok(Some(prepared))
    }
}
