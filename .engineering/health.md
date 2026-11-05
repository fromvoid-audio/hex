# engineering health

Snapshot: 5 November 2026, Linux x86_64, pinned Rust/Cargo 1.94.0.
The runtime has resumed the supplied goal automatically; it is active. No scheduled
task was created. A runtime stop still requires checkpoint recovery.

Latest verified cycle: [priority Stop after compensated source refusal](https://pulse.iocore.dev/#/issues/5009).
The existing actual-callback regression now issues priority Stop after the rejected
candidate is reclaimed, compares PCM with an independently stopped oracle across
1/7/64/512-frame blocks, and reaches silence beyond the future onset/release.
Each callback retains zero total and signed-net allocation/count/byte checks.
The existing fixture and runtime implementation are reused.

At source `562e452e40f632b3b384dc669290699d17ea3c14` over `69d66f11fb7a710f5dc4be89a324cff587238751`:

- Six static/live receiver regressions passed in 4.09 s (build time separate);
  current-base revalidation passed in 4.08 s.
- Both allocation-counter sensitivity calibration tests passed.
- Strict owner all-target Clippy passed in default and allocation/stream-support configurations.
- Final guard/format and metadata verification are recorded with the canonical PR #2085.

A provisional runtime refusal was superseded by upstream PRs #1974/#2080 and is
not in the retained diff. Those changes preserve the static send route and refuse
unproved compensation; their source/native acceptance remains in the owning Pulse
issues. Commit `020108332` and raw logs retain the superseded experiment honestly.

Prior verified outcomes and exact commands/source/base/log references remain in
[measurement records](measurements.jsonl) and canonical PRs: empty shard refusal,
reviewed Python inventory, and 200-to-2 document-output test selection with the
228-test compiled inventory preserved. Old health snapshots remain in Git history.

The [private-project CPU report](https://pulse.iocore.dev/#/issues/4965) still needs
its matched optimized Mac/project baseline. Linux/headless checks establish neither
macOS UI/listening acceptance nor physical deadlines or application speed.
Ignored `artifacts/engineering-cycle-5009/` retains local raw evidence; canonical
PR/Pulse records retain essential results if executor-local logs expire.

Final integrated evidence at `2decefbeb08cb15422f363addb9eece2510e643b` over `eddf7202823f1de3574899ac699adad942ba5481` repeats the receiver/calibration and both strict owner configurations successfully; all seven quick guard/format steps pass.
