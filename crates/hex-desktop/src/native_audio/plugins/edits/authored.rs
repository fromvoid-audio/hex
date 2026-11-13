//! Lane preparation extends the original instrument before source matching.
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
        if !destination.is_instrument() {
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
            // The metadata extension uses the same serialized controller as
            // display polling. Finish an admitted read on its original owner
            // before starting the edit, while retaining its explicit terminal.
            self.yield_parameters_to_control(control, input.context(), cancellation)?;
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
            let host = self
                .host
                .as_ref()
                .ok_or("qualified vst3 helper unavailable")?;
            let current = |control: &PublicationControl| {
                let playing = control.status.playing.load(Acquire);
                control
                    .status
                    .native_parameter_source(playing)
                    .map(|(source, epoch)| (source, (epoch, playing)))
            };
            let before = current(control);
            let extend = |control: &mut PublicationControl| {
                parameter_gesture::metadata::extend_authored(
                    host,
                    control,
                    binding,
                    class,
                    id,
                    hint,
                    cancellation,
                )
            };
            let selection = match extend(control) {
                // The metadata operation has drained its old token. Revalidate
                // once when transport advanced on the same installed graph.
                Err(_)
                    if before.zip(current(control)).is_some_and(
                        |((old, old_transport), (new, new_transport))| {
                            old == new && old_transport != new_transport
                        },
                    ) =>
                {
                    extend(control)?
                }
                result => result?,
            };
            self.sources[index].authored = Some(selection);
        }
        Ok(())
    }
}
