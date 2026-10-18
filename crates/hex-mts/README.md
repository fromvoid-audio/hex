# hex-mts

## Purpose

One desktop-owned serialized MTS-ESP publisher, with explicit project ownership,
bounded coalesced desired publications, independent published identities and
reserved disable/project-switch/shutdown control. All native calls and owned
registration cleanup stay on its worker thread. Retrying unavailable/conflicting
masters is explicit. Native tuning and offline rendering do not depend on it.

## Dependencies

`hex-tuning` supplies validated control values. The separately audited, pinned
`mts-esp-master-adapter` owns the excluded native ABI boundary; see
[boundary](../../docs/mts-esp-boundary.md). Hex workspace unsafe remains forbidden.
The broker uses the standard library for its short control mailbox and worker.

## Does not own

Canonical project edits/history, audio callbacks, DSP, native output, UI focus,
plugin hosting, privileged library installation or signing policy. A compatible
installed MTS runtime/client and actual signed Mac qualification remain external
requirements. No sample-atomic native/external publication promise is made.
