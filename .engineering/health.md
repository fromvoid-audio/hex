# engineering health

Snapshot: 5 November 2026; Linux x86_64, Rust/Cargo 1.94.0. Measurement source:
`4b7309c4fd930ef4c51eadb0cb7408fd5c2b5ac0`, a tree-equivalent Pulse linkage commit
over upstream `7712eb87a2ef900dc4be8c98a3c8c26db5c09280`.
The concurrent workspace additions are prose/JSON only; executable inputs were
unchanged. See [measurement records](measurements.jsonl) for exact commands/logs.

| check | result | wall time |
| --- | --- | --- |
| architecture boundaries | passed, 30 explicit crate owners | 0.098 s |
| pinned dependency patch guard | passed | 0.070 s |
| UI style guard | passed | 1.384 s |
| device composition boundary | passed | 0.472 s |
| existing Rust-shard owner regressions | passed, 2 tests including real Cargo/libtest | 2.799 s |
| headless `hex-diagnostics` check, locked/offline | passed | 1.498 s |

The configured native Linux libraries and pinned Rust toolchain are available.
These focused checks do not establish a full-workspace green build, comprehensive
tests, application performance, macOS GPUI/CoreAudio behavior, physical deadlines
or listening acceptance. No application performance benchmark was run: the first
selected improvement is verification reliability, not speed.

First cycle: [empty-shard refusal](https://pulse.iocore.dev/#/issues/4990) is
implemented and verified. Before the fix, empty inventory, a filter typo and
skipping every case each returned 0. They now return 1 with an explicit diagnostic.
Real Cargo/libtest checks preserve mixed empty/nonempty targets and ordinary test
failure propagation. An explicit runner-only allowance preserves existing empty
feature configurations; an aggregate regression still rejects all-empty coverage.

At `e7cbe9ca107db5c4e945a7ee64ecd9b33085ef13`, all 172 owning CI/planner/selector
tests passed in 19.690 s, and all seven quick guards/format checks passed in
22.272 s. Raw logs and commands are in the measurement records and canonical PR
#2077. These are reviewed boundary checks, not a full-workspace/native certificate.
The diagnostic before/after timings do not establish a performance improvement.

The first broad run failed in two existing self-test cases. The temporary metadata
Git fixture now ignores generated Python bytecode, preserving its intended source
mutation checks. The isolated browser scrollbar guard fixture was missing from the
reviewed inventory; [its focused repair](https://pulse.iocore.dev/#/issues/4993)
merged in PR #2078 after 31 tests and quick checks passed. The original failed log
is retained as `artifacts/engineering-bootstrap/owner-tests.log`; no failure was
counted as passing. The final broad run uses ordinary bytecode behavior.

Git fetch works. GitHub CLI API access returned `Forbidden`; connected GitHub and
Pulse tools successfully read the repository and created canonical issue/PR links.
Use those tools rather than changing credentials or network policy.

The user supplied the complete `/goal` instruction and the agent is following it.
Native goal readback is unavailable in this cloud executor: its app-server control
socket is absent. This is not a verified native activation claim. No scheduler was
created; ongoing execution requires an active authorized runtime.


Second cycle: [document-output test ownership](https://pulse.iocore.dev/#/issues/4997)
is verified at `1f860e4d8a1efbe14b00006e135cc5c01d6f0947`, with current-base
integration qualification recorded in PR #2081. A `document_output.rs` change
previously selected 200 registered native-audio tests; it now selects the two
owning regressions. All 228 compiled test names remain exactly once after the two
namespace substitutions. Harness, fixture and assertion bodies are byte-identical;
the shared native lease remains in use. This reduces test-ownership coupling,
without an application-speed claim or weaker regression oracles.

The two cases passed before the move in 0.1721/0.0617 s and after in
0.1066/0.1064 s, including launch. Two focused fixture/deadline consumers passed in
0.0367/0.0068 s. These small timing differences are not treated as a speed gain.
Both strict desktop Clippy configurations, six source guards and 30 shared selector
tests passed. The cold baseline build took 13m31s, separate from test time.
Raw commands, inventories, per-case logs and binary hashes are retained under
`artifacts/engineering-cycle-4997/`; the canonical PR retains essential results.
