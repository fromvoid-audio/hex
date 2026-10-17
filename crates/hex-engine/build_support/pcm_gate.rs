//! Version-bound declaration selector; not a security/allocator attestation.
pub const COMMIT: &str = "4a4ef493e3a1488c6e321570238084b38948f6db";
#[derive(Clone, Copy)]
pub struct Observation<'a> {
    pub verbose: &'a str,
    pub host: &'a str,
    pub target: &'a str,
    pub ordinary_install: bool,
    pub unsupported_inputs: bool,
}
pub fn decide(observed: Observation<'_>) -> Result<(), &'static str> {
    if observed.unsupported_inputs {
        return Err("unsupported_wrapper_or_ambiguous_flags");
    }
    if !observed.ordinary_install {
        return Err("missing_or_nonstandard_install_metadata");
    }
    if observed.host != observed.target
        || !matches!(
            observed.target,
            "x86_64-unknown-linux-gnu" | "aarch64-apple-darwin"
        )
    {
        return Err("unsupported_or_cross_target");
    }
    if field(observed.verbose, "release: ") != Some("1.94.0")
        || field(observed.verbose, "commit-hash: ") != Some(COMMIT)
        || field(observed.verbose, "host: ") != Some(observed.host)
    {
        return Err("unsupported_or_malformed_compiler");
    }
    Ok(())
}
fn field<'a>(value: &'a str, key: &str) -> Option<&'a str> {
    let mut values = value.lines().filter_map(|line| line.strip_prefix(key));
    let first = values.next()?;
    values.next().is_none().then_some(first)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn check(
        verbose: &str,
        host: &str,
        target: &str,
        installed: bool,
        flags: bool,
    ) -> Result<(), &'static str> {
        decide(Observation {
            verbose,
            host,
            target,
            ordinary_install: installed,
            unsupported_inputs: flags,
        })
    }
    #[test]
    fn ordinary_native_supported_and_every_unsupported_input_refuses() {
        for target in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin"] {
            let metadata = format!("release: 1.94.0\ncommit-hash: {COMMIT}\nhost: {target}\n");
            assert!(check(&metadata, target, target, true, false).is_ok());
            assert!(check(&metadata, target, target, false, false).is_err());
            assert!(check(&metadata, target, target, true, true).is_err());
            assert!(check(&metadata, target, "unknown", true, false).is_err());
            assert!(check(&metadata, "other", target, true, false).is_err());
            assert!(
                check(
                    &metadata.replace(COMMIT, "unknown"),
                    target,
                    target,
                    true,
                    false
                )
                .is_err()
            );
            assert!(
                check(
                    &metadata.replace("1.94.0", "1.94.1"),
                    target,
                    target,
                    true,
                    false
                )
                .is_err()
            );
            assert!(
                check(
                    &format!("{metadata}release: 1.94.0\n"),
                    target,
                    target,
                    true,
                    false
                )
                .is_err()
            );
            assert!(check("", target, target, true, false).is_err());
        }
    }
}
