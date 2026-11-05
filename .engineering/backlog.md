# ranked engineering index

Snapshot: 5 November 2026. Live linked Pulse issues own scope, priority, dependencies
and status. Refresh this short index each cycle; do not copy the full project backlog.

| rank | opportunity | evidence and benefit | effort / risk | next action |
| --- | --- | --- | --- | --- |
| 1 | [reject empty Rust shard selections](https://pulse.iocore.dev/#/issues/4990) | `scripts/test_shards.py` has no empty-jobs refusal; a typo can report success without executing tests. Prevent false verification. | small / low; tooling only | reproduce with real Cargo/libtest, add minimal refusal, verify mixed empty/nonempty targets and failure propagation |
| 2 | [merge test budget](https://pulse.iocore.dev/#/issues/4001) | existing measured over-budget tooling/desktop probes; reuse the recorded baseline and prerequisite rather than starting a duplicate audit | medium / test-coverage risk | reread its live blocker and prerequisite; select a bounded independent owner only if ready |
| 3 | [classify UI test tiers](https://pulse.iocore.dev/#/issues/3828) | existing large UI suite cost; preserve boundary coverage while reducing repeated setup | large / moderate; defer until smaller valuable work is exhausted | reread current test inventory and existing evidence before proposing a slice |

Deduplication: the existing Repository Housekeeping & Engineering Foundation goal
is reused. Searches of current engineering, autonomous, health and shard work found
no existing bootstrap or empty-shard refusal issue. Existing paused arranger and
streaming programs retain their holds; this mission does not silently restart them.
Completed, disproved, superseded or blocked leads should not grow into speculative
local tasks. Retire their index row with a brief reason in the owning Pulse note.
