use std::{
    fs,
    path::PathBuf,
    process::Command as ProcessCommand,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path =
            std::env::temp_dir().join(format!("hex-cli-help-tests-{}-{id}", std::process::id()));
        fs::create_dir(&path).expect("create test directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

const ALL_COMMANDS: [&str; 7] = [
    "demo",
    "edit-demo",
    "render-wav",
    "import-project",
    "render-project",
    "export-project",
    "recover-project",
];

#[test]
fn help_flag_lists_all_seven_subcommands_exits_zero_and_writes_no_files() {
    let dir = TempDir::new();
    for flag in ["--help", "-h", "help"] {
        let output = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
            .current_dir(&dir.0)
            .arg(flag)
            .output()
            .expect("run hex-cli help");
        assert!(
            output.status.success(),
            "expected exit 0 for {flag}, stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        for command in ALL_COMMANDS {
            assert!(
                stdout.contains(command),
                "help output for {flag} missing command {command:?}: {stdout}"
            );
        }
        assert!(
            fs::read_dir(&dir.0).unwrap().next().is_none(),
            "help invocation {flag} must not create any files"
        );
    }
}

#[test]
fn unknown_command_and_missing_args_exit_nonzero_with_complete_usage() {
    let dir = TempDir::new();
    let unknown = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .current_dir(&dir.0)
        .arg("not-a-command")
        .output()
        .expect("run hex-cli unknown");
    assert!(!unknown.status.success());
    let stderr = String::from_utf8_lossy(&unknown.stderr);
    assert!(stderr.contains("not-a-command"), "stderr: {stderr}");
    for command in ALL_COMMANDS {
        assert!(
            stderr.contains(command),
            "unknown-command usage missing {command:?}: {stderr}"
        );
    }

    let empty = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .current_dir(&dir.0)
        .output()
        .expect("run hex-cli with no args");
    assert!(!empty.status.success());
    let empty_stderr = String::from_utf8_lossy(&empty.stderr);
    for command in ALL_COMMANDS {
        assert!(
            empty_stderr.contains(command),
            "no-args usage missing {command:?}: {empty_stderr}"
        );
    }

    let bad_arity = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .current_dir(&dir.0)
        .arg("render-project")
        .output()
        .expect("run hex-cli render-project without args");
    assert!(!bad_arity.status.success());
    assert!(
        String::from_utf8_lossy(&bad_arity.stderr)
            .contains("usage: hex-cli render-project <project.hex> <new-output.wav>")
    );

    let bad_demo = ProcessCommand::new(env!("CARGO_BIN_EXE_hex-cli"))
        .current_dir(&dir.0)
        .arg("demo")
        .output()
        .expect("run hex-cli demo without output");
    assert!(!bad_demo.status.success());
    assert!(
        String::from_utf8_lossy(&bad_demo.stderr)
            .contains("usage: hex-cli <demo|edit-demo> <new-output.wav>")
    );

    assert!(fs::read_dir(&dir.0).unwrap().next().is_none());
}
