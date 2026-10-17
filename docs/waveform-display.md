# detailed Arrangement waveform display

## cold clip-cache geometry identity — #2651 / PR267

The first clip-child key was captured during row construction, before the live
lane prepaint callback measured its bounds. The child then built trim/drag
geometry from the updated lane while retaining the earlier key. The following
selection-only draw compared that old key against the same geometry already
painted and rebuilt the clip scene unnecessarily.

The child now records the exact lane bounds consumed immediately before building
its content, after parent/snapshot/track validation. This does not advance its
epoch or schedule a frame. Actual geometry changes retain normal invalidation.
The strengthened selection regression performs one cold draw, immediate GPUI
in-process selection events, and one draw per update without executor parking or
extra settling. Removing the assignment reproduces two child builds instead of
one. The fixed case also checks current selection paint, cold clip/trim bounds
and capture through the reused trim handle. All 522 enabled UI tests, strict
desktop/UI Clippy and architecture/GPUI/style/format guards pass.

The optimized merged `5607b14c` native run completed all 360 samples without
failures, but 100-track cut selection-first-paint p95 was 28.229 ms (median
5.649 ms), missing the 16.67 ms target. Six measured selection samples exceeded
the budget; all were at 100 tracks. Their cause is not established by wall-time
samples. Raw output is retained in
`artifacts/issue2648/smooth-5607b14c-capture1/`. This redundant-invalidation fix
does not claim to explain every tail.
The fixed optimized candidate `17ce787d` passed all 20 local CI steps and the
unchanged 360-row native protocol: all four 100-track selection p95 values are
6.208–9.466 ms, with a 9.847 ms maximum across all 160 measured selections.
Medians are 4.723–4.937 ms. No measured selection exceeds 16.67 ms in this run.
Fixtures, six metrics, 5 warm-ups/40 measured iterations and 7–17 ms phase delays
remain unchanged. [Full raw data, intermediate runs and source identity](evidence/time-selection-2026-10-17/report.md)
retain the failed merged-source run and document PR263's intended endpoint
correction. These are native paint bounds, not physical display/audio deadlines.

## dense selection feedback cache experiment — #2648 / PR264

The waveform LRU now retains 512 entries while keeping the existing 65,536
aggregate device-column bound. A 350-visible-clip working-set regression fails
with the previous 128-entry limit and passes with 512. Bounded native diagnostics
confirmed 350 misses/evictions per selection frame before this change and 350
hits with no preparation afterward. Those diagnostic elapsed spans include
scheduling and overlap; they are not measurements of thread CPU time.

An editor-owned map retains at most `hex_project::MAX_TRACKS` GPUI child views
containing the original audio/MIDI clip elements, shapes, clipping, trim handles,
drag listeners and tooltips. No decorative geometry realignment or canvas
batching is retained. Project generation changes clear all children. On each new
immutable snapshot, one bounded reconciliation prunes removed track IDs and
updates every retained child to the current Arc, releasing old offscreen snapshot
ownership while preserving same-project input identity. Pointer-only updates do
not traverse project tracks.

Each child's key includes track/index, revision, viewport, musical grid/snap,
display scale, palette, selected clip and measured lane geometry/drag availability.
Time ranges and track endpoint focus stay outside it. Changed keys advance an
opaque GPUI cache epoch, rebuilding in the current draw with the same child and
global element identity. Busy edits, modals and transient drags rebuild the legacy
child content each draw; the first idle draw rebuilds once before reuse. There
is no notification queued during prepaint. Replacing a child or temporarily
removing its view ancestry was rejected after rendered tests proved it could
lose pending mouse-down capture across otherwise-current state refreshes.

Headers, lane geometry measurement, selection shading/cursors and transient
previews remain live. Main/track selection updates notify the editor instead of
globally refreshing the window; other drag/hover refresh behavior is preserved.
A pointer-down alone keeps the child identity and listeners stable until drag
capture. The read-only same-revision snapshot refresh regression fails with the
identity-replacing cache, passes with the original direct builder, and passes
with the epoch implementation. Snapshot history, palette/grid/resize,
busy-to-idle, pending capture and genuine selection-scene reuse are covered.

The test-support-only GPUI debug bounds map now replays exact paint-history
segments for cached scenes. Nested selector reuse, duplicate overwrite order,
movement and removal pass without disabling caching. The real audio/MIDI trim,
clip drag and worker Undo/Redo regressions run with this cache enabled. See
[native boundary decisions](native-input.md) for the epoch and metadata patches.

The first eight-case native child-cache diagnostic at `49e7109` still missed
the target (100-track selection first paint 18.5–29.2 ms). The merged-source
360-sample intermediate miss and passing fixed-source capture are recorded
above. Source regressions and native paint measurements remain separate evidence.

The optimized headless fixed-fixture run at `cfc07ca` measured zero clip rebuilds
but explicit draw median 18.353 ms/p95 19.123 ms; cached clip paint median
12.813 ms/p95 13.422 ms dominated it. This is the limited test-framework profile
described below, not native input latency. The replay implementation replaces
repeated per-primitive bounds-tree insertions with one validated ordered interval for a
cached scene range. Internal order differences/ties, layer operations, typed
payloads and fresh path IDs are preserved; the unchanged old loop handles
refused contexts. No whole-row paint-layer shortcut is used.

The private deterministic oracle compares actual old/new batch painter sequences
in every overlap cell, including translucent mixed kinds, nested layers, fresh
overlays, multiple cached ranges and eight repeated generations with stable
order spans. Refused invalid geometry/layer/range/overflow cases leave state
unchanged. All 510 enabled UI tests pass with the cache and replay path active.
Native image-buffer Surface rendering has no new fixture; its shared append
branch is unchanged. The same oracle also passed in the optimized test binary.
See [the scene replay boundary](native-input.md).

The optimized headless comparison at `80330bc` retained zero clip rebuilds over
40 measured draws. Draw median/p95/max fell from 18.353/19.123/43.264 ms to
5.426/5.799/5.996 ms. Nested cached-clip paint wall median/p95 fell from
12.813/13.422 ms to 0.858/1.065 ms. These elapsed spans include scheduling and
are not thread CPU measurements; the nested spans overlap the whole draw.
Local receipts and raw output are retained under
`artifacts/issue2648/headless-bulk-replay-recovery/`, with the earlier run under
`artifacts/issue2648/headless-replay/`. This establishes a framework replay
speedup under the limits below, not the native 16.67 ms selection-feedback target.
Subsequent full local CI and native results are recorded with the candidate;
the passing fixed-source native rerun is recorded above.

The ignored `headless_time_selection_framework_profile` test opens the existing
100-track fixture through the normal Session worker with isolated settings and
no audio. It drives 5 warm-up and 40 measured steady selection updates in a
1240×760, 2× GPUI test window. Only explicit `Window::draw` wall time is measured;
input dispatch, setup, worker loading, arena clearing and reporting are outside
that interval. Each draw must paint the new range and reuse warmed clip children.
Run with an absolute `HEX_TIME_SELECTION_FIXTURE_DIR`, `--release`, UI test support
and `--ignored --nocapture`; debug requires the explicit smoke-only flag.
The test backend uses `NoopTextSystem`, test selector bookkeeping and no native
font shaping/raster, GPU submission/presentation, physical input/display or audio.
Its framework timings diagnose cache/replay work; they are not comparable native
feedback measurements or proof of smoothness. The canonical native harness and
its six metrics, fixtures and 360-sample protocol remain unchanged. Temporary
production diagnostic hooks, phase brackets and eight-case termination were
removed; the native harness is byte-identical to merged measurement source
`bc8da487`. The retained ignored profile emits only iteration, warm-up flag,
explicit draw wall time and test-only clip-build count.

Source implemented, 9 October 2026. The user-provided screenshot is the
visual reference: black filled signed waveforms, separate channel bands and
centerlines, with the musical grid visible through the clip. Native appearance
and timing remain user acceptance checks; exact Live rendering parity is not
claimed.

## cache and source fidelity

Control-side imports now request up to 65,536 disjoint leaf bins, capped by the
source frame count. Each bin retains independent signed min/max for both engine
channels. A pairwise extrema hierarchy permits allocation-free range queries
with logarithmic lookup work. Resident waveform counters include every hierarchy
bin, not only leaves. The hierarchy adds at most approximately another leaf
array plus logarithmic rounding overhead; existing source/import limits remain.
Peak construction stays on control workers.

The cache shares the existing immutable converted PCM allocation. Source ranges
of at most 64 frames use those samples directly, avoiding neighboring-bin peak
bleed at deep zoom. Wider ranges conservatively include intersecting leaf bins
and query the hierarchy. Cached peaks do not invent sample detail; sample-level
PCM is available when zoom reaches short source ranges. This reflects the current
project-rate stereo representation; mono decoding still duplicates channels.
No new schema or callback ownership path is introduced. Headless streaming and
future disk-backed waveform cache work remain separate.

`Waveform::from_peaks` validates fixture/cache extents and finite ordered extrema;
`peaks_for_frames` queries a half-open source range without heap allocation by
inspection. Queries are control/UI operations, never callback work. Signed
extrema must remain asymmetric where the source is asymmetric. Trims, looping
phase and source wraps map through the existing clip source rules.

## verification

Cache tests compare all ranges of an odd-sized hierarchy with direct signed
extrema, isolate a real transient at sample zoom, retain overview extrema and
reject malformed peaks. Session checks preserve media accounting/history and
import compilation. UI tests must cover zoom-dependent density, channel polarity,
clip/preview mapping and through-clip grid alignment. Native performance is
unmeasured; do not equate a bounded query with measured paint responsiveness.


## bracket/clip/waveform source milestone complete — 9 October 2026

Original bracket cursor variants, two retained macOS cursor objects, directional
fallbacks elsewhere, and a narrow test-only rendered-cursor request readout.
Verified original GPUI baseline and exact 12-file input/cursor delta; no unsafe
Hex code, proprietary assets or registry changes. Cursor delivery/scaling/hotspot
appearance remains native acceptance.

Audio/MIDI clips and prepared previews fill their 44px rows with a one-pixel
black border and warm fills. Shared black signed stereo waveform paths draw
zoom-adaptive detail from one query per column, with a bounded 4,096-column cap
and real sample traces at deep zoom. Grid lines remain aligned through fills
and beneath the waveform; empty-area feedback remains line/clip only.

235 UI tests pass, one ignored; workspace tests 700 pass, 1 ignored.
Media/session/UI strict lint, desktop build, formatting, architecture/style and
pinned GPUI provenance guards pass. Native paint timing remains unmeasured.
Review: `artifacts/hex-clip-visuals-20261009.app`; source/binary/log identity in
`artifacts/clip-visuals-manifest.json`. Check both bracket hovers, waveform zoom
and source detail, flush borders/grid continuity, and resize/drop previews.
User owns native checks; prior running apps/unsaved projects preserved.
No commit/push; broader roadmap remains paused.

## waveform paint correction and track context menu — 9 October 2026

The user reported a blank waveform in the clip-visuals review build. The shared
canvas had no intrinsic height, so its paint callback received zero-height
bounds and returned. Pinning the canvas to its parent fixes that layout path.
Removed the old 3.5-percent viewport-width waveform cutoff for normal and
resize-preview clips, and removed a crossing closure segment in the silhouette.
A rendered-layout regression checks full canvas bounds on 8px and 260px clips.
Native waveform appearance must be rechecked by the user; previous source checks
did not establish visible rendering.


Verification: 747 workspace tests passed, 1 ignored; 239 UI tests
passed, 1 ignored. Strict UI/project lint, desktop build, formatting and
architecture/UI-style/GPUI guards passed. Separate review bundle:
`artifacts/hex-waveform-menu-20261009.app`; source/binary/check identity:
`artifacts/waveform-menu-manifest.json`. User checks: visible waveform while
zooming/resizing; right-click track/empty space; insert position; Undo/Redo;
Escape and outside-click dismissal. Running apps and unsaved work preserved.
