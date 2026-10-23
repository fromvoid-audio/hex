//! Pure compiler view borrowed from selected actual Root owners. The Work
//! guard holds those owners through the visitor; no ordinary bank lock spans
//! compilation and no worker, callback or publication authority escapes.
use super::*;
use crate::plugin_effects::NativeEffectDefinition;

impl Custody {
    pub(in crate::candidate_custody) fn with_native_effect_definition<R>(
        &self,
        bindings: &[Binding],
        prepare: impl FnOnce(&NativeEffectDefinition<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        if bindings.len() > MAX_SOURCES {
            return Err("native effect selection capacity exceeded".into());
        }
        let work = self
            .work
            .try_lock()
            .map_err(|_| "native effect preparation is busy")?;
        let bank = self
            .bank
            .try_lock()
            .map_err(|_| "native effect selection is busy")?;
        if !self.enabled() || bank.closing {
            return Err("native effect selection is closing".into());
        }
        let mut selected = Vec::new();
        selected
            .try_reserve_exact(bindings.len())
            .map_err(|_| "native effect selection allocation refused")?;
        for binding in bindings {
            let index = bank
                .sources
                .index(*binding)
                .ok_or("native effect source differs")?;
            let slot = &bank.sources.slots[index];
            if slot.phase != Phase::Accepted
                || slot.retiring
                || slot.starting
                || slot.processor.is_none()
            {
                return Err("native effect source is not constructed".into());
            }
            let Some(Owner::Effect(effect)) = work.sources.cells[index].owner.as_ref() else {
                return Err("native effect source owner differs".into());
            };
            if effect.binding() != *binding || slot.identity != Some(effect.identity()) {
                return Err("native effect source binding differs".into());
            }
            selected.push(effect);
        }
        drop(bank);
        let definition = NativeEffectDefinition::from_effects(selected)
            .map_err(|error| format!("native effect definition refused: {error}"))?;
        prepare(&definition)
    }
}

impl CandidateRoot {
    pub(crate) fn with_native_effect_definition<R>(
        &self,
        bindings: &[Binding],
        prepare: impl FnOnce(&NativeEffectDefinition<'_>) -> Result<R, String>,
    ) -> Result<R, String> {
        self.require_native_source_control_open()
            .map_err(str::to_owned)?;
        self.0
            .eligible
            .with_native_effect_definition(bindings, prepare)
    }
}
