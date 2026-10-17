//! Host scope only after a note leaves its compact coefficient bank. No owning
//! handles/pointers enter the sampler payload or 360-byte bank entry.
use super::*;
#[derive(Clone, Copy, Debug)]
struct InlineScope {
    context: SharedContextRef,
    id: crate::shared_analytic::SharedContextId,
}
#[derive(Clone, Copy, Debug)]
pub(crate) struct ScopedMidiNote {
    pub(crate) note: PreparedMidiNote,
    scope: Option<InlineScope>,
}
impl From<PreparedMidiNote> for ScopedMidiNote {
    fn from(note: PreparedMidiNote) -> Self {
        Self { note, scope: None }
    }
}
impl ScopedMidiNote {
    pub(super) fn for_bank_entry(
        note: PreparedMidiNote,
        context: Option<(u16, crate::shared_analytic::SharedContextId)>,
    ) -> Self {
        let scope = if matches!(note, PreparedMidiNote::Inline(_)) {
            context.map(|(index, id)| InlineScope {
                context: SharedContextRef::Revision(index),
                id,
            })
        } else {
            None
        };
        Self { note, scope }
    }
    pub(crate) fn inline_without_resources(self) -> Option<PreparedAnalyticNote> {
        match (self.note, self.scope) {
            (PreparedMidiNote::Inline(note), None) => Some(note),
            _ => None,
        }
    }
    pub(crate) fn release_frames(self) -> u64 {
        self.note.release_frames()
    }
    pub(crate) fn context_ref(self) -> Option<SharedContextRef> {
        self.scope
            .map(|scope| scope.context)
            .or_else(|| self.note.context_ref())
    }
    pub(crate) fn context_id(self) -> Option<crate::shared_analytic::SharedContextId> {
        self.scope
            .map(|scope| scope.id)
            .or_else(|| self.note.context_id())
    }
    pub(crate) fn is_current(self) -> bool {
        self.scope.is_none() && self.note.is_current()
    }
    pub(crate) fn with_context_ref(mut self, context: SharedContextRef) -> Self {
        if let Some(scope) = &mut self.scope {
            scope.context = context;
        } else {
            self.note = self.note.with_context_ref(context);
        }
        self
    }
    pub(crate) fn detach(
        mut self,
        transitions: &[Option<hex_lattice::AnalyticContinuation>],
    ) -> Self {
        self.note = self.note.detach(transitions);
        self
    }
    pub(crate) fn rebind(
        mut self,
        context: u16,
        owner: &SharedContextOwner,
    ) -> Result<Self, Invalid> {
        if let Some(scope) = &mut self.scope {
            if owner.id() != scope.id || !owner.is_inline_resource_scope() {
                return Err(Invalid(
                    "inline resource source configuration cannot be rebound",
                ));
            }
            scope.context = SharedContextRef::Revision(context);
        } else {
            self.note = self.note.rebind(context, owner)?;
        }
        Ok(self)
    }
    pub(crate) fn sample(
        &self,
        contexts: &SharedAnalyticContextTable,
        retained: &[Option<crate::shared_analytic::SharedContextLease>],
        age: u64,
        gate: u64,
    ) -> Option<Frame> {
        if self.scope.is_none() {
            self.note.sample(contexts, retained, age, gate)
        } else {
            self.sample_at(contexts, retained, &[], 0, age, gate).ok()
        }
    }
    pub(crate) fn sample_at(
        &self,
        contexts: &SharedAnalyticContextTable,
        retained: &[Option<crate::shared_analytic::SharedContextLease>],
        transitions: &[Option<hex_lattice::AnalyticContinuation>],
        performance_frame: u64,
        age: u64,
        gate: u64,
    ) -> Result<Frame, Invalid> {
        if let Some(scope) = self.scope {
            let owner = match scope.context {
                SharedContextRef::Revision(index) => contexts.owner(index),
                SharedContextRef::Retained(index) => retained
                    .get(usize::from(index))
                    .and_then(Option::as_deref)
                    .map(Arc::as_ref),
            }
            .filter(|owner| owner.id() == scope.id && owner.is_inline_resource_scope())
            .ok_or(Invalid(
                "inline resource source configuration is unavailable",
            ))?;
            let PreparedMidiNote::Inline(note) = self.note else {
                return Err(Invalid("inline resource note class is invalid"));
            };
            Ok(note.sample(age, gate, owner.resources()))
        } else {
            self.note.sample_at(
                contexts,
                retained,
                transitions,
                performance_frame,
                age,
                gate,
            )
        }
    }
}
