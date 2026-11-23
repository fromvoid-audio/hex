//! Direct-child ownership for the enrolled ORIGINAL never-fork Mac profile only.
//! No subreaper, descendant observation/reaping or containment claim.
pub use hex_scan_supervisor::unix::{DirectOutput as Output, DirectPending as Pending};
use std::{path::Path, time::Duration};

pub fn run(
    helper: &Path,
    module: &Path,
    request: u64,
    deadline: Duration,
    input: &[u8],
) -> Result<(Output, Option<Pending>), String> {
    hex_scan_supervisor::unix::run_direct_child(
        helper,
        module,
        request,
        deadline,
        input,
        hex_clap_scan::MAX_PACKET,
    )
}
