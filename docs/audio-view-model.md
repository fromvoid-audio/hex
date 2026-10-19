# audio clip placement model

Pulse2854 / PR456 extracts the model prerequisite for clip-local looping2843.
The pinned intake main is `2eef0cb1ab096445df38c704ed3a1dfe68e4bdb3`;
the canonical plan commit has the same tree. This source introduces no loop UI,
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
the existing audio/MIDI overwrite helpers and private candidate/history. Marker
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
