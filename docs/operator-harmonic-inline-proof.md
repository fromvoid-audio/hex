> Historical design/proof record. The production implementation and current
> decisions are documented in [operator harmonics](operator-harmonics.md). Earlier HOLD, unresolved-law and
> uninstalled-runtime statements below describe that checkpoint, not current code.

# Private harmonic inline encoding proof

This is a disconnected, test-only checkpoint on canonical PR158/#2342. It does
not install a pool, register a kernel, publish a codec/version, or change provider
or callback contracts. The original typed-capture overflow fixtures remain.

The candidate keeps two independent histories and every receipt, harmonic and
enable clock, ID, recurrence value, scratch/committed field and metadata value.
Only active/gate-present booleans share one checked byte. The full gate u64 is
separate, including Some(0) and Some(MAX); no numeric value is an absent sentinel.
Unknown flag bits refuse decoding. The exact Operator provider format/version
and zero reserved words4–7 are checked before omitting those words. Four level
words retain their raw bits. Target identity remains separate from the complete
namespace/generation/context owner key. Decoding reconstructs the unchanged
8-word AnalyticContinuation; the real legacy provider still validates owner,
values, clock and PCM. No value is normalized, clamped or quantized here.

On the supported x86_64 layout (8-byte alignment), actual sizes are:

| Class | Frozen typed proposal | Inline candidate |
| --- | ---: | ---: |
| Enable binding | 120 | 88 |
| Voice/history | 120 / 2024 | 112 / 1896 |
| Law | 4056 | 4056 |
| Complete pair | 8312 | 8024 |
| Capture record / slot including metadata | 2440 / 2488 | 2408 / 2456 |
| 1142-slot pool including two 256-handle vectors and header | 2853688 | 2817144 |

Pool charge is 104-byte header + 1142*2456 + 512*24. Actual Vec capacities are
asserted. A 256-pair vector charges its 24-byte header and 256*8024 = 2054168.
There is no side buffer, shared arena, map or owned allocation inside an encoded
pair/record. The copied legacy enable/harmonic history is not aliased or removed.
The compact lease model actually stores compact slots, not full records behind
handles. Its owner header is 128 bytes (pool + root vector). Root capacity*48 and
every real provider's complete declared context bytes are included separately in
owned_bytes; these are not hidden in the 24-byte handles. Static tables remain
unchanged externally shared owners, and are not newly allocated by this proof.

The context proposal remains 2564 bytes, including its payload and Arc header.
Retained recipe charge is 2456+2564 = 5020: 417 charge 2093340; 418 charge 2098360
and refuse the independent 2MiB budget without mutation. The 256 staging map,
slot capacity, context ceiling4096, generation/refcount overflow and 8192 pair
limit remain independent. Retired bytes stay charged until explicit control
reclamation; no expired-owner charge deletion or content-only owner dedup occurs.

Adding the *proposed* full pool to the previously measured EQ playback fixture
30728168 gives 33545312, 9120 below the unchanged33554432 cap. This arithmetic is
not a newly installed runtime pool or universal admission proof. Immutable graphs
are outside that existing preroll-runtime ledger, and contexts have independent
limits. Other valid configurations still require complete actual admission.
The frozen additive pair8312 and EQ33581856/excess27424 remain negative controls.

Proofs compare both independent decoded histories bitwise, including inactive
values, signed zero/NaN sentinel bits and full-width IDs/recurrence. Enable tests
compare the real provider at before-start, interrupted and endpoint frames and
retain the restart-clock counterexample. The existing builtin512-versus64
negative control and 17/25/64 bindings remain. Compact storage repeats meaningful
owner/clock/refusal/PCM, saturation-abort-retry, detached references, namespace
exhaustion, generation/reference overflow, staging rollback and control-retirement
oracles, with its actual417 recipe boundary. This disconnected model does not
prove cross-kind imports, concurrency fences, production allocations, native
performance, or painter behavior. The full local CI allocation probes cover
existing production routes, not an uninstalled harmonic runtime.

Native amplitude, Normalize, Repeat and range laws remain unresolved. No Live
numerical parity, native listening/painting/Undo/reopen, hardware deadline,
optimized macOS packaging or complete #2342 readiness is claimed. HOLD remains:
blocked Pulse/draft PR, no registered gate evidence or readiness mutation.
