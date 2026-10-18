# Track-header volume — Pulse2715 / PR331

Resident Arrangement headers use the shared compact NumericControl rail and
editable dB readout. Every regular hybrid track shares its canonical gain, range
0..2 and existing linear-amplitude/dB scale (zero displays minus infinity).
The minimum header stays176px wide and44px high; resizing, automation, mute,
solo and return-send controls retain their existing ownership.

A held drag uses typed TrackGain gestures, the existing single-in-flight
coalescer and resident prepared-preview publication. Each accepted update clones
and prepares an isolated overlay; this is not a scalar callback parameter.
Preview does not edit canonical history, revision or dirty state. Release commits
one absolute Gain command; Cancel restores latest canonical state, including
independent edits. Existing runtime inheritance keeps sustained playback running.
Static gain refuses automation-owned targets, and recording blocks gesture Begin.
Save/export/close use the existing numeric finalization barriers.

VerifiedStreaming retains its functional explicit +/- controls. Audible held-drag
support for that production mode remains open under Pulse2792 / PR398; there is
no inaudible commit-on-release fallback or new resource-admission permission.

The public actor also refuses TrackGain Begin in VerifiedStreaming before an
active overlay or success acknowledgment. Refused Change/Commit leave canonical
state/history untouched; ordinary explicit Gain commands and Undo still work.
Own four header UI fixtures and the recording-race fixture use the existing
bounded test constructor with retained, owned browser/preferences/theme paths
installed before browser worker startup. Earlier ambient-constructor UI evidence
remains withdrawn for native use; replacement results require their own source.

Headless checks cover rendered minimum/expanded geometry, target isolation,
drag/cancel and editable values, automation and recording fences, actor PCM and
history, post-fader return/balance/mute, persistence, bounded coalescing, and actual
callback runtime handoff with zero total/net allocations. Worker timing is measured
separately from physical input/audio latency. Exact results belong to the retained
source-specific evidence; final integrated full CI belongs to the merger.

## Finite native acceptance (pending)

On the qualified optimized candidate, use a disposable mixed audio/MIDI project.
Drag each track's rail while playback continues: silence→positive, positive→zero,
unity and maximum; only that track changes, including a post-fader return.
Listen for sustained-note continuity, clipping/dropouts and responsive held mixing.
Release once, Undo/Redo once; drag then Escape, including after an independent edit.
Edit a dB value, use Enter/Escape, then save/reopen. Check automation ownership and
recording refusal, mute/solo/return buttons, narrow/wide and short/tall headers,
selection, text focus and both resize handles. VerifiedStreaming must retain +/-;
live fader acceptance there awaits2792. Capture optimized input-to-applied-audio,
terminal/backlog and device timing separately; headless checks do not prove these.
Root coordinates all native work and app delivery; this source change installs nothing.
