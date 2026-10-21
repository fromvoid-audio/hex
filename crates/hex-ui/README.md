# hex-ui

## Purpose

Own GPUI views, display state, interactions, lowercase text, and bundled Ioskeley Mono styling.

## Dependencies

hex-project read-only metadata and commands; hex-session worker endpoints;
hex-mts safe publisher handle; hex-engine contracts; hex-audio and optional hex-midi service handles; GPUI; the already-locked unicode-segmentation package supplies complete grapheme
boundaries in the existing native text-input primitive. The backend is created
and injected by hex-desktop.

The optional controller input row selects a CoreMIDI endpoint/channel for the
selected instrument and dispatches a generation-bound engine lease through the
session worker. See [controller input](../../docs/controller-input.md).

## Does not own

Device construction, callback DSP, project persistence, or duplicated musical scheduling.

Read ../../AGENTS.md and ../../docs/architecture.md before extending this crate.

Run `CARGO_INCREMENTAL=0 cargo test -p hex-ui --features ui-test-support`
for GPUI test-platform lifecycle regressions as well as normal unit tests.
Close tests replace a bootstrap root with a dirty editor and exercise Cancel,
Discard, repeated close requests and Cmd-Q prompt completion. This optional
feature is not enabled in the shipped desktop build; native macOS sheet behavior
still requires a separate click-through check.

The default editor lives in editor/: read-only views, bounded request dispatch,
native dialogs, keyboard editing, virtualized track rows and cached peak drawing.
The session worker owns edits and file operations. Icon actions have lowercase
tooltips; all view roots inherit the required bundled font.

app.rs and layout/ retain the generated demo behind --demo; those demo contracts
are separate from the canonical editor.

Canonical timeline controls include bounded zoom/pan/fit, track keyboard selection,
solo and selected-clip loop-region commands. Compact widgets share one constructor
and stable descriptive IDs; viewport/loop controls call view handlers directly.
The view displays the loop interval but never implements audio wrap scheduling.
The Arrangement draws a bounded musical grid using the project tempo and meter
maps behind clips and across empty space, with subtle alternating bands every
four adaptive grid units anchored to meter origins. The 24 px overview above the
ruler caches a 128×16 occupancy raster per project revision; audio and
tempo-converted MIDI clips share bins when they overlap, and track density is
aggregated into 16 vertical bands. Its outlined viewport can be repositioned by
clicking the overview. Clip moves snap to the same adaptive landmarks by default;
the snap control cycles adaptive, fixed quarter, eighth and sixteenth grids, then
off. Holding Command changes effective snapping without hiding the selected visual
grid, and bypasses an active grid or temporarily enables adaptive snapping when
the base grid is off. Vertical wheel gestures over the ruler or timeline zoom
around the pointer after a small bounded pixel accumulation, with physical wheel
up zooming in regardless of macOS natural-scrolling preference; wheel input
outside the Arrangement is left to its panel.
Middle-button dragging pans the horizontal frame viewport and the virtualized
track-list scroll handle, with capture through release and cancellation on Escape
or project/mode changes. Grid visibility is adaptive and separate from explicit
clip-edit snap settings.

The editor now has a canonical Arrangement/Session view toggle. Session scenes are
vertically virtualized; track columns are paged from the measured window width so
only the visible columns and scene rows produce cell views, with arrows to reach
every track. The grid reads the project's stable track, scene and cell IDs. Empty
cells, explicit stop cells and MIDI cells have separate display and selection
behavior. Session MIDI clips share the piano-roll interactions and source editor;
source edits dispatch `SetSessionMidiClipSource`, while Arrangement placement,
movement and duplication remain scoped to Arrangement clips. Paging is the first
bounded horizontal navigation model; continuous horizontal scrolling remains open.
With editor focus in Session mode, left/right moves across tracks and then through
the rightmost scene-launch column; up/down moves across scenes, clamping at grid
edges. A scene-only selection stays scene-only for vertical movement; Left enters
the last track cell, while Right stays at the scene column. With no selection, an
arrow selects the top-left cell, or the first scene when there are no tracks;
empty grids and stale IDs are safe no-ops. Enter launches the selected MIDI cell
or scene and activates a selected Stop cell for its track. Piano-focused arrows
keep their note-editing behavior, and Enter does not launch while the piano roll
owns focus.
With only a scene selected, the compact toolbar and `cmd-d` duplicate it, while
`cmd-alt-up` and `cmd-alt-down` move it by one scene. Duplicate selection follows
the accepted worker result only if the source scene remains selected in Session
mode; reorder keeps the same scene ID selected and reveals its accepted row. Cell
duplication remains unsupported.
The compact Session toolbar cycles local launch quantization between immediate
(`none`, the default) and a fixed four-quarter-note interval (`4 qn`). This setting
applies to launches, track/all stops and Arrangement return, is captured in each
queued command, and is not saved in project data. Four quarters is not meter-aware
bar quantization; global and per-clip quantization parity remain unimplemented.
The cancel-pending control requests typed cancellation and leaves route markers
unchanged until callback telemetry reports the resulting state.
In Session view, clicking a cell or scene header returns keyboard focus to the
editor. Delete clears an occupied selected MIDI/Stop cell, does nothing for an
empty selected cell, and removes a selected scene only when no cell is selected;
the piano roll keeps Delete scoped to its selected notes while it owns focus.

Session cell and scene launch requests capture project generation and revision,
require the corresponding audio revision to be applied, and revalidate stable IDs
before queue admission. A valid launch and transport start are one atomic callback
action, so a rejected launch cannot start unrelated Arrangement playback.
Unavailable Session playback is reported while editing and saving stay available.
A bounded latest-request ticket reports callback consumption or a typed rejection.
Accepted callback route telemetry drives compact Session track/cell markers only
when its exact audio revision maps to the audible read-only project view and the
displayed project generation. Separate 256-entry scratch and last-good route
buffers and one snapshot are tied to the live audio status channel; contended reads keep
that snapshot only on the same channel, while empty/unavailable snapshots, project
replacement, recovery, faults and shutdown clear or supersede it. At most 32
publication candidates retain their exact worker view; publication pauses at that
bound until callback acknowledgments free space. The grid shows an `audio updating`
cue when the audible mapping does not match the displayed project. Initial and
recovery graphs use the same exact-view mapping path. Route owner and
pending launch/stop/return glyphs come from accepted telemetry; transport playing
is read separately, so a selected Session owner while transport is not playing is
not presented as active playback. A committed Silence owner keeps a compact stopped marker after its pending
glyph clears. No route indicator is inferred from a click or consumed ticket. Rapid
space presses follow the latest admitted Play, Pause or launch intent while
commands wait for callback consumption.

Project saving uses `cmd+s` for the associated document; an untitled project opens
the create-new path dialog. `cmd+shift+s` always opens that dialog and refuses to
replace an existing file. Save work runs through the session worker.

The transport reserves the shared macOS titlebar leading inset for native window
controls; panel and track padding remain compact. Track rows remain 44 px. The
canonical mixer strip adds 26 px for selected-track balance and Main gain controls,
dispatching typed worker commands; it never changes DSP or stored state locally.

The `⟳` transport action requests a restart of the default audio output through
hex-audio's bounded lifecycle service. Recovery remains stopped and the UI waits
for the current graph and captured cursor seek before enabling Play/seek/split
again. It does not select devices; physical failure/restart acceptance remains open.

Arrangement audio and MIDI clips move within or across hybrid tracks through
canonical `MoveClip` or `MoveMidiClip` edits on drop. Outside Arrangement lanes,
the pointer carries a compact clip ghost; over a valid lane it disappears and
only the exact, viewport-clipped landing preview remains. Project edits wait
until a valid drop. MIDI remains beat-anchored and its preview bounds use the
canonical tempo map. See ../../docs/clip-drag.md for the drag contract and
remaining native acceptance.

WAV rows can be dragged from the browser into a hybrid Arrangement track.
Background preparation supplies the project-rate duration and a bounded waveform
summary; outside lanes the cursor ghost shows loading or prepared metadata, while
inside a valid lane only the exact snapped landing preview appears. A lane drop
uses the normal import worker and one undoable project edit. Invalid or canceled
drops do not import. See ../../docs/browser-drag.md for the contract and native
acceptance notes.
Browser WAV drops near a painted Arrangement row boundary show a full-width
track insertion line; blank space below the painted final row and an empty
Arrangement target an appended track with a locked landing preview. Between-row
insertion stays line-only. Only Arrangement body
geometry accepts these targets. Ruler, browser, mixer and outside release remain
non-targets. The worker receives a stable before-track ID or end anchor plus the
snapped start; it never receives a guessed row index.
Adaptive and fixed pointer-move snap grids use the canonical map; automatic timeline/track
scrolling remains deferred.

Visible, uncropped audio and MIDI clip edges expose 4 px-wide, 6 px-high bottom-corner trim handles; on clips narrower than 8 px each handle uses at most half the clip width. The remaining upper body stays available for moving clips, including at narrow zoom. During an active trim the original clip's painted contents are transparent while its stable hit geometry remains in place, and one in-track preview shows the candidate extents, waveform or bounded MIDI note marks. Default adaptive/fixed/off snap behavior and the platform modifier are resolved again for stationary modifier changes and release. Audio previews use metadata-only canonical trim and commit one `TrimClipEdge`; MIDI previews use `MidiClip::preview_edge_trim` without cloning authored notes and commit one `TrimMidiClipEdge`. Canceled, stale, no-op and off-track gestures create no edit. Loop waveform summaries include canonical source phase, so left audio trims keep displayed peaks aligned with playback. See ../../docs/clip-resizing.md for the detailed contract and native acceptance checklist.

The existing compact mixer strip includes selected-clip fade-in/fade-out controls in 5 ms steps with rounded project-frame increments. Clicks dispatch one canonical `SetClipFades` command and wait for the worker view before changing the readout or envelope. Split clips show a `split` marker while fade values remain on their inherited original span; the tooltip explains that editing reanchors and clamps both fades to the current clip duration. Visible nonneutral envelopes use the canonical model gain law, are clipped to the viewport and current clip segment, and use at most 32 regular samples plus four exact breakpoints.

The arrangement ruler and compact transport controls read the canonical tempo and meter maps. Tempo changes by 1 bpm at the cursor's nearest quarter-note point; the meter control cycles 4/4, 3/4, 6/8 and 5/4 at that same point. Ruler labels follow each visible meter section's denominator-beat and bar landmarks, converted through the canonical tempo map; meter markers retain priority, with spacing and a 64-tick cap. Frame viewport, seeking and playback remain frame-based. Clip-move snapping defaults to adaptive and also offers fixed quarter, eighth, sixteenth and off modes; Command temporarily toggles snapping for the current drag, including at mouse-up. Preview and drop use the same canonical frame/beat resolver and cancel when the base snap setting or captured revision/view changes. Keyboard moves and trims remain frame-based. This implements the documented grid/modifier subset; edge magnetism and exact Live shortcut parity remain unverified.

The MIDI editor separates canonical command helpers from cached Canvas painting/
input and pure selection/group geometry. Normal mode uses marker/marquee and
double-click authoring; B toggles single-click draw authoring. Sorted Arc-backed
selection supports Shift toggle/additive marquee and select-all. Group body
movement shares a bounded time/pitch delta, preserving expression and spacing;
changed release and group deletion dispatch atomic worker commands. Dedicated
piano focus routes arrows/delete to notes. Group edges/parameters and continuous
draw strokes remain deferred. Native selection/group acceptance is recorded in
docs/midi.md; source tests alone do not establish every native gesture.

Continuous MIDI drawing lives in editor/midi_stroke.rs (pure bounded resolution)
and editor/midi_draw.rs (editor admission/lifecycle/coordinates), with shared
preview geometry in midi_scene.rs and thin Canvas routing/painting. Pointer
motion is local; valid release sends one mixed canonical transaction. Native
whole-project drawing acceptance is still pending; see docs/midi.md.

The selected MIDI clip exposes compact grid-stepped controls for finite
Arrangement length, source extent/start, retained loop bounds and loop enable.
Every accepted Arrangement change dispatches the full atomic `SetMidiClipSource`
command; Session source changes dispatch `SetSessionMidiClipSource` and omit the
Arrangement-only length control.
Arrangement and source durations stay independent. Invalid bounds, overflow and
source shrinks that would exclude authored notes are refused. The panel reads
only clip metadata while painting; authored note ends are scanned only when a
source shrink is requested. Beat labels use compact scientific notation above
the normal display range to keep the six control groups inside the minimum-width
window.

Prepared MIDI Arrangement thumbnails are read-only summaries built with the
compiled project, not projected during Canvas paint. They sample at most 32
occurrences per clip, and the Arrangement painter draws at most 128 note shapes
per track row, showing a sampled-count badge when content is denser. Native sparse
clip fit/crop and badge checks pass; source-control interaction, minimum-window
layout and dense-content visual acceptance remain open.


## Arrangement clip visuals and edge cursors — 9 October 2026

Audio and MIDI clips fill the full 44 px lane with a 1 px black border and a
warm coral fill that keeps the black waveform and musical grid legible. Trim
previews and landing previews use the same row bounds and border. Cached stereo
signed extrema are painted as two bounded, filled waveform bands at device-pixel
density (capped at 4,096 columns); each column queries both channels once, and
deep zoom reads only the session's bounded source window. Looping and trim source
phase use the canonical clip timeline mapping. Drag ghosts with prepared peaks
reuse that painter; pending imports show only a neutral marker.

Thin invisible left/right edge targets remain full-height, with the central body
kept available for clip movement at minimum widths. They request distinct bracket
cursors. macOS retains two original high-contrast AppKit cursor images for the
platform lifetime; other platforms use directional resize cursors. Rendered
GPUI tests verify which edge requests each bracket. Native macOS cursor artwork
and drag appearance still require the user's click-through review.

Reusable context-menu presentation lives in `src/context_menu.rs`; the editor
track menu dispatches canonical hybrid insertion commands. See
[context menus](../../docs/context-menus.md).

Arrangement edit-cursor state and overlay live in `editor/edit_cursor.rs`.
Command-E dispatches canonical audio/MIDI split commands using the clicked
position independently of playback. See [edit cursor](../../docs/edit-cursor.md).

The unified clip detail pane has a compact open/close chevron and keeps selected
clips/notes when collapsed. Hiding cancels note gestures and releases MIDI focus.
Arrangement audio provides a cached stereo source waveform, ruler and markers,
with canonical position/length/source/play-start/loop edits in a narrow left
column. MIDI uses the existing note editor with its source controls on the left;
Session audio retains its existing source controls in the shared slot. Gain,
pitch, warp modes, numeric entry and marker dragging remain follow-ups.
See ../../docs/clip-view.md for contracts and user acceptance checks.

Waveform drawing retains ready-to-paint paths in the editor-owned bounded
`waveform_geometry` cache (512 entries, 65,536 aggregate device columns). Keys
include retained source identity, clip/source mapping, visible interval, bounds
and scale. Project replacement clears cached ownership. Arrangement/detail
redraws reuse unchanged geometry; live trims omit invisible original paint
children and suppress notifications for identical snapped candidates. Pending
control colors and compact status slot widths remain stable. See
../../docs/clip-edit-performance.md for measurement scope and native checks.

Arrangement rows retain bounded GPUI child views for unchanged legacy clip
content. Lane geometry, headers, selection overlays and transient previews stay
live in the editor. Project generation clears cached ownership; snapshot updates
replace all retained snapshot Arcs and prune removed tracks. View/grid/palette,
selected-clip and measured-lane changes advance an explicit scene cache epoch,
preserving input identity. Busy edits and drags rebuild original child content
each draw. See ../../docs/waveform-display.md for the cache contract, ignored
headless framework diagnostic and pending native timing gate.

## arrangement time selection — 9 October 2026

`editor/edit_cursor.rs` owns the track-scoped point cursor and split action;
`editor/time_selection.rs` owns transient range gestures, steady overlays,
endpoint focus and clipboard-request routing. Authoritative range edits stay
in hex-project and the Session worker. See [time selection](../../docs/time-selection.md)
for native acceptance and finite MIDI limitations.

Session logging is injected by desktop through `hex-diagnostics`; only that
crate's worker owns log file I/O. See [session diagnostics](../../docs/diagnostics.md).

Device rails support middle-button horizontal panning and a compact bottom-status
chain overview with a draggable viewport frame, click/wheel navigation and a
device-pane toggle. Tracks and returns share the measured GPUI scroll handle;
navigation is view state and does not enter project history.

The explicit `hex-desktop --profile-time-selection` mode embeds the real editor
in a disposable offline native profiling root. It injects input only into that
window and reports scene-paint/following-frame timing, with isolated preferences
and no recovery journal. Normal startup/editing is unchanged. See
../../docs/time-selection-profile.md for fixture shape and timing limitations.

Persistent native scalar controls and their injected panel policies live in
`editor/device_panel/bindings.rs`; numeric Session adaptation lives in
`gesture_adapter.rs`. The host retains project/worker lifecycle and compound
edits. See [native panel host bindings](../../docs/native-device-bindings.md)
for captured identity, exact gesture ownership and the remaining independent
embedding boundary.

The test-only `hex-media` dependency constructs bounded verified-source limits for
real worker/key-handler audition-refusal fixtures; production UI does not decode media.

Sampler computer-keyboard audition reuses existing focus/physical-key/toggle
state. An output-bound epoch is reserved before worker activation; correlated
acknowledgement enables direct Client desired-key publication. Key up and terminal
cancellation bypass worker I/O, with old-held/autorepeat suppression. Empty MIDI
tracks can first-load samples; controls/replacement/history remain unavailable.
See [the native keyboard boundary](../../docs/sampler.md#native-computer-keyboard--pulse2966--pr550).
