# audio clip placement model

Pulse2854 / PR456 extracts the model prerequisite for clip-local looping2843.
The pinned intake main is `2eef0cb1ab096445df38c704ed3a1dfe68e4bdb3`;
the canonical plan commit has the same tree. This source introduces no loop handle,
Engine preparation, waveform association, DSP, export or resource-custody change.

`AudioClipEdit` retains one processing domain and selected `view_start..view_end`.
Optional Copy `AudioViewPlacement` supplies Once/Repeat, positive outer length in
the same frame/beat clock, and consumed cycle phase. Repeat phase is inside the
selected cycle. Once phase can exceed it, yielding no occurrences without
revealing hidden source. Left trim advances start and consumed phase together;
Once retains the full offset and Repeat uses modulo. Splits preserve roots and
source mapping. Explicit Repeat activation normalizes phase; ordinary outer
resize preserves mode and phase. First capture uses the current processed crop,
even with raw underlying source looping; existing capture retains its cycle.

The allocation-free borrowed `AudioViewProjection` exposes checked outer bounds,
signed historical first boundary/root origin, count and indexed occurrences.
Each beat-clock occurrence projects absolute endpoints through the actual tempo
map; it does not repeat a rounded frame period. Frame clocks retain frame
semantics. Negative historical boundaries are metadata, never fabricated at zero.
Empty Once branches before in-root arithmetic. Consumers still must preflight
aggregate work, actual root/PCM ownership and output counts before allocation.
This model is not resource admission or proof that the current Engine renders
placement-aware audio.

`ResizeAudioClipView` accepts only an operation, recomputes current state and uses
the existing audio/MIDI overwrite helpers and private candidate/history. Only
newly occupied growth after the previous outer endpoint is overwritten; unchanged
or shortened bounds preserve existing overlaps and no-op history/IDs. Marker
edits keep occupancy fixed; RepeatOuterEnd activates and resizes atomically.
The scalar preview is allocation-free. `preview_audio_view_resize` may detach
small edit metadata, shares the domain Arc, projects and validates actual asset
bounds. Canonical execution shares the checked materializer but never accepts a
GUI-prepared Clip. No whole-project or PCM copy occurs in these helpers.

The audio-only slice needs no MIDI Once policy, playback changes or new snapshot
fields outside AudioClipEdit. Existing transformed Arrangement-audio to Session
transfer remains an explicit atomic refusal: Session's unwarped one-shot owner
cannot represent this domain. Ordinary audio copy remains supported. No notes,
media or user project files are deleted or converted.

The current project envelope advances from31 to32 and requires explicit placement
in each audio edit, including null. It preserves main's required tuning and
complete device-asset manifests. Format31 refuses explicitly; no migration or
compatibility reader is introduced. Source1's separate A/B slice also targets32;
its DeviceDto/comparison/required-manifest hunks must coexist in the final current
format during ordinary integration. This prerequisite does not import or qualify
that unmerged slice, and does not promise compatibility with unpublished alpha
branch formats.

Checks cover both clocks, fractional tempo, signed/nonzero origins, exhausted
silence, trim/split/extend-back, explicit Repeat and raw-loop capture, overwrite
capacity/refusal/history, checked/stale previews, current-format round trips and
canonical Session binding/refusal. Engine output-range/custody, playback/export,
waveforms, handles and rounded rendering stay on separate coordinated scopes.
Final prospective-tree qualification and merges belong to the sole merger;
native/app/physical acceptance and delivery belong to the coordinator.

The exact-content guarded targeted run `20261019T001731Z-5e0ce3bb` passed284
Project cases,123 Storage cases and5 existing Session conversion cases, with zero
failures/ignored, strict relevant all-target Clippy, architecture, touched-file
formatting and diff checks. This includes19 placement/model cases,4 audio codec
cases and explicit omitted-null refusal. It is partial automated model/codec
evidence, not the required final prospective full21, native or audible-loop
qualification. The dated results prose was added after that run and reviewed as
documentation; it does not alter the tested Rust or test bytes.

## review corrections, 19 october2026

On frozen integration `2918cc49`, three loaded audio/MIDI overlap regressions
reproduced carving inside the previous outer range on unchanged, shortened and
grown bounds. The bounded correction uses the existing overwrite helpers only
for positive growth `[old_end, new_end)`, retaining the original material inside
the already occupied range. No-op Redo/IDs, one Undo/Redo and late preparation
refusal remain atomic. This does not introduce the future global no-overlap rule.

The existing Arrangement Looping control now separates retained-source looping
from optional placement mode. With placement, the control's on/off, glyph,
tooltip and inverse command use Once/Repeat. Without it, ordinary source-loop
behavior is unchanged. PlayStart's source-phase-versus-trim branch and the source
range description retain the raw flag. Freshness compares both flags, identity
and revision; no Project `Clip::looping()` meaning changed. The placed tooltip
does not claim that disabling repetition resets the retained source phase.

Two actual visible-control baseline cases dispatched the wrong inverse for
Once/raw-on and Repeat/raw-off. Both now pass and their actual UI commands apply
through canonical model history without changing the raw source flag/phase.
A controlled same-revision mode-only update invalidates captured values, and a
newly rendered control accepts the next click. This is not an assertion that GPUI
keeps stale listeners after redraw. The source-phase discriminator and all three
ordinary control tests also pass: seven UI cases, zero failed/ignored. These
injected loaded-view tests prove rendering/dispatch and canonical model edits,
not real worker acknowledgement or audible playback.

Separately, 22 audio-model, four current32 codec, five Session binding and six
actual return-workflow cases pass: 37 unique cases, zero failed/ignored. The
return fixture's one stale current-version expectation is corrected from31 to32;
its PCM, routing, devices, history and saved-document assertions are retained.
Strict touched-component checks and immutable exact-source evidence are retained
in Pulse2854. Original failure, invalid offscreen fixture and test-compilation
logs remain separate from passing receipts. Final prospective full21 and merge
belong to the sole merger. No new native run, app operation or full445 audible
loop/handle/export acceptance is implied by this prerequisite repair.

## default-test and signed-projection codec return

The placement control's three rendered GPUI cases and their three private
helpers now use the same `ui-test-support` feature boundary as their existing
window fixture and GPUI test context. All four pure control-mapping tests remain
in the default test configuration. This fixes a default-library test compilation
failure; it does not skip a runtime failure or disable the pure mapping cases.

A bounded retained root with a legal, fully consumed Once cursor can produce a
signed fade-projection origin below `-u64::MAX` during the actual musical-clock
conversion. The model admits this state, but the decoder previously rejected its
own encoded document. The decoder now matches the model's inclusive
`-2*u64::MAX..=u64::MAX` bound. The original full-domain frame limit, source,
placement, tempo, fade and overflow validation remain unchanged. Tests retain a
480k-frame root, use the canonical conversion, compare complete reopened
snapshots and cover both exact signed endpoints plus malformed/outside values.
An initial oversized-root fixture was correctly refused by the model before
serialization; that setup failure is preserved separately from the reproduced
codec failure.

The change retains current format32, required placement/tuning/asset fields and
explicit old-format refusal. It introduces no migration or compatibility reader.
A/B and both-slot asset manifests still require integration with their actual
current-main implementation; this isolated return does not import or qualify
another owner's branch. The original Engine, waveform, playback, native and
full445 acceptance gaps remain open. Exact prospective qualification belongs to
the merger.
