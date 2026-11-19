//! Bounded worker-side ordering behind an already-admitted graph publication.
//! The callback does not wake this worker; it polls scalar observations. The
//! parked request keeps its FIFO place, and later requests stay in the existing
//! bounded channel. No graph or audio resource is held here.
use super::{QueuedRequest, ResidentPublicationReadiness, ResidentRuntimeLoop};
use std::time::{Duration, Instant};

const DEADLINE: Duration = Duration::from_millis(500);
pub(super) const POLL: Duration = Duration::from_millis(2);

pub(super) struct Wait {
    request: QueuedRequest,
    output: u64,
    submission: Option<ResidentRuntimeLoop>,
    started: Instant,
}
impl Wait {
    #[allow(clippy::result_large_err)] // Return exact request custody without an extra allocation.
    pub(super) fn begin(
        mut request: QueuedRequest,
        readiness: ResidentPublicationReadiness,
    ) -> Result<Self, QueuedRequest> {
        let (output, submission, busy) = match readiness {
            ResidentPublicationReadiness::NoOutput => return Err(request),
            ResidentPublicationReadiness::Idle {
                output_identity,
                publication_in_flight,
                ..
            } => (output_identity, None, publication_in_flight),
            ResidentPublicationReadiness::Pending { submission, .. } => {
                (submission.output_identity, Some(submission), true)
            }
            ResidentPublicationReadiness::Applied {
                submission,
                publication_in_flight,
            }
            | ResidentPublicationReadiness::Refused {
                submission,
                publication_in_flight,
                ..
            } => (submission.output_identity, None, publication_in_flight),
        };
        if !busy {
            return Err(request);
        }
        request.publication_waited = true;
        Ok(Self {
            request,
            output,
            submission,
            started: Instant::now(),
        })
    }
    #[allow(clippy::result_large_err)] // The parked request remains owned on control.
    pub(super) fn poll(
        mut self,
        readiness: ResidentPublicationReadiness,
        document_intent: u64,
    ) -> Result<QueuedRequest, Self> {
        let failure = if self.request.document_intent != document_intent {
            Some("edit cancelled · project changed")
        } else if self.started.elapsed() >= DEADLINE {
            Some("edit cancelled · audio publication timed out")
        } else {
            match readiness {
                ResidentPublicationReadiness::NoOutput => {
                    Some("edit cancelled · audio output changed")
                }
                ResidentPublicationReadiness::Idle {
                    output_identity,
                    publication_in_flight,
                    ..
                } if output_identity == self.output && self.submission.is_none() => {
                    if publication_in_flight {
                        return Err(self);
                    }
                    None
                }
                ResidentPublicationReadiness::Pending { submission, .. }
                    if submission.output_identity == self.output
                        && self.submission == Some(submission) =>
                {
                    return Err(self);
                }
                ResidentPublicationReadiness::Applied {
                    submission,
                    publication_in_flight,
                } if submission.output_identity == self.output
                    && self.submission.is_none_or(|tag| tag == submission) =>
                {
                    if publication_in_flight {
                        return Err(self);
                    }
                    None
                }
                ResidentPublicationReadiness::Refused {
                    submission,
                    publication_in_flight,
                    ..
                } if submission.output_identity == self.output && self.submission.is_none() => {
                    if publication_in_flight {
                        return Err(self);
                    }
                    None
                }
                ResidentPublicationReadiness::Refused { submission, .. }
                    if self.submission == Some(submission) =>
                {
                    Some("edit cancelled · previous audio publication was refused")
                }
                _ => Some("edit cancelled · audio output changed"),
            }
        };
        self.request.publication_wait_failure = failure;
        Ok(self.request)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::service::Request;
    use std::sync::atomic::AtomicU64;

    fn tag() -> ResidentRuntimeLoop {
        ResidentRuntimeLoop {
            output_identity: 3,
            audio_revision: 4,
            source_context: None,
            source_origin: crate::service::ResidentSourceOrigin::Unknown,
            section_loop: None,
        }
    }
    fn pending() -> ResidentPublicationReadiness {
        ResidentPublicationReadiness::Pending {
            submission: tag(),
            applied_revision: 2,
            publication_in_flight: true,
        }
    }
    fn wait() -> Wait {
        let request = QueuedRequest::continuation(Request::Undo, &AtomicU64::new(9));
        match Wait::begin(request, pending()) {
            Ok(wait) => wait,
            Err(_) => panic!("pending publication must park"),
        }
    }
    #[test]
    fn waits_for_the_exact_application_and_reservation_release() {
        let wait = match wait().poll(pending(), 9) {
            Err(wait) => wait,
            Ok(_) => panic!("still pending"),
        };
        let wait = match wait.poll(
            ResidentPublicationReadiness::Applied {
                submission: tag(),
                publication_in_flight: true,
            },
            9,
        ) {
            Err(wait) => wait,
            Ok(_) => panic!("reservation still held"),
        };
        let request = match wait.poll(
            ResidentPublicationReadiness::Applied {
                submission: tag(),
                publication_in_flight: false,
            },
            9,
        ) {
            Ok(request) => request,
            Err(_) => panic!("ready"),
        };
        assert!(request.publication_wait_failure.is_none());
    }
    #[test]
    fn refuses_on_rejection_output_replacement_document_change_and_timeout() {
        let refused = ResidentPublicationReadiness::Refused {
            submission: tag(),
            applied_revision: 2,
            publication_in_flight: false,
        };
        let cases = [
            (wait(), refused, 9),
            (wait(), ResidentPublicationReadiness::NoOutput, 9),
            (wait(), pending(), 10),
        ];
        for (wait, readiness, intent) in cases {
            let request = match wait.poll(readiness, intent) {
                Ok(request) => request,
                Err(_) => panic!("must refuse"),
            };
            assert!(request.publication_wait_failure.is_some());
        }
        let mut expired = wait();
        expired.started = Instant::now() - DEADLINE;
        let request = match expired.poll(pending(), 9) {
            Ok(request) => request,
            Err(_) => panic!("deadline"),
        };
        assert!(
            request
                .publication_wait_failure
                .unwrap()
                .contains("timed out")
        );
    }
}
