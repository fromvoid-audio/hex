//! One newly touched target joins Root only after actual SDK validation and a
//! fully disposed isolated constructor-base query. No live processor restore.
use super::*;
use hex_audio::plugin_worker::{
    NativeWorkerAuthoredParameterProgress as Progress, ParameterPageQuery,
};

#[allow(clippy::too_many_arguments)] // Exact SDK target and its existing source/query owners.
pub(super) fn extend(
    host: &Hosting,
    control: &mut PublicationControl,
    binding: Binding,
    class: [u8; 16],
    origin: PluginParameterOrigin,
    steps: u32,
    cancellation: Option<&RequestCancellation>,
) -> Result<AuthoredSelection, Error> {
    let id = origin.parameter;
    let metadata = control.native_source_metadata(binding).map_err(refused)?;
    if metadata.selection().cid != class {
        return Err(lost());
    }
    let selection = AuthoredSelection::from_targets(
        metadata
            .authored_base_values()
            .map(|(old, _)| (old, Some(class)))
            .chain(std::iter::once((id, Some(class)))),
    )
    .map_err(refused)?
    .ok_or_else(lost)?;
    let original = edits::original_state(control, binding, cancellation).map_err(refused)?;
    let constructor =
        edits::canonical_bases(host, control, binding, &original, &selection, cancellation)
            .map_err(refused)?;
    edits::check_cancel(cancellation).map_err(refused)?;
    let (_, revision) = validate_current_parameter(control, binding, origin, steps, false)?;
    let query = ParameterPageQuery {
        first: origin.page_first,
        revision,
        conversion: None,
    };
    let class = Hosting::saved_identity_on_control(&original)
        .map_err(refused)?
        .class;
    let token = admit(control, |control| {
        control.begin_native_source_parameter_extension_on_control(binding, class, id, steps, query)
    })?;
    let mut failure = None;
    loop {
        if cancellation.is_some_and(RequestCancellation::is_cancelled) {
            failure.get_or_insert_with(|| "vst3 metadata extension cancelled".to_owned());
        }
        let result = if failure.is_some() {
            control.drain_native_source_parameter_extension_on_control(binding, &token)
        } else {
            control.complete_native_source_parameter_extension_on_control(
                binding,
                &token,
                &constructor,
            )
        };
        match result {
            Ok(Progress::Pending) | Err("native preparation is busy" | "native source is busy") => {
            }
            Ok(Progress::Armed {
                id: actual,
                steps: actual_steps,
                ..
            }) if failure.is_none() && actual == id && actual_steps == steps => {
                return Ok(selection);
            }
            Ok(Progress::Refused(reason)) if failure.is_none() => return Err(refused(reason)),
            Ok(Progress::Failed) if failure.is_none() => return Err(lost()),
            Ok(_) => {
                return Err(refused(failure.unwrap_or_else(|| {
                    "vst3 metadata changed · refresh the list".into()
                })));
            }
            Err(reason) if failure.is_none() => {
                // A source fence may refuse before consuming the SDK terminal.
                // Drain without publication; closure leaves late custody in Root.
                failure = Some(reason.to_owned());
            }
            Err(_) => {
                return Err(refused(
                    failure.unwrap_or_else(|| "vst3 metadata extension failed".into()),
                ));
            }
        }
        edits::poll_control(control);
    }
}
