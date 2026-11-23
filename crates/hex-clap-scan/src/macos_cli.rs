//! Binary-private Mac deadline and retained direct-child cleanup. No entry dispatch.
#[path = "supervisor_macos.rs"]
pub(super) mod supervisor;
use std::time::Duration;
pub(super) fn deadline(args: &[std::ffi::OsString], at: usize) -> Result<Duration, String> {
    hex_scan_supervisor::cli::parse_deadline_duration(args.get(at).map(AsRef::as_ref))
}
pub(super) fn finish(
    mut output: supervisor::Output,
    mut pending: Option<supervisor::Pending>,
) -> Result<supervisor::Output, Box<(String, supervisor::Output)>> {
    if let Some(lease) = pending.as_mut() {
        output.packet.clear();
        eprintln!("cleanup incomplete: direct-child owner retained; no successful result");
        loop {
            match lease.poll() {
                Ok(true) => break,
                Ok(false) => {}
                Err(e) => eprintln!("cleanup retained: {e}"),
            };
            std::thread::sleep(Duration::from_millis(20));
        }
        output.cleanup = lease.cleanup();
        return Err(Box::new((
            "cleanup completed after retained failure".into(),
            output,
        )));
    }
    Ok(output)
}
