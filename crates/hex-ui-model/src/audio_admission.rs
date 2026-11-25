//! Correlates canonical project revisions with asynchronously admitted audio.
//!
//! Canonical edits are owned by the session worker and remain committed if a
//! callback-side graph handoff is rejected. This UI-side record reports that
//! distinction without feeding admission status back into project history.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectRevision {
    pub generation: u64,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StoppedPlaybackRestore {
    WaitingForEdits { generation: u64, output: u64 },
    Preparing,
    Ready,
    Published(u64),
}

/// A canonical or native latency change can rebuild after a faded Stop.
/// The output and project identities fence both the stop and the later resume.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LatencyRebuild {
    Unresolved {
        generation: u64,
        output: u64,
        audio_revision: u64,
    },
    Waiting {
        project: ProjectRevision,
        output: u64,
        runtime_audio: Option<u64>,
    },
    Fading {
        project: ProjectRevision,
        output: u64,
        frame: u64,
        ticket: u64,
        runtime_audio: Option<u64>,
    },
    Restoring {
        project: ProjectRevision,
        output: u64,
        frame: u64,
        resume: bool,
        runtime_audio: Option<u64>,
        installed: bool,
    },
}

/// A live canonical edit refused by a held Arrangement gate, a session runtime
/// conflict, or a paused foreign tail can leave audio on the prior graph. Track
/// the exact refused revision so pausing republishes it once (or marks it
/// exhausted when already paused) instead of stranding playback on an older
/// graph.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PausedRepublish<R> {
    Unresolved {
        generation: u64,
        output: u64,
        audio_revision: u64,
        reason: R,
    },
    WaitingForPause {
        project: ProjectRevision,
        output: u64,
        audio_revision: u64,
    },
    Submitted {
        project: ProjectRevision,
        output: u64,
        rejected_audio_revision: u64,
    },
    Exhausted {
        project: ProjectRevision,
        output: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionState {
    Pending,
    Applied,
    Rejected(&'static str),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CurrentAdmission {
    pub project: ProjectRevision,
    /// None means the current canonical revision has not yet been published
    /// to the audio callback; an assigned revision remains pending until a
    /// terminal applied/rejected acknowledgment arrives.
    pub audio_revision: Option<u64>,
    pub state: AdmissionState,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AudibleRevision {
    pub project: ProjectRevision,
    pub audio_revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct AudioAdmissionStatus {
    current: Option<CurrentAdmission>,
    last_audible: Option<AudibleRevision>,
}

impl AudioAdmissionStatus {
    pub fn published_audio_revision(&self, generation: u64, project_revision: u64) -> Option<u64> {
        self.current
            .filter(|current| {
                current.project.generation == generation
                    && current.project.revision == project_revision
            })
            .and_then(|current| current.audio_revision)
    }

    pub fn current(&self) -> Option<CurrentAdmission> {
        self.current
    }

    pub fn last_audible(&self) -> Option<AudibleRevision> {
        self.last_audible
    }

    /// Start tracking a canonical revision. Repeated events for the same
    /// revision preserve its terminal status; a generation change distinguishes
    /// a replacement project that reuses the same revision number.
    pub fn canonical_revision(&mut self, project: ProjectRevision) -> bool {
        if self
            .current
            .is_none_or(|current| current.project != project)
        {
            self.current = Some(CurrentAdmission {
                project,
                audio_revision: None,
                state: AdmissionState::Pending,
            });
            true
        } else {
            false
        }
    }

    /// Sealed comparison authoring proof changes correlation, not audio status.
    /// Applied requires the caller's actual current candidate/output association.
    pub fn comparison_revision(
        &mut self,
        previous: ProjectRevision,
        next: ProjectRevision,
        applied_is_current: bool,
    ) {
        if let Some(current) = self.current.as_mut()
            && current.project == previous
            && (current.state != AdmissionState::Applied || applied_is_current)
        {
            current.project = next;
        }
        if applied_is_current
            && let Some(audible) = self.last_audible.as_mut()
            && audible.project == previous
        {
            audible.project = next;
        }
    }

    pub fn published(&mut self, project: ProjectRevision, audio_revision: u64) -> bool {
        if let Some(current) = self.current.as_mut()
            && current.project == project
        {
            // A newer publication for the same canonical state supersedes an
            // earlier attempt, so stale rejection telemetry cannot overwrite it.
            if current
                .audio_revision
                .is_none_or(|previous| audio_revision >= previous)
            {
                let before = *current;
                current.audio_revision = Some(audio_revision);
                current.state = AdmissionState::Pending;
                return *current != before;
            }
        }
        false
    }

    pub fn rejected(
        &mut self,
        project: ProjectRevision,
        audio_revision: u64,
        reason: &'static str,
    ) -> bool {
        if let Some(current) = self.current.as_mut()
            && current.project == project
            && current.audio_revision == Some(audio_revision)
            && current.state != AdmissionState::Applied
        {
            current.state = AdmissionState::Rejected(reason);
            return true;
        }
        false
    }

    /// A control-side refusal can precede any submitted audio revision. Keep
    /// its canonical state visible after transient event text has changed.
    pub fn unpublished_refusal(&mut self, project: ProjectRevision, reason: &'static str) -> bool {
        if let Some(current) = self.current.as_mut()
            && current.project == project
            && current.audio_revision.is_none()
            && current.state != AdmissionState::Applied
        {
            let before = current.state;
            current.state = AdmissionState::Rejected(reason);
            return before != current.state;
        }
        false
    }

    /// Record only a candidate explicitly promoted by the applied-revision
    /// high-water mark. A stale candidate can update the last-audible record,
    /// but it cannot replace a newer pending/applied current revision.
    pub fn applied(&mut self, project: ProjectRevision, audio_revision: u64) -> bool {
        let audible = AudibleRevision {
            project,
            audio_revision,
        };
        let mut changed = false;
        if self
            .last_audible
            .is_none_or(|previous| audio_revision >= previous.audio_revision)
        {
            let next = Some(audible);
            changed = self.last_audible != next;
            self.last_audible = next;
        }
        if let Some(current) = self.current.as_mut()
            && current.project == project
            && current
                .audio_revision
                .is_none_or(|previous| audio_revision >= previous)
        {
            let before = *current;
            current.audio_revision = Some(audio_revision);
            current.state = AdmissionState::Applied;
            changed |= *current != before;
        }
        changed
    }

    /// Audio recovery starts a fresh callback status domain. The canonical
    /// project stays committed, while the audible graph must be re-established.
    pub fn reset_audio(&mut self) {
        let project = self.current.map(|current| current.project);
        *self = Self::default();
        if let Some(project) = project {
            self.canonical_revision(project);
        }
    }

    /// A transient graph is now audible. Preserve any newer canonical
    /// publication already pending, but do not claim the old canonical graph
    /// remains installed or audible.
    pub fn preview_applied(&mut self, audio_revision: u64) -> bool {
        let mut changed = self.last_audible.take().is_some();
        if let Some(current) = self.current.as_mut()
            && current.state == AdmissionState::Applied
            && current
                .audio_revision
                .is_some_and(|revision| revision <= audio_revision)
        {
            current.audio_revision = None;
            current.state = AdmissionState::Pending;
            changed = true;
        }
        changed
    }

    pub fn label(&self) -> String {
        let Some(current) = self.current else {
            return "audio revision unavailable".to_owned();
        };
        let revision = current.project.revision;
        let state = match current.state {
            AdmissionState::Pending => "pending".to_owned(),
            AdmissionState::Applied => "applied".to_owned(),
            AdmissionState::Rejected(_) => "rejected".to_owned(),
        };
        let mut label = format!("rev {revision} {state}");
        if let Some(audible) = self.last_audible
            && (audible.project != current.project
                || current.state != AdmissionState::Applied
                || current.audio_revision != Some(audible.audio_revision))
        {
            label.push_str(&format!(" · rev {} audible", audible.project.revision));
        }
        label
    }

    pub fn description(&self) -> String {
        let Some(current) = self.current else {
            return "audio admission status is unavailable".to_owned();
        };
        let audio = current.audio_revision.map_or_else(
            || "not published".to_owned(),
            |revision| revision.to_string(),
        );
        let state = match current.state {
            AdmissionState::Pending => "pending".to_owned(),
            AdmissionState::Applied => "applied".to_owned(),
            AdmissionState::Rejected(reason) => format!("rejected: {reason}"),
        };
        let mut description = format!(
            "project generation {} revision {} · audio revision {audio} · {state}",
            current.project.generation, current.project.revision
        );
        if let Some(audible) = self.last_audible {
            description.push_str(&format!(
                " · last audible: project generation {} revision {} · audio revision {}",
                audible.project.generation, audible.project.revision, audible.audio_revision
            ));
        }
        description
    }
}

#[cfg(test)]
mod tests {
    use super::{AdmissionState, AudioAdmissionStatus, ProjectRevision};

    fn project(generation: u64, revision: u64) -> ProjectRevision {
        ProjectRevision {
            generation,
            revision,
        }
    }

    #[test]
    fn changed_media_rejection_reaches_the_admission_description() {
        let current = project(2, 17);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(current);
        status.published(current, 31);
        let reason = "original media changed · reopen or relink the source";
        assert!(status.rejected(current, 31, reason));
        assert!(status.description().contains("original media changed"));
        assert!(status.description().contains("reopen or relink"));
    }

    #[test]
    fn current_revision_transitions_pending_applied_and_rejected() {
        let current = project(2, 17);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(current);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);

        // Queue/full or deferred callback admission has no terminal event.
        status.published(current, 31);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
        assert_eq!(status.current().unwrap().audio_revision, Some(31));
        assert_eq!(status.last_audible(), None);

        let reason = "source history unavailable · previous audio continues";
        assert!(status.rejected(current, 31, reason));
        assert_eq!(
            status.current().unwrap().state,
            AdmissionState::Rejected(reason)
        );
        assert!(status.description().contains("previous audio continues"));
        assert!(status.description().contains("audio revision 31"));
        assert!(status.description().contains("rejected:"));

        // A later admission of the same canonical revision can recover it.
        status.published(current, 32);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
        status.applied(current, 32);
        assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
        assert_eq!(status.last_audible().unwrap().audio_revision, 32);
    }

    #[test]
    fn stale_rejection_cannot_clobber_newer_pending_or_replacement_revision() {
        let old = project(4, 20);
        let current = project(4, 21);
        let replacement = project(5, 0);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(old);
        status.published(old, 50);
        status.canonical_revision(current);
        status.published(current, 51);
        assert!(!status.rejected(old, 50, "old revision rejected"));
        assert_eq!(status.current().unwrap().project, current);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);

        status.canonical_revision(replacement);
        status.published(replacement, 52);
        assert!(!status.rejected(current, 51, "old project rejected"));
        assert_eq!(status.current().unwrap().project, replacement);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
    }

    #[test]
    fn older_rejection_does_not_replace_newer_published_candidate() {
        let current = project(3, 8);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(current);
        status.published(current, 10);
        status.published(current, 11);
        assert!(!status.rejected(current, 10, "coalesced stale rejection"));
        status.applied(current, 11);
        assert_eq!(status.current().unwrap().audio_revision, Some(11));
        assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
        assert_eq!(status.last_audible().unwrap().audio_revision, 11);
    }

    #[test]
    fn stale_applied_candidate_does_not_replace_current_or_latest_audibility() {
        let old = project(1, 9);
        let current = project(1, 10);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(old);
        status.published(old, 3);
        status.canonical_revision(current);
        status.published(current, 4);
        status.applied(old, 3);
        assert_eq!(status.current().unwrap().project, current);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
        assert_eq!(status.last_audible().unwrap().project, old);
        assert!(status.label().contains("rev 9 audible"));
        assert!(
            status
                .description()
                .contains("project generation 1 revision 9")
        );
        status.applied(current, 4);
        status.applied(old, 3);
        assert_eq!(status.current().unwrap().state, AdmissionState::Applied);
        assert_eq!(status.last_audible().unwrap().project, current);
    }

    #[test]
    fn audio_reset_keeps_canonical_identity_pending_and_clears_old_audibility() {
        let current = project(7, 40);
        let mut status = AudioAdmissionStatus::default();
        status.canonical_revision(current);
        status.published(current, 2);
        status.applied(current, 2);
        status.reset_audio();
        assert_eq!(status.current().unwrap().project, current);
        assert_eq!(status.current().unwrap().audio_revision, None);
        assert_eq!(status.current().unwrap().state, AdmissionState::Pending);
        assert_eq!(status.last_audible(), None);
    }

    #[test]
    fn comparison_correlation_keeps_pending_rejected_and_real_audio_revision() {
        let old = ProjectRevision {
            generation: 3,
            revision: 7,
        };
        let next = ProjectRevision {
            generation: 3,
            revision: 8,
        };
        for rejected in [false, true] {
            let mut state = AudioAdmissionStatus::default();
            state.canonical_revision(old);
            state.published(old, 91);
            if rejected {
                state.rejected(old, 91, "refused");
            }
            state.comparison_revision(old, next, false);
            assert_eq!(state.current().unwrap().project, next);
            assert_eq!(state.current().unwrap().audio_revision, Some(91));
            assert_eq!(
                state.current().unwrap().state,
                if rejected {
                    AdmissionState::Rejected("refused")
                } else {
                    AdmissionState::Pending
                }
            );
            assert!(state.last_audible().is_none());
        }
        let mut state = AudioAdmissionStatus::default();
        state.canonical_revision(old);
        state.applied(old, 91);
        state.comparison_revision(old, next, false);
        assert_eq!(
            state.current().unwrap().project,
            old,
            "no actual candidate/output association means no applied promotion"
        );
        state.comparison_revision(old, next, true);
        state.comparison_revision(old, next, true);
        assert_eq!(state.current().unwrap().project, next);
        assert_eq!(state.last_audible().unwrap().project, next);
        assert_eq!(state.current().unwrap().audio_revision, Some(91));
    }
}
