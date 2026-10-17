# native project tuning runtime

Pulse2740 / PR351 continues the canonical tuning prerequisite2739 / PR350.
The complete editor, independent reference-tone audition and MTS broker remain
goal4 requirements; native tuning does not depend on MTS availability.

## preparation and pitch laws

One validated immutable128-key table is prepared on control. Disabled tuning
uses the exact existing A440 formula. The effective table is compared by f64
bits and participates in shared-context/template/bank identity; audition settings
do not change pitched instrument cache identity. Tables are not copied into every
clip or authored device state. Sine, FM4 and Operator prepare their pitched voices
from this table in Arrangement, Session, live keyboard and offline export.
Unsupported providers explicitly refuse nonneutral preparation.

MIDI key, channel, velocity and occurrence identity remain authored facts. Ratio
oscillators use the mapped frequency; fixed-Hz Operator oscillators retain their
absolute frequency and multiplier. Existing pitch envelopes, transpose, glide,
key tracking and expression laws operate on the prepared voice; project tuning
does not rewrite their authored parameters. Actual voiced pitches must satisfy
the instrument's sample-rate/Nyquist rules. Unused unsupported keys do not cause
blanket rejection of a valid scale.

## continuation and refusal

Sine/FM4 inline notes carry a static provider authority. Providers validate
nonpitch coefficients and compensate each carried occurrence at its actual age:
`offset_new = offset_old + (step_old - step_new) * age`, with wrapping phase
arithmetic. Interned banks remain immutable. Repeated and skipped revisions start
from the applied occurrence's sampler. Shared Operator notes use the existing
provider-authorized mutable continuation and preflight both runtime twins.
Held/released voice phase, envelope, feedback/filter state and age are preserved
for supported transitions. Detached tails keep their originating law.

The selected keyboard's family identity inherits only after checking canonical
same-except-tuning state and the authentic previous prepared source. Fresh Open
and ordinary edits start a fresh family; a hint is scoped to the exact physical
input lease. The worker invalidates ordinary-edit inheritance during preparation,
but cancellation of the audible keyboard lease occurs at audio publication.
A pending/refused candidate must not prematurely silence the installed graph.

Control preparation failure leaves canonical state/history unchanged. A later
callback refusal based on actual held voices leaves the applied runtime/PCM
unchanged while canonical desired state/history remains committed. Stop/release
and a new eligible publication provide recovery. Desired project, native-applied
and MTS-published revisions remain separate; there is no sample-atomic external
publication promise.

## bounded ownership and footprint

No callback constructs a table, allocates, locks, performs I/O or destroys a
graph's final owner. Per-occurrence continuation uses constructor-owned fixed
storage and ordinary control retirement. On64-bit builds:

- Inline sampler352 bytes, including its8-byte static authority; bank entry360.
- One shared table header1112 bytes: old80 + table1024 + scalar identity8.
- Complete maximum combined note-bank/index inventory48,140,288 bytes/revision;
  two revisions96,280,576. This is a derived metadata bound, not process RSS.
- Release entry488 bytes;192-span buffer84,488 bytes. Paired release entries plus
  span buffer16,075,272 bytes; with context owner/lease slots16,599,560. Existing
 16/18/32 MiB scoped/preroll ceilings and overflow refusals remain unchanged.
- Neutral shared contexts keep their existing compact configuration, so the
  unchanged2 MiB retained harmonic-context ceiling still admits426 proposed
  contexts. Only custom-tuned wide mutable owners retain a fixed inline table;
  their actual factory/runtime size includes it. Legacy feedback/LFO twins keep
  their existing8KiB bound. No configuration gains heap owners or callback clones
  of a reference-counted/owned table.

The keyboard reserves two64-voice inline override arrays in its one selected-track
runtime, rather than per project track. The explicit release timing test reports
actual runtime/override bytes and same-tuning versus changed-tuning handoff cost
for17 held keys. Timing excludes construction, rendering and donor destruction;
it does not establish physical audio deadlines.

The first cloud release probe (128 handoffs per mode,17 held keys,48 kHz,
137-frame renders between handoffs) reported same-tuning p50/p95/p99/max
13.565/21.706/59.395/74.034 microseconds and alternating tuning
17.749/23.632/1136.897/1226.521 microseconds. Runtime size54,576 bytes includes
45,056 bytes for paired inline overrides. This is one uncontrolled shared-host
sample with large outliers, not a speedup or sustained deadline certificate.
Retained raw output names the command and fixture; final candidate validation
remains separate from this development measurement.

## evidence and remaining checks

Cloud continuation reproduced and repaired the Operator fixture's nonexistent
`preflight` call, stale footprint assertions and premature worker-side keyboard
cancellation. Focused regressions include rapid/Undo/skip phase continuation,
released/fixed-Hz voice retention, canonical refusal/history, non-octave degree
maps with root offset, Arrangement/Session agreement, independent tuned Operator
PCM16 export and real callback retuning/retirement with zero total and signed-net
allocation counts/bytes. Logs remain under `artifacts/tuning/cloud5/` once the
candidate checkpoint is retained. Failed development runs remain evidence too.

An additional combined all-target feature matrix reproduces known unrelated
Pulse2722 / PR338 (playback_stack fixture omits Expression). Its failed output
is retained; the repository's required scoped matrix is unchanged. Full final
exact-source local CI and final diff review remain required before merge.

Mac follow-ups use the coordinator's single serial local thread: exact-source
native checks and optimized combined delivery, user interaction/listening,
CoreAudio/deadline evidence and installed MTS-compatible client/signing validation.
Linux tests establish none of those. No Mac work is launched by this cloud lane.

Exact mutable Arrangement/Session topology matching includes the existing
shared tuning identity. Identical authored instrument bytes with a changed table
must use the prepared history-retarget path rather than incompatible exact-copy
history. A public Operator keyboard playback regression and real FM4/Operator
callback held/released-retune allocation fixture cover this distinction; the
callback fixture failed on the old exact-match path before the repair.
