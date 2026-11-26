//! Capability requests share the original controller mailbox and SDK reader.
//! Copied completion is not Root capability publication or a canonical base.
use super::guarded_write::GuardedWrite;
use super::*;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    Observe,
    Extension,
    GuardedWrite,
}

#[derive(Clone, Copy)]
#[allow(clippy::large_enum_variant)] // One fixed native controller slot includes the bounded input batch.
pub(super) enum Operation {
    Observe(ControllerRequest),
    Guarded(GuardedWrite),
    Extend {
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: ParameterPageQuery,
    },
}
impl Default for Operation {
    fn default() -> Self {
        Self::Observe(ControllerRequest::default())
    }
}
impl Operation {
    pub(super) fn kind(&self) -> Kind {
        match self {
            Self::Observe(_) => Kind::Observe,
            Self::Extend { .. } => Kind::Extension,
            Self::Guarded(_) => Kind::GuardedWrite,
        }
    }
    pub(super) fn validate(self) -> Result<(), &'static str> {
        match self {
            Self::Guarded(_) => Ok(()),
            Self::Observe(query) => query
                .validate()
                .map_err(|_| "invalid native controller query"),
            Self::Extend { id, query, .. } => {
                query
                    .validate()
                    .map_err(|_| "invalid native extension query")?;
                if id > i32::MAX as u32 || query.revision == 0 || query.conversion.is_some() {
                    return Err("invalid native extension identity");
                }
                Ok(())
            }
        }
    }

    pub(super) fn begin(
        self,
        instance: &mut hex_plugin_host::native::Prepared,
    ) -> Result<Option<Reply>, Error> {
        match self {
            Self::Guarded(input) => input.apply(instance).map(Reply::guarded),
            Self::Observe(query) => {
                instance.begin_controller_request_on_worker(query)?;
                Ok(None)
            }
            Self::Extend {
                class,
                id,
                steps,
                query,
            } => {
                match instance.begin_authored_parameter_extension_on_worker(class, id, steps, query)
                {
                    Ok(()) => Ok(None),
                    // Native marks actual I/O/protocol failures faulted. A
                    // healthy source can refuse capacity/identity admission
                    // without losing its existing PCM processor.
                    Err(Error::Protocol(reason))
                        if instance.validate_worker_transfer_on_control().is_ok() =>
                    {
                        Ok(Some(Reply {
                            extension: Some(NativeWorkerAuthoredParameterProgress::Refused(reason)),
                            ..Reply::default()
                        }))
                    }
                    Err(error) => Err(error),
                }
            }
        }
    }
}

impl Reply {
    pub(super) fn extension(
        progress: hex_plugin_host::native::AuthoredParameterExtensionProgress,
    ) -> Option<Self> {
        use NativeWorkerAuthoredParameterProgress as Progress;
        use hex_plugin_host::native::AuthoredParameterExtensionProgress as Native;
        let extension = match progress {
            Native::Pending => return None,
            Native::Armed {
                revision,
                id,
                steps,
            } => Progress::Armed {
                revision,
                id,
                steps,
            },
            Native::Changed { revision, total } => Progress::Changed { revision, total },
            Native::Refused(reason) => Progress::Refused(reason),
        };
        Some(Self {
            extension: Some(extension),
            ..Self::default()
        })
    }
}
/// Original-worker capability acknowledgement only; it grants no canonical
/// base, Root source admission, endpoint custody or graph publication.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NativeWorkerAuthoredParameterProgress {
    Pending,
    Armed { revision: u64, id: u32, steps: u32 },
    Changed { revision: u64, total: u16 },
    Refused(&'static str),
    Failed,
}

impl NativeWorker {
    /// Uses the same single controller slot and original reader. The caller
    /// must reserve native metadata capacity before worker construction.
    pub fn begin_authored_parameter_extension_on_control(
        &mut self,
        class: hex_plugin_state::ClassId,
        id: u32,
        steps: u32,
        query: ParameterPageQuery,
    ) -> Result<NativeWorkerControllerQuery, &'static str> {
        self.begin_query(Operation::Extend {
            class,
            id,
            steps,
            query,
        })
    }

    /// Only the exact source/ordinal/kind consumes its terminal. Revoked live
    /// tokens fail even if the SDK completed; stopped tokens use joining control.
    pub fn poll_authored_parameter_extension_on_control(
        &mut self,
        request: &NativeWorkerControllerQuery,
    ) -> Result<NativeWorkerAuthoredParameterProgress, &'static str> {
        if request.kind != Kind::Extension {
            return Err("native controller operation kind differs");
        }
        match self.poll_reply(request, |reply| reply.extension)? {
            Poll::Pending => Ok(NativeWorkerAuthoredParameterProgress::Pending),
            Poll::Failed => Ok(NativeWorkerAuthoredParameterProgress::Failed),
            Poll::Ready(reply) => reply.ok_or("native extension terminal differs"),
        }
    }
}
