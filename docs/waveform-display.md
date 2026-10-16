# detailed Arrangement waveform display

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
batching is retained. The map clears on project generation or immutable snapshot
identity/revision change, releasing offscreen children and their old snapshots.
Each child's key includes track/index, viewport, musical grid/snap, display scale,
palette, selected clip and measured lane geometry/drag availability. Time ranges
and track endpoint focus deliberately stay outside this key.

Headers, lane geometry measurement, selection shading/cursors and transient
previews remain live. Main/track selection updates notify the editor instead of
globally refreshing the window; other drag/hover refresh behavior is preserved.
Any edit, modal, project-open or transient drag/pan bypasses the cache. The
original builder handles those states and the first idle frame rebuilds it.

The prototype regression proves actual clip-scene reuse while successive normal
input-path selection updates change the live overlay and lane epochs. The
existing selection persistence regression, strict UI/desktop lint and source
guards pass. A separate test-support limitation is visible: GPUI's cached
prepaint replay omits debug-selector bounds, so the existing audio-trim fixture
cannot locate its cached handle. That must be resolved without disabling cache
reuse before final verification. Native timing of this child-view experiment,
the full lifecycle regression set and the final unchanged 360-sample benchmark
remain pending; the 16.67 ms p95 target is not established by source checks.

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
