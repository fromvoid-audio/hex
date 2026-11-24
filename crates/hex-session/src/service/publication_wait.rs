//! Bounded worker-side ordering behind an already-admitted graph publication.
//! The callback does not wake this worker; it polls scalar observations. The
//! parked request keeps its FIFO place, and later requests stay in the existing
//! bounded channel. No graph or audio resource is held here.
use super::{QueuedRequest, ResidentPublicationReadiness, ResidentRuntimeLoop};
use std::time::{Duration, Instant};

pub(super) const DEADLINE: Duration = Duration::from_millis(500);
pub(super) const POLL: Duration = Duration::from_millis(2);

/// The switch and its early display have already committed. A held EQ pair can
/// rebase only after the exact canonical publication releases the output slot.
/// The session worker polls the audio control owner; the callback never waits.
pub(super) fn wait_for_switch_rebase(
    audio: &mut dyn super::WorkerAudioSink,
    expected: super::ResidentGraphSubmission,
    cancelled: impl Fn() -> bool,
) -> Result<(), super::SessionError> {
    let start = Instant::now();
    loop {
        if cancelled() {
            return Err(super::SessionError::Invalid(
                "paired preview cancelled · source changed",
            ));
        }
        let readiness = audio.resident_publication_readiness();
        if expected.matches_applied(readiness) {
            return Ok(());
        }
        match readiness {
            ResidentPublicationReadiness::Pending { submission, .. }
            | ResidentPublicationReadiness::Applied { submission, .. }
                if expected.matches_runtime_loop(submission) => {}
            ResidentPublicationReadiness::Refused { submission, .. }
                if expected.matches_runtime_loop(submission) =>
            {
                return Err(super::SessionError::Invalid(
                    "paired preview cancelled · track switch publication refused",
                ));
            }
            _ => {
                return Err(super::SessionError::Invalid(
                    "paired preview cancelled · audio output changed",
                ));
            }
        }
        if start.elapsed() >= DEADLINE {
            return Err(super::SessionError::Invalid(
                "paired preview cancelled · audio publication timed out",
            ));
        }
        audio.poll();
        std::thread::sleep(POLL);
    }
}

pub(super) enum Step {
    Ready(QueuedRequest),
    Waiting(Wait),
}

pub(super) struct Wait {
    request: QueuedRequest,
    output: u64,
    submission: Option<ResidentRuntimeLoop>,
    started: Instant,
}
impl Wait {
    pub(super) fn begin(
        mut request: QueuedRequest,
        readiness: ResidentPublicationReadiness,
    ) -> Step {
        let (output, submission, busy) = match readiness {
            ResidentPublicationReadiness::NoOutput => return Step::Ready(request),
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
            return Step::Ready(request);
        }
        request.publication_waited = true;
        Step::Waiting(Self {
            request,
            output,
            submission,
            started: Instant::now(),
        })
    }
    pub(super) fn poll(
        mut self,
        readiness: ResidentPublicationReadiness,
        document_intent: u64,
    ) -> Step {
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
                        return Step::Waiting(self);
                    }
                    None
                }
                ResidentPublicationReadiness::Pending { submission, .. }
                    if submission.output_identity == self.output
                        && self.submission.is_none_or(|tag| tag == submission) =>
                {
                    return Step::Waiting(self);
                }
                ResidentPublicationReadiness::Applied {
                    submission,
                    publication_in_flight,
                } if submission.output_identity == self.output
                    && self.submission.is_none_or(|tag| tag == submission) =>
                {
                    if publication_in_flight {
                        return Step::Waiting(self);
                    }
                    None
                }
                ResidentPublicationReadiness::Refused {
                    submission,
                    publication_in_flight,
                    ..
                } if submission.output_identity == self.output && self.submission.is_none() => {
                    if publication_in_flight {
                        return Step::Waiting(self);
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
        Step::Ready(self.request)
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
            Step::Waiting(wait) => wait,
            Step::Ready(_) => panic!("pending publication must park"),
        }
    }
    #[test]
    fn waits_for_the_exact_application_and_reservation_release() {
        let wait = match wait().poll(pending(), 9) {
            Step::Waiting(wait) => wait,
            Step::Ready(_) => panic!("still pending"),
        };
        let wait = match wait.poll(
            ResidentPublicationReadiness::Applied {
                submission: tag(),
                publication_in_flight: true,
            },
            9,
        ) {
            Step::Waiting(wait) => wait,
            Step::Ready(_) => panic!("reservation still held"),
        };
        let request = match wait.poll(
            ResidentPublicationReadiness::Applied {
                submission: tag(),
                publication_in_flight: false,
            },
            9,
        ) {
            Step::Ready(request) => request,
            Step::Waiting(_) => panic!("ready"),
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
                Step::Ready(request) => request,
                Step::Waiting(_) => panic!("must refuse"),
            };
            assert!(request.publication_wait_failure.is_some());
        }
        let mut expired = wait();
        expired.started = Instant::now() - DEADLINE;
        let request = match expired.poll(pending(), 9) {
            Step::Ready(request) => request,
            Step::Waiting(_) => panic!("deadline"),
        };
        assert!(
            request
                .publication_wait_failure
                .unwrap()
                .contains("timed out")
        );
    }
}
