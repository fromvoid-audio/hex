//! Headless common-Unix ownership controls. Passing on Linux is not a Darwin result.
#![cfg(target_os = "linux")]
#[path = "../src/supervisor_macos.rs"]
mod supervisor;
#[test]
#[ignore = "requires freshly built ORIGINAL Linux helper root; common Unix backend only"]
fn original_never_fork_success_crash_and_deadline_are_actually_waited() {
    let root = std::path::PathBuf::from(
        std::env::var_os("HEX_CLAP_DIRECT_CHILD_ROOT").expect("original root"),
    );
    for (name, success) in [("good", true), ("crash", false), ("hang", false)] {
        let (out, pending) = supervisor::run(
            &root.join("hex-clap-probe"),
            &root.join(format!("{name}.clap")),
            1,
            std::time::Duration::from_millis(200),
            &[],
        )
        .unwrap();
        assert!(pending.is_none());
        assert!(out.cleanup.direct_reaped);
        assert!(!out.cleanup.pending);
        assert_eq!(out.cause.is_none(), success, "{out:?}");
        assert_eq!(
            out.cleanup.descendant_policy,
            "not-applicable-audited-original-never-fork"
        );
    }
}
