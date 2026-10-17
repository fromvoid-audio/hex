# Playback construction stack probe

The opt-in `hex-engine` example `playback_stack` probes the stack reservation
needed to construct and preroll `SessionPlayback`. Run both profiles from the
repository root:

```sh
CARGO_INCREMENTAL=0 cargo run -p hex-engine --features instrument-test-support --example playback_stack
CARGO_INCREMENTAL=0 cargo run --release -p hex-engine --features instrument-test-support --example playback_stack
```

Each stack-size trial launches a child copy of the example. The child prepares
the canonical project revision before creating a worker with the requested
stack reservation. That worker constructs interactive `SessionPlayback`
candidates, prerolls them, and renders held and released output. A stack
overflow aborts only the child process. The parent treats only an explicit
stack-overflow diagnostic as a failed stack trial; other child failures stop the
probe as errors. The search checks the 512 KiB floor and the existing 8 MiB
production reservation, then narrows a failed/passed bracket to 1 MiB when the
floor fails. The 8 MiB trial is a required upper bound: if it fails, the probe
exits with an error instead of expanding beyond the production worker contract.

The two deterministic fixtures are:

- `utility`: one sine-instrument track with a stateful Utility DC-removal filter,
  routed at 0.5 gain to one Return track with its own DC-removal Utility filter.
  It contains one held MIDI note.
- `dense-mutable`: 100 tracks, each with a registered test-only mutable
  instrument, 32 simultaneous one-beat notes, a stateful Utility DC-removal
  filter, and a 0.5 send to a shared Return track with a stateful Utility filter.
  The probe prepares a held candidate at frame 512 and a released candidate at
  frame 24,704 (48 kHz, 120 BPM), then renders 256 frames from each. The toy
  instrument is a stack fixture, not a product device or a performance model.

The probe checks the reported playback clocks, finite non-silent PCM, and
identical held/released sample digests at each passing stack size. It measures
whether the whole worker operation completes at a requested stack reservation;
it does **not** measure peak/high-water stack use. Revision construction is
outside the worker stack being probed. It does not exercise GPUI, a native audio
callback, physical device deadlines, or an actual user project.

## Recorded run

Source base: `07976097cfb54e1d09993f3d95cab8f917f44a89`, before Return-effect
tail-retirement changes. Host: Apple M5 Pro, macOS 26.6.2 (25G83),
`aarch64-apple-darwin`, rustc 1.98.1 (`48a229cea`, LLVM 22.1.8). Stack values
are Rust `thread::Builder::stack_size` requests; they are not measured committed
stack bytes.

| profile | fixture | last failing request | first passing request | held digest | released digest |
| --- | --- | ---: | ---: | --- | --- |
| debug | utility | 1 MiB | 2 MiB | `2d4dc69f34a2c077` | `0037b4c5ed926441` |
| debug | dense mutable | 1 MiB | 2 MiB | `1ced8046c53d4a4b` | `1ebf3b887bf37b1f` |
| release | utility | none at 512 KiB floor | 512 KiB | `2d4dc69f34a2c077` | `0037b4c5ed926441` |
| release | dense mutable | none at 512 KiB floor | 512 KiB | `1ced8046c53d4a4b` | `1ebf3b887bf37b1f` |

The existing 8 MiB seek-worker reservation passed every case. Debug evidence
brackets completion between 1 and 2 MiB at the probe's 1 MiB resolution; release
completed at the 512 KiB probe floor, so its lower bound was not measured. These
are bounded fixture results, not a universal minimum or a peak-stack guarantee.
Keep constructor/preroll work on the dedicated control worker and retain the
8 MiB production reservation until broader workload and platform evidence
supports a deliberate change.

Raw console logs are retained locally (ignored by Git) under
`artifacts/profiles/playback-stack-20261010/0797609/`: `debug.txt`,
`release.txt`, `fmt.txt`, `clippy.txt`, `architecture.txt`, and `diff-check.txt`.
Exact commands, from the repository root, were:

```sh
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/portalsurfer/dev/hex/artifacts/build-cache/operator-fixed-tuning cargo run -p hex-engine --features instrument-test-support --example playback_stack --locked
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/portalsurfer/dev/hex/artifacts/build-cache/operator-fixed-tuning cargo run --release -p hex-engine --features instrument-test-support --example playback_stack --locked
cargo fmt --all --check
python3 scripts/check-architecture.py
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=/Users/portalsurfer/dev/hex/artifacts/build-cache/operator-fixed-tuning cargo clippy -p hex-engine --all-targets --features instrument-test-support --locked -- -D warnings
git diff --check
```

The recorded run used the source base above plus the focused probe sources
identified below. Their SHA-256 identities at capture were:

- `crates/hex-engine/Cargo.toml`:
  `b38351a4e998fd3df7aa4f3f46d67b204b79978e53eba817b5d5c305dc987d35`
- `crates/hex-engine/examples/playback_stack.rs`:
  `b28d8b64164be272f0f8e0cbc7940a5a769feeb7aebfc8e5825c9d0c7c06bb9e`
- `crates/hex-engine/examples/playback_stack/mutable_instrument.rs`:
  `78277bd81dc1499deb44c11312649ed9e7c63d6f59248144ce2f9b9df8ff67d1`

The debug executable SHA-256 is
`268119ff427055ad72c9f3dea30ce01655669d15f7c54d59bdb53ecc33b119fe`; the
release executable SHA-256 is
`6f1f674387054ad4fa40e1c4bb6b6075dc7738b74d19c7ebcf3a20afec71d4ed`. Both
were built with `CARGO_INCREMENTAL=0`, the `instrument-test-support` feature,
and `--locked` in the assigned local target directory.

## Detached-effect fixture residency, 17 October 2026

Pulse2787 / PR393 addresses a separate default-debug-stack overflow in
`session_playback::tests::detached_arrangement_effect_suffix_renders_after_its_source_ends`.
Both clean combined parent327ebe98 and spectrum repair96ee313a reproduce the same
SIGABRT with `RUST_MIN_STACK` unset on Linux x86-64/Rust1.94. Their independently
built engine test binaries are byte-identical:22042888bytes,
SHA25636245a4db3e0a247fec7c47e5183dd0369da1962bb9b76c5930f75a32d8e35a4.
The parent release version of this exact fixture passes on default test stacks.

The original debug fixture reserves1537288bytes in its own function frame,
keeping a reference and four old/new playback transition pairs as inline locals.
Its helper reserves227096bytes and the shared `new_with_policy` constructor
247896bytes, before deeper calls. These are compiler prologue observations, not
peak/high-water stack measurements. Test source first appeared in41ce6c5f,
Pulse2203/PR93; the cloud16MiB fixture setting documented by2348/PR160 predates
this repair and does not prove default-stack/native qualification.

The test now constructs and renders each unchanged transition in one helper
invocation. All source projects, partitioned reference output, unchanged-edit,
bypass, configuration and mix cases, retained-group/active flags and PCM
comparisons remain. The debug top-level fixture frame shrinks to31016bytes and
its largest transition helper reserves350312bytes. No production constructor,
callback storage, allocation, lifetime or stack reservation changes. The exact
fixture passes in debug and release without a stack override after this change.
This demonstrates the fixture correction; it does not guarantee every graph's
production stack usage or establish physical deadlines.

A broader default-stack engine diagnostic passes the repaired fixture, then
aborts in the separate harmonic fixture
`routed_session_owner_imports_held_and_releasing_notes_through_first_paint_and_undo`.
That original diagnostic is retained as negative evidence; the focused follow-up
below resolves the same fixture-residency cause without changing production. Original negative logs,
source/binary hashes and the supported production8MiB seek/export reservations
remain preserved. Coordinator ownership and exact combined qualification must
resolve outstanding blockers before delivery.


### Routed harmonic fixture, same construction cause

The routed Session harmonic import/first-paint/Undo fixture also keeps seven
actual `SessionPlayback` owners inline. On the unchanged harmonic source blob
`b0f3b9da7ace265ac5208c0e96c0c2b9c107689f`, its default-stack debug version
aborts while release passes. Its debug function frame reserves1636376bytes
before calling the unchanged shared constructors. The two other largest observed
fixture frames pass on default stacks, so static frame size alone was not used
to declare additional defects or rewrite their tests.

A helper local to this one test now constructs each identical runtime in a Box.
All seven owners still coexist, and every source project, routing/launch, loop,
held/releasing note proof, first-paint comparison, refused StopTrack, subsequent
PCM comparison and Undo assertion remains. No shared fixture helper, tuning,
resource, production constructor, callback, admission or lifetime contract changes.
The caller frame is now51448bytes; the test-local constructor helper reserves
226504bytes. No stack-size wrapper or environment override is used.

With both fixture corrections, broad default-stack engine library coverage passes
527tests with zero failures and seven existing ignored profiles. Both original
cases also pass in release mode on default test stacks. These scoped results are
separate from repository full-CI qualification and do not establish production
peak stack use or native hardware deadlines. Preserve the failed pre-fix default
receipts and the documented normal cloud `RUST_MIN_STACK=16777216` configuration
as distinct evidence. Final integration with the independently tracked spectrum
factory repair requires its own exact-source checks and coordinated Mac cohort.
