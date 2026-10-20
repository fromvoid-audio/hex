# pinned main track

Pulse 12 owns the always-visible Arrangement main row. It is a fixed 44 px sibling
below the regular scrolling tracks and their trailing append/drop region, above
the shared clip/device detail panel. Its timeline grid uses the regular viewport;
its 176 px header aligns with regular track headers. Canonical master gain controls
move into this row; Session retains its existing master control strip.

Main is not a regular TrackId or clip/sample destination. Arrangement bounds
remain the upper regular region. The outer drag surface spans both regions so
crossing/releasing over main updates and cancels previews safely instead of
leaving stale regular drop targets. No project schema or audio routing changes.
Main-project time selection (ticket 14) is implemented in a separate focused
change under verification. Real return routing/rows (13) remain a separate feature.

Source checks pass: 304 UI tests with two existing ignores, including empty/short/
dense minimum 880×540 geometry, canonical master commands in Arrangement/Session,
clip/sample cancellation over main and regular append/import regression paths;
strict UI Clippy, architecture/style/GPUI guards, formatting and diff review.
Full clean-commit CI and native acceptance remain separate gates.

For the combined native build, verify the row stays at the bottom while scrolling
many tracks, including with the detail panel opened; the last regular track is
reachable. At minimum window size, check browser width, timeline/header alignment
and master controls. Drop a browser sample into trailing empty regular space,
then try a drop on main: only the regular drop should create a track. Existing
clip and captured time-selection drags should keep working across boundaries.

## project time selection

Pulse 14 treats main as an editing proxy over all canonical project tracks. A
point click places the cursor across those tracks; dragging paints one steady
time rectangle, preserving gaps. The upper-third hover handle moves that whole
rectangle horizontally; Option copies it. The lower region paints a new range.
Copy/cut/replace-paste and ripple insertion reuse the existing bounded worker
commands, immutable media and one Undo, without turning main into a TrackId.
An empty project has no valid proxy range. Topology changes reconcile the scope
with the current project; future canonical returns join it when implemented.

Command-E on the main cursor dispatches `SplitTracksAtFrame`. The project
validates a nonempty unique track set, converts the cursor frame to musical time
once, and splits strictly crossing audio/MIDI clips on a private candidate. A
later failure leaves the complete original intact, including direct Project
application; touching clip boundaries is not a split. One Editor command means
one history entry. Ordinary single-clip splitting retains its existing policy.

Focused source checks cover mixed audio/MIDI atomic splitting and one Undo on
100 tracks. They do not establish native input-to-paint latency or physical
audio deadlines. After storage recovery, all four project split tests, seven main-row UI tests
and eighteen edit-cursor tests pass. Empty/stale topology and explicit move-scope
retention regressions pass, along with strict project/UI Clippy and source guards.
Full local CI passed all 14 steps on clean `373e8c2` in 276.455 s, with
unchanged source digest and `stale=false`. Evidence:
`artifacts/local-ci/20261009T182718Z-b28879d0/summary.json`.
[PR31](https://github.com/PORTALSURFER/hex/pull/31) merged on 10 October; the user
accepted the combined native build that day. The remaining dense 1/100-track
feedback/completion timings were recorded on 16 October through supplementary
[PR258](https://github.com/PORTALSURFER/hex/pull/258):
[complete results and limits](evidence/time-selection-2026-10-16/report.md).
At 100 tracks, median selection paint is about33ms; this does not establish
60Hz or subjective smoothness. Pulse29 retains dense UI acceptance. In the
combined build, select time on main, copy/cut/paste and ripple-paste over gaps,
move through the upper-third handle, Option-copy, and split a main cursor across
audio and MIDI. Check Undo, selection after track deletion, and empty projects.

## completed selection persistence — pulse2386

Autoscroll maintenance runs only while a captured time-selection or range-move
gesture exists. An idle controller tick preserves the completed selection,
its start/end and track/project scope after mouse release. Stale active gestures
still cancel, and Escape explicitly clears the range. Selection remains view
state and creates no history entry. Rendered pointer-sequence regressions cover
audio, MIDI and empty lanes plus the main editing proxy, followed by idle ticks
and Escape. Native macOS confirmation on the exact optimized candidate remains
pending; source tests do not establish native paint timing.

## unsnapped main gesture projection — pulse2647

Captured Main-row selection, range movement and horizontal edge scrolling use
current Main-lane geometry throughout the gesture, including mouse release and
modifier updates. Regular-track gestures continue to use the nearest current
regular lane. A stale Main layout waits for fresh geometry rather than falling
back to a different row. This keeps an unsnapped stationary click at a zero-length
project cursor even when Main and regular lane widths differ, and prevents
range endpoints or move destinations from shifting by that difference.

The focused regression checks a stationary click, exact range endpoints and a
stationary move handle with differing widths and panned/resized geometry.
Native user check: disable snap, click Main without moving, then drag a range and
its upper-third handle after resizing/panning; verify cursor, endpoints and Undo.
Source validation and merge evidence are retained with PR263; native input is a
separate acceptance check.

## main audio effects — pulse2882 / pr484

The original row-only implementation above did not add a processing chain.
Main now has its own effect-only canonical device vector with global DeviceIds;
it remains a singleton, never a regular TrackId or clip destination. Clicking
its Arrange or Console header selects its shared device rail. Existing effect
controls, add/drop/reorder, bypass/remove, A/B comparison and presets address
that actual Main inventory. Track and Return selection keep their existing
owners. Instruments are refused in Main; selecting Main never inserts a track.

One prepared Main chain processes the sum of Tracks and Returns, followed by
existing master gain once, then the Main observer. Track/Return capture taps and
stems stay before Main/master processing. Arrangement, launched Session audio,
compensated routing, linear/prepared WAV and controlled range export share this
boundary. Controlled nonzero-range export reconstructs stateful Main history by
the existing bounded preroll; the simple linear export helpers retain their
existing seek semantics. No new DSP law, lookahead or latency cap is introduced.

Current format33 includes the required bounded Main inventory, both A/B banks,
host trims, bypass, stable IDs and complete device-asset inventory. Earlier
formats are explicitly refused; no alpha migration or user-data rewrite is
performed. Main uses existing per-chain/global processor and byte caps. Its
prepared runtime, physical owner kind9/index0, shared handles and meters join
current accounting. The narrow audio assembly constructor still explicitly
refuses authored Main, just as its existing restricted graph class refuses
other unsupported authored chains; ordinary compile/Session/export works.
Future395 custody overlays must carry Main on their combined modern source;
this change does not import or qualify that unpublished implementation.

Matching Main processor histories are inherited only after normal handoff's
fallible phases commit; the existing stopped compensated fast path also retains
history. Newly inserted processors start fresh. Unavailable prepared native
runtime and warmed legacy-history refusal policies include Main, preventing a
silent fallback or a cold-history substitution. Device-meter lookup resolves
the same actual prepared Main tap.

Source checks cover real header/add/reorder/parameter/Cancel/A-B/copy/preset and
project Save/Open requests, refusal without canonical mutation, exact Undo/Redo,
nonlinear post-sum PCM, stateful normal/fast handoff, exact combined runtime cap,
range-export history, pre-Main stems, and instrumented callback publication /
render / retirement. Exact receipt/source identities are recorded in Pulse;
these component checks are not the sole merger's prospective full integration
or physical device qualification.

For Root's later optimized native check, record the exact build/source: select
Main in both views, add an effect and edit it while audio plays; confirm Main
processes the complete sum once and master gain remains independent. Switch and
copy distinct A/B banks, reorder/bypass/remove and Undo/Redo, save/open a project
and preset, and verify ordinary Track/Return selection. Try an instrument and a
sample on Main: neither may create a Main instrument or clip. Check focused EQ
and metering against the selected Main device, plus dense/minimum-width layout.
This task does not claim native output deadlines, listening or user acceptance.


## Main review repairs — source continuation, 20 October 2026

The pre-Main summing graph retains its original Track/Return arrival, alignment,
clock and tap/stem contract. Compilation separately validates the actual enabled
serial Main processors' known algorithmic latency, adds it with checked arithmetic
under the existing cumulative compensation limit, and stores the final output
arrival in the existing prepared Project. The existing public latency report now
includes Main. Bypass and memoryless matrices contribute zero; unknown or excessive
latency refuses the candidate before publication. No artificial delay, new graph
node, queue, owner, runtime API or additional Main DSP pass is introduced.

Synthetic/empty constructors initialize final latency to zero. The restricted
assembly constructor still refuses authored Main effects and initializes the same
zero scalar; its exhaustive restore and inventory patterns acknowledge the scalar
without changing resource custody or claims. Actual Project layout measured2360
bytes both before and after this change on the checked cloud toolchain; no fixed
four-byte budget increment is assumed. Existing accounting uses actual layout and
retains the same limits. Native's separate prepared-carrier fields must compose by
hunk; no whole Native overlay is included here.

Session validation now includes Main's inactive comparison payloads in the same
Effect provider validation as Track/Return. The metadata-only classifier includes
Main in both comparison stripping and proof of a real typed Switch/Copy edge;
active provider bytes, host trims, bypass and other audible fields remain intact.
A saved project with valid active Main state and malformed inactive state now
refuses at Session open, preserving original file bytes. Valid comparison states,
both directional copies without switching, current-format round trips and atomic
Undo/Redo retain the existing format33 and global trim semantics. Absent B still
means an identical bank: Copy is a genuine no-op, not invented bank creation.

Red-first checks observed four actual Main latency failures (final15 reported7,
Main-only3 reported0, unknown admitted, cumulative65537 admitted) and two actual
Main comparison failures (invalid command accepted, malformed saved bank opened).
Separate fixture-API, exhaustive-pattern and initial-copy test-assumption failures
are retained as such. The repaired latency cases include actual PCM at multiple
partitions, Renderer, pre-Main captures and warm Main history/refused incompatible
handoff. Component qualification and exact evidence are recorded in Pulse2882;
sole-merger prospective validation, actual merge and native optimized acceptance
remain separate. No device/app/native operations are performed by this source lane.
