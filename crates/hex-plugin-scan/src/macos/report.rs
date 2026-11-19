//! Copied standalone scan records, with no process-global signal or child ownership.
use super::identity::{Bundle, Executable};
use crate::Discovery;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum GroupOutcome {
    Sent,
    Absent,
    PermissionDenied { errno: i32 },
    Failed { errno: i32, error: String },
}
impl GroupOutcome {
    pub fn compatible_with_direct_retirement(&self, exited: bool) -> bool {
        matches!(self, Self::Sent | Self::Absent)
            || (exited && matches!(self, Self::PermissionDenied { .. }))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Cleanup {
    pub direct_reaped: bool,
    pub direct_exit_code: Option<i32>,
    pub direct_exit_signal: Option<i32>,
    pub custody_lost: bool,
    pub cleanup_unconfirmed: bool,
    pub exit_observed_before_signals: bool,
    pub original_group_term: Option<GroupOutcome>,
    pub original_group_kill: Option<GroupOutcome>,
    pub direct_kill: Option<String>,
    pub descendants: String,
    pub pending: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Report {
    pub profile: String,
    pub request: u64,
    pub generation: u64,
    pub receipt_sha256: String,
    pub helper_sha256: String,
    pub bundle: Bundle,
    pub executable: Option<Executable>,
    pub bundle_identifier: Option<String>,
    pub discovery: Option<Discovery>,
    pub interruption: Option<String>,
    pub error: Option<String>,
    pub stdout: String,
    pub stderr: String,
    pub cleanup: Cleanup,
}

impl Cleanup {
    pub fn may_publish(&self) -> bool {
        self.direct_reaped
            && self.direct_exit_code == Some(0)
            && !self.pending
            && !self.custody_lost
            && !self.cleanup_unconfirmed
            && [
                self.original_group_term.as_ref(),
                self.original_group_kill.as_ref(),
            ]
            .into_iter()
            .all(|value| {
                value.is_some_and(|outcome| {
                    outcome.compatible_with_direct_retirement(self.exit_observed_before_signals)
                })
            })
    }
}
