# hex

hex is a native Rust digital audio workstation using GPUI and CPAL. It is active
alpha software for macOS arm64. Feature completeness, Live compatibility and
native listening acceptance are ongoing work, tracked in [Pulse](https://pulse.iocore.dev/).
The private repository does not currently select a distribution license.

## Run

Install the native Rust/macOS build toolchain and use locked dependencies.
Save and quit hex before replacing the development app.

```sh
./launch          # update clean main, build/install optimized hex, then open
./launch --test   # also run full local CI
./launch --open   # verify/open the installed app without updating or building
```

The default app is `artifacts/hex.app`, identity `org.portalsurfer.hex.dev`.
Normal launch refuses running installed or raw desktop processes. Use
`./launch --help` for explicit restart and recovery options; never infer permission
to discard an unsaved project. Launch without `--test` records tests as not run.
See [native builds](docs/test-builds.md) for qualified user-test delivery.

For a specific debugging need:

```sh
cargo run -p hex-desktop --locked
cargo run -p hex-desktop --locked -- --demo
```

## Check

```sh
python3 scripts/local-ci.py
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
python3 scripts/local-ci.py --quick  # partial feedback only
```

Only a complete passing required plan on the unchanged final source qualifies a
merge. Checks run locally; do not create or trigger hosted CI.
See [local CI](docs/local-ci.md) and [development workflow](docs/development-workflow.md).
Native UI, listening and physical audio deadlines require separate evidence.

## Headless tools

```sh
cargo run -p hex-cli --locked -- demo artifacts/hex-demo.wav
cargo run -p hex-cli --locked -- render-project project.hex output.wav
cargo run -p hex-cli --locked -- recover-project project.hex recovered.hex
python3 crates/hex-plugin-scan/native/build.py --help
```

Use original/licensed fixtures and create-new destinations; preserve projects and
media. Inspect command usage before importing, recovering or replacing files.
The standalone scanners print usage when run without arguments; they accept
controlled build receipts, not arbitrary installed third-party plugin paths.

## Find information

Code contracts, crate responsibilities, units and lifecycle rules live in the
owning Rust crate/module/API doc comments. Read the source and tests; use
`cargo doc --workspace --no-deps` to browse the API. Do not maintain parallel
feature specs or implementation journals in Markdown.

`docs/` contains practical contributor instructions. [Research](research/README.md)
contains recorded experiments, provenance and raw measurements. Requirements,
future work, acceptance checklists and progress belong in Pulse; code review and
check logs belong with the canonical PR and `artifacts/`.

Read [AGENTS.md](AGENTS.md) before contributing. Font and source notices remain in
[ATTRIBUTION.md](ATTRIBUTION.md) and the original license/provenance files.
