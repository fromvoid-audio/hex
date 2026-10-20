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

Automation-owned headers show `auto` instead of the unrelated static gain/dB
value. Both authored envelopes and recorded touch spans retain their existing
edit refusal; the readout has no gain drag binding. Bounded rendered Service
fixtures verify both indications and unchanged Project/history after pointer drag.

## reference-composed mixer fields — Pulse2835 / PR439

The supported resident header component places a compact name/automation/send
column beside two mixer rows. The upper row contains a numbered track activator,
Solo and a clearly disabled recording-availability indication. The lower row has
one centered, filled dB field and one bipolar balance field. Both use the shared
`NumericControl::FilledSlider` appearance: click to type, drag horizontally,
Shift for fine motion, Enter to validate and Escape to cancel. The fill is a
parameter position, never an audio meter; balance fills outward from a marked
center and retains Hex's existing stereo-balance law and l/r/c input codec.
Standard and CompactSlider geometry elsewhere stays unchanged. The 176×44px
minimum header remains usable; width/height resizing and native font shaping
have separate finite checks, rather than an imported PR431 sizing implementation.

Track pan and Return gain use additive typed targets in the existing worker
numeric-gesture protocol. Their overlays prepare through the same finite mixer
command path as track gain, without canonical history edits until commit. The
existing generation/token/sequence, cancellation, rebase and terminal handling
remain shared. Canonical Balance and ReturnGain commands enforce their existing
ranges; one completed gesture creates one Undo step. Automation-owned track gain
still reads `auto`, and gain-recording/edit barriers remain effective. Return
rows reuse the filled gain style and actual mute command without changing pinned
placement or importing the separate PR438 row-unification work. VerifiedStreaming
retains the original explicit +/- gain controls for both kinds of track; held
mixer targets refuse that mode rather than acknowledge an inaudible preview.

This is a supported-controls component, not full Pulse2835 acceptance. Recording
arm/input capture is absent and remains deferred to Pulse26 / version0.2; the
circle has no arm state or enabled action. Return buses currently have no pan or
Solo model/commands, so no fake controls are shown. No recording, routing, schema,
DSP or native input feature is added by this presentation change. The user rejected
PR331's earlier appearance; its source/history evidence is preserved and does not
count as acceptance of this visual refinement.

The source tests retain a real pre-change rendered-layout failure (separate
86px rail/root and46px value field), completed GPUI scene bounds/paint evidence,
actual mouse/text input and real Service PCM/history/current-save round trips.
Linux's deterministic GPUI test platform has no GPU framebuffer and uses no-op
text shaping; scene evidence is not a native screenshot or a Live parity claim.
The reference-composition case opts into bundled Ioskeley native shaping on macOS.

Root's later optimized native checklist: compare the supplied crop against the
actual two-row header at minimum/wide widths and short/tall rows; inspect centered
dB/l-r-c text, bipolar center, fill contrast, activator/Solo state and disabled
recording tooltip. Drag and type normal-track gain/pan and Return gain while
listening, release/Undo/Redo, then Cancel; save/reopen. Check name selection,
automation/send buttons, resize edges, invalid/IME text and shortcut isolation.
Verify Return rows remain pinned and streaming retains real +/- controls. Record
actual source/build and font/visual/audio observations separately; no app action
or installation is performed by the source executor.
