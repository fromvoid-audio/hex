# hex

hex is a native Rust digital audio workstation using GPUI and CPAL. It is active
alpha software for macOS arm64. Feature completeness, Live compatibility and
native listening acceptance are ongoing work, tracked in [Pulse](https://pulse.iocore.dev/).
The private repository does not currently select a distribution license.

## Run

Install the native Rust/macOS build toolchain and use locked dependencies.
Save and quit hex before replacing the development app.

```sh
./launch          # verify/open the installed app without updating or building
./launch --update # update clean main, build/install optimized hex, then open
./launch --test   # also run full local CI
./launch --open   # verify/open the installed app without updating or building
```

The default app is `artifacts/hex.app`, identity `org.portalsurfer.hex.dev`.
Normal launch refuses running installed or raw desktop processes. Use
`./launch --help` for explicit restart and recovery options; never infer permission
to discard an unsaved project. An explicit `--update` build records tests as not run.
See [native builds](docs/test-builds.md) for qualified user-test delivery.
Use [`./release beta`](docs/releases.md) for immutable signed/notarized beta,
nightly and stable distribution packages.

To run the optimized desktop directly from source:

```sh
cargo run -r --locked
cargo run -r --locked -- --demo
```

Current development builds enable action and audio failure diagnostics by default,
including optimized builds. The recommended preset records semantic actions,
requests/outcomes, pointer/key intent and observed view changes together with
audio publication/readiness and transport. Raw callback timing remains off.

Use `./launch --open --diagnostics=off` to turn detailed tracing off for the next
launch, or `./launch --open --diagnostics` to select the recommended preset
explicitly. Save and quit an existing process before changing launch options.
`./launch --test --diagnostics=off` also preserves the opt-out after building.
`--diagnostics=all` additionally records every callback timing sample and can
overflow sooner. Explicit modes require a verified installed build advertising
diagnostic support; older bundles need rebuilding. Raw source runs accept the
same options after `--`.

When a failure occurs, note what you heard and the approximate time. Keep the
session's current log and its `.1.log` through `.3.log` siblings together; the
header repeats source/build/flags and segment metadata. The log button opens a
native viewer. Use the session, category and time controls or search to narrow
the list. Use older and newer lines to move through large sessions; filters
apply to the displayed page and live history may shift on refresh. Click a row
to inspect it, then shift-click another to select a range and copy it.
Times shown in the viewer are UTC. Refresh reads new records; the copied excerpt
is limited to 1 MiB and includes available session context and loss warnings.
Check loss counters before interpreting a missing event. New action
records redact typed names/search contents and paths, with counts, IDs and numeric
values retained. Existing status/error messages may contain paths or source text;
review a capture before sharing it. Input tracing is not complete replay: rapid
intermediate view changes, pointer movements, OS dialogs and text contents are
excluded. Native listening and device stress still require testing.

Detailed audio diagnostics are written to the existing session log
(**open session log** in the app):

```sh
cargo run -r --locked -- --audio-diagnostics
cargo run -r --locked -- --audio-diagnostics=publication,readiness,transport
```

Categories are `publication`, `readiness`, `transport` and `timing`; `all` enables
all four. Use `--audio-diagnostics=off` to disable audio tracing on the next launch
while leaving action diagnostics enabled, or `--diagnostics=off` to disable both.
Save and quit an existing process before starting a diagnostic run.
Records correlate worker submission, ordinary and eligible callback
preflight/application, command dequeue, priority stop and applied/rejected status
using output/revision/context identities. Dequeue is distinct from application. Timing
records use the native adapter's whole-callback measurement; they exclude the
trace push itself. Log timestamps are control receipt times, while frame fields
are audio observations. The callback retains at most 256 resource-free records;
control drains at most 64 per poll. Overflow loses diagnostics only and reports
sequence gaps/loss counters. A log does not establish physical audio deadlines.

Session log rows carry producer wall time, monotonic elapsed time and sequence
numbers. Messages are quoted/escaped; loss and truncation are explicit. The
active `.log` path stays stable as it rolls; `.1.log` through `.3.log` retain the
preceding segments, newest first. Include those sibling files when capturing a
failure. New sessions retain at most four 8 MiB segments plus one bounded
rollover header, with ten sessions retained; older logs expire normally.
A logging startup/shutdown timeout or file error reports incomplete logging.


The desktop is the default workspace member. Plain `cargo build` and `cargo test`
also select it; use `--workspace` to select all members or `-p` for a specific crate.
Omit `-r` only for a specific debugging need.

## Native widget sandbox

```sh
./launch-sandbox
```

One disposable native window catalogs the shared controls, appearance variants,
chrome, layouts and device compositions. The workspace category includes native
text/IME fields, the actual main-interface menu and sample-peak status, compact
buttons, gauges and scrolling rows. Select a named entry; edit its local
values, switch theme, disable supported controls, reset it or narrow the preview.
Plots use editable fixture values; they do not process audio. No project, audio,
file or preferences I/O occurs. The former `knob_review` app is included here.
The launcher builds the optimized sandbox from the current checkout, then opens
it. It works from any directory and does not update Git or replace `hex.app`.
Headless catalog/input checks use:

```sh
cargo test -p hex-sandbox --features ui-test-support --locked
```

## Check

```sh
python3 scripts/local-ci.py
python3 scripts/local-ci.py --profile merge --base BASE_SHA --source HEAD_SHA
python3 scripts/local-ci.py --quick  # partial feedback only
```

Merge requires a complete passing change-based check set on the final candidate,
with affected contracts, consumers and configurations covered or explicitly resolved.
Checks run locally; do not create or trigger hosted CI.
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
