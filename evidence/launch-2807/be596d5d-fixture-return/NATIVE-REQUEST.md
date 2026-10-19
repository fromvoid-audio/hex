# Fixture return — NEW exact source, no production delta

This request supersedes the uncompleted a1a4 request only. Original a1a4 Mac first22 was20PASS1ERROR1FAIL; later20+25 and Swift smoke NOT RUN. Original report977cb6e8b59ec67c8704ec5e06d70f8c2e6e92a93366bb71762311390b8ad798 / manifestedd57e53568f767e103bbcb4acceae56063ef3dafef75f0694b7a102d39fa6f5 remain negative evidence. No a1a4 compiled-native green claim.

Only TWO test methods changed. Positive fixture compile30s / operation5s capture actual wait arguments [30,5,5], one compiler/two calls. Negative fixture waits up to15s ONLY for its own invocation JSONL before starting a real .2s process wait; helper sleeps60s. It asserts timeout, owned child reaped, one compile, no retry or leftover workspace. Production still120/15 INCLUDING startup. Fixture synchronization is not a production readiness exemption.

All three suites must run at THIS new exact source (67 unique /0skip/all3exit0). Actual Swift smoke remains unexecuted and requested after them; no AppKit fixture dispatch. Smoke script differs from old packet ONLY in expected head/tree constants.

# Root serial Mac request — new production compiled helper

Canonical413 head be596d5dbbdc05a3ce8aedbc0f05d90e4920a647,
tree338a8e6c671de3f15af348b28d898ab908ef0457; parenta1a4391e.
Exact Swift source SHA5e1cf00760ca6d73f91f0131ce86cf61012a2db46dccc35c99180c69f41d79d0,
UNCHANGED from the timed-out interpreted source and successful diagnostic compile.

This packet is a REQUEST, not native execution evidence. Preserve old parent Mac45,
interpreted snapshot15.027770s timeout, and diagnostic27.1944s compile/.2639s
snapshot as separate historical evidence. A successful diagnostic binary was NOT
substituted into production: this source changes the actual lazy constructor/run
path and needs qualification at THIS new head.

Root's normal exact-source/protected-app/output/lock preflight first. One serial
lane/private target and temp paths. No native helper signals/operations against
actual Hex, no launch --force-restart invocation, no build/install/launch/open,
permission/signing changes, user-media/state operations, cache warmup or old-source
retry. Fail fast and seal original raw failure; no source edit, timeout widening,
zero-test green, unsupported skip or silent command replacement.

## A — actual systemPython targeted regression suites

Run serially from the exact source checkout:

```
/usr/bin/python3 -m unittest discover -s scripts/tests -p test_launch_restart_compile.py -v
/usr/bin/python3 -m unittest discover -s scripts/tests -p test_launch_restart.py -v
/usr/bin/python3 -m unittest discover -s scripts/tests -p test_launch.py -v
```

Expected22 compiled-helper +20 +25 =67 unique tests,0skips/failures,3exit0. These use ONLY
fixture-created compiler/helper/dummy children and disposable state. Existing
PreflightTests still resolve only their newly created owned repo before deriving
strict app paths; no global TMPDIR/guard workaround. Native helper tests similarly
resolve only their created fixture root. Source1's original45 Mac pass remains
historical; this67 must execute against the new production source.

The new suite includes separate cold compile/runtime budgets; one actual fixture
compiler+same helper for snapshot/quit/force; lazy behavior/default120/15 provenance;
source/copy/compiler/binary identity, byte/inode/link/directory substitutions;
refusal/failure/no retry; owned compiler+descendant timeout/interrupt with unrelated
child survival; native timeout; and retained workspace when child exit cannot be
confirmed. Python3.9 grammar was checked in cloud, but actual systemPython execution
here is required. No cloud actual Swift/AppKit execution is claimed.

## B — actual production compile followed by THREE absent-path native probes

After A passes, use the packet's `compiled-absent-smoke.py` WITHOUT source edits:

```
/usr/bin/python3 /OWNED_PACKET/compiled-absent-smoke.py /EXACT_SOURCE /OWNED_NEW_REPORT_PARENT
```

Replace only the three owned paths. Script source is in PAYLOADS.json. It refuses
non-Darwin, wrong/dirty source/tree or wrong Swift SHA. It creates one exclusively
owned0700 fixture and NEVER creates the `hex.app` directory. It imports the actual
production NativeController and calls snapshot(), causing ONE source-bound fresh
Swift compilation with separate120s compile budget in its own private workspace.
It verifies actual provenance, unchanged15s operations and same executable hash/
inode. No precompile/warm cache from the old diagnostic may be adopted.

Expected native results (NOT three more unittest counts):

1. compiled production snapshot: exit0 /exact[];
2. SAME binary, `quit` on absent private app/invented identity: exit1 /identity
   changed refusal;
3. SAME binary, `force` on that absent path/identity: exit1 /same refusal.

Important scope: NativeController.call correctly refuses termination when no
installed seal exists. Cases2–3 directly probe the SAME compiled Swift helper's
absent-target protocol through its bounded child runner. They do NOT bypass the
production seal check for any real app, do not target actual PID2147483647, and
are NOT running-app normal-quit/force evidence. Swift enumerates the private
absent bundle path and refuses before any native termination call. The raw argv,
stdout/stderr/exit and source+binary identity remain in the report.

Require exactly ONE successful compilation, same binary all operations, no
private app appears, and own helper/module workspace removed after confirmed
child exits. Retain phase logs, native-compile.json, native-helper.json and
native-probe.json; source/temp identity and compiler/binary hashes, full command,
actual wall-clock/budgets, protected app/output preservation and final child/
lock state belong in root's sealed report. If compile/toolchain/runtime/cleanup
fails, seal its exact failure instead of warming/substituting/rerunning.

## Still UNEXECUTED / separate review

Actual owned AppKit fixture cases1–4 from earlier413 design are NOT dispatched by
this request: real running-app graceful quit, cancellation→force, identity change
and independent process survival need their separate root-reviewed assignment.
Do not substitute actual user Hex for a fixture. No open-app/physical restart,
installation, full localCI/merge/delivery or user acceptance follows from A/B.
Sole merger owns prospective full localCI/source review/merge; Pulse remains
InProgress until actual merge, and ReadyForReview only then under user policy.
