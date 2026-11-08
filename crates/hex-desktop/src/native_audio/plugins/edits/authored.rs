//! Live lane preparation extends the original instrument before source matching.
//! SDK queries and isolated constructor facts retain their existing owners;
//! cached UI pages supply only optional lookup hints, never eligibility.
use super::*;

impl Plugins {
    #[allow(clippy::too_many_arguments)] // Exact prospective device and original canonical source.
    pub(super) fn extend_edit_targets(
        &mut self,
        control: &mut PublicationControl,
        input: &ResidentEffectPreparation<'_>,
        destination: PluginEffectDestination,
        device: u64,
        state: &NativeDeviceState,
        cancellation: Option<&RequestCancellation>,
    ) -> Result<(), String> {
        if !destination.is_instrument() || !control.status.playing.load(Acquire) {
            return Ok(());
        }
        let Some(desired) = AuthoredSelection::from_targets(input.authored_plugin_targets(device))?
        else {
            return Ok(());
        };
        let Some(index) = self.sources.iter().rposition(|source| {
            source.accepted
                && !source.retiring
                && source.project_generation == input.context().project_generation
                && source.owner == destination.owner()
                && source.device == device
        }) else {
            return Ok(());
        };
        if self.sources[index]
            .authored
            .as_ref()
            .is_some_and(|armed| armed.covers(Some(&desired)))
        {
            return Ok(());
        }
        let binding = self.sources[index].binding;
        if !original_state(control, binding, cancellation)?.processing_eq(state)
            && !captured_state_approved(control, binding, state, cancellation)?
        {
            // An actual processor-state replacement keeps its existing fresh
            // preparation path; equal class alone cannot retain this worker.
            return Ok(());
        }
        for (id, class) in input.authored_plugin_targets(device) {
            check_cancel(cancellation)?;
            let armed = armed_selection(control, binding, cancellation)?;
            let class = class.ok_or("authored vst3 class is missing")?;
            if armed
                .as_ref()
                .is_some_and(|armed| armed.contains(class, id))
            {
                continue;
            }
            let hint = self
                .observed_parameters
                .iter()
                .find(|inventory| {
                    inventory.binding == binding
                        && inventory.query.context.project_generation
                            == input.context().project_generation
                        && inventory.query.device.value() == device
                        && inventory.query.output_identity == binding.root
                })
                .and_then(|inventory| inventory.rows.iter().find(|row| row.id == id))
                .map(|row| row.page_first);
            let selection = parameter_gesture::metadata::extend_authored(
                self.host
                    .as_ref()
                    .ok_or("qualified vst3 helper unavailable")?,
                control,
                binding,
                class,
                id,
                hint,
                cancellation,
            )?;
            self.sources[index].authored = Some(selection);
        }
        Ok(())
    }
}
