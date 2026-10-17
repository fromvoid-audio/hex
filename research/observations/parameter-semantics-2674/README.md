# parameter snapshot validation profile

17 October 2026; original Hex control-side experiment, not a Live reference study.
Exact source, tool, workload and measurement boundaries are in
[measurement.json](measurement.json); raw aggregates are
[before](operator-release-before.csv) and [after](operator-release-after.csv).
The benchmark source is `crates/hex-devices/examples/profile_parameter_snapshots.rs`.
The external measurement crate copied that file byte-for-byte and depended on
the exact clean verification checkout; its small separate target avoided native
GPUI cache interference. Both runs used the same release settings and machine.

The production Operator registry applies a 64-entry harmonic snapshot to the
default authored state. Four warm-ups precede 16 measured patches. Independent
readback checks every requested value after each patch. Compilation, source
construction, readback and caller destruction are outside the timer. Internal
registry preparation and allocation are included. With 16 samples, nearest-rank
p95 is the maximum; these are descriptive aggregates, not a stable latency bound.

Before: median 11.089208 ms, p95/max 12.526333 ms. After: median 4.004041 ms,
p95/max 4.777375 ms. Uncontrolled system activity and sequential measurements
limit causal/timing claims. This supports a narrow preparation-cost improvement,
not whole UI acceleration, callback safety, native deadlines or a proven cause
of the full-suite timeout.

Deterministic regression evidence is stronger: a 64-entry default atomic patch
must enumerate no more than 65 complete descriptor sets (one batch preflight plus
one fresh candidate per entry); the previous implementation enumerated 129.
Additional fixtures remove a later parameter, narrow its domain, or make it
read-only after the first private patch. All must reject before a second private
mutation and preserve the caller's source state. Both new cases failed before
the repair. Fifteen contract and two registry cases pass afterward.

Full local CI at `75f6d2cf4db87050e8b477a696ca2bd81cfc5244` failed after
1363.506 s: UI step had 621 passed, one failed and three existing ignored tests.
Its receipt remains `artifacts/local-ci/20261017T110225Z-65adcf95/summary.json`
in the stable verification checkout, SHA256
`0059be742ec1a7bfd06faf158dd0cc84b4ad3f92429ae6543c3ce3ff506faf8f`.
Source digest was `9cae3bcbc100c952e7bf2087994a976ac79dcf27242e5a06ed99edb0a21c4875`,
clean and unchanged. The harmonic test timed out waiting for a worker action;
its existing timeout was not extended. That same case passed focused at the old
source (56.28 s) and repaired `bcd170ab` (115.02 s), so whole-case speedup is
not established. Raw red/green/UI/build/Clippy/report logs remain under
`artifacts/parameter-semantics/{75f6d2cf,bcd170ab,1f45886e}/` there.

Independent source review accepts the repair and benchmark. Fresh full local CI
on the final candidate remains necessary before readiness or merge.
