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
    extend_target(
        host,
        control,
        binding,
        class,
        origin.parameter,
        |control| {
            let (_, revision) = validate_current_parameter(control, binding, origin, steps, false)?;
            Ok((
                steps,
                ParameterPageQuery {
                    first: origin.page_first,
                    revision,
                    conversion: None,
                },
            ))
        },
        cancellation,
    )
}

#[allow(clippy::too_many_arguments)] // Exact source/target and optional non-authoritative page hint.
pub(in crate::native_audio::plugins) fn extend_authored(
    host: &Hosting,
    control: &mut PublicationControl,
    binding: Binding,
    class: [u8; 16],
    id: u32,
    hint: Option<u16>,
    cancellation: Option<&RequestCancellation>,
) -> Result<AuthoredSelection, String> {
    extend_target(
        host,
        control,
        binding,
        class,
        id,
        |control| find_target(control, binding, id, hint, cancellation),
        cancellation,
    )
    .map_err(|error| match error {
        Error::Refused(reason) | Error::SourceLost(reason) => reason,
    })
}

fn extend_target(
    host: &Hosting,
    control: &mut PublicationControl,
    binding: Binding,
    class: [u8; 16],
    id: u32,
    query: impl FnOnce(&mut PublicationControl) -> Result<(u32, ParameterPageQuery), Error>,
    cancellation: Option<&RequestCancellation>,
) -> Result<AuthoredSelection, Error> {
    let metadata = edits::source_metadata(control, binding, cancellation).map_err(refused)?;
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
    let constructor = edits::canonical_bases(
        host,
        control,
        binding,
        &original,
        &selection,
        cancellation,
        None,
    )
    .map_err(refused)?;
    edits::check_cancel(cancellation).map_err(refused)?;
    let (steps, query) = query(control)?;
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

/// Page decoding bounds the inventory; an increasing offset and fixed total
/// bound a cold lookup. PCM value revisions need not agree across pages: only
/// the fresh target row is used, and extension rechecks it on the same SDK lease.
fn find_target(
    control: &mut PublicationControl,
    binding: Binding,
    id: u32,
    hint: Option<u16>,
    cancellation: Option<&RequestCancellation>,
) -> Result<(u32, ParameterPageQuery), Error> {
    use hex_audio::plugin_worker::{ControllerRequest, NativeWorkerControllerProgress};
    let mut first = hint.unwrap_or(0);
    let mut hinted = first != 0;
    let mut total = None;
    loop {
        edits::check_cancel(cancellation).map_err(refused)?;
        let token = admit(control, |control| {
            control.begin_native_source_controller_query_on_control(
                binding,
                ControllerRequest::SelectedParameters { first },
            )
        })?;
        let mut cancelled = false;
        let page = loop {
            cancelled |= cancellation.is_some_and(RequestCancellation::is_cancelled);
            match control.poll_native_source_controller_query_on_control(binding, &token) {
                Ok(NativeWorkerControllerProgress::Pending) | Err("native preparation is busy") => {
                    edits::poll_control(control)
                }
                Ok(NativeWorkerControllerProgress::Page(page)) => break page,
                _ => return Err(refused("vst3 target metadata changed · refresh the list")),
            }
        };
        if cancelled {
            return Err(refused("vst3 target lookup cancelled"));
        }
        if page.first != first {
            return Err(lost());
        }
        if let Some(parameter) = page.as_slice().iter().find(|parameter| parameter.id == id) {
            if !parameter.is_editable() || !parameter.supports_authored_automation() {
                return Err(refused("vst3 target does not support automation"));
            }
            return Ok((
                parameter.step_count,
                ParameterPageQuery {
                    first,
                    revision: page.revision,
                    conversion: None,
                },
            ));
        }
        if hinted {
            first = 0;
            hinted = false;
            continue;
        }
        if *total.get_or_insert(page.total) != page.total {
            return Err(lost());
        }
        let next = first
            .checked_add(u16::try_from(page.as_slice().len()).map_err(|_| lost())?)
            .ok_or_else(lost)?;
        if next <= first || next >= page.total {
            return Err(refused("vst3 automation target is missing"));
        }
        first = next;
    }
}

#[cfg(test)]
impl Plugins {
    pub(in crate::native_audio) fn lookup_authored_target_for_test(
        control: &mut PublicationControl,
        binding: Binding,
        id: u32,
        hint: Option<u16>,
    ) -> Result<(u32, u16), String> {
        find_target(control, binding, id, hint, None)
            .map(|(steps, query)| (steps, query.first))
            .map_err(|error| match error {
                Error::Refused(reason) | Error::SourceLost(reason) => reason,
            })
    }
}
