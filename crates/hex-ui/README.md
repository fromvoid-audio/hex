# hex-ui

## Purpose

Own GPUI views, display state, interactions, lowercase text, and bundled Ioskeley Mono styling.

## Dependencies

hex-project read-only metadata and commands; hex-session worker endpoints;
hex-engine contracts; hex-audio service handle; GPUI. The backend is created and injected by hex-desktop.

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
Opt-in pointer-move snap grids use the canonical map; automatic timeline/track
scrolling remains deferred.

Visible, uncropped clip edges expose compact horizontal trim handles. GPUI drag previews use canonical metadata-only edge-trim previews and dispatch one `TrimClipEdge` command on a valid same-track drop; canceled, stale, no-op and off-track gestures create no edit. Loop waveform summaries include the canonical source phase, so left trims keep displayed peaks aligned with playback.

The existing compact mixer strip includes selected-clip fade-in/fade-out controls in 5 ms steps with rounded project-frame increments. Clicks dispatch one canonical `SetClipFades` command and wait for the worker view before changing the readout or envelope. Split clips show a `split` marker while fade values remain on their inherited original span; the tooltip explains that editing reanchors and clamps both fades to the current clip duration. Visible nonneutral envelopes use the canonical model gain law, are clipped to the viewport and current clip segment, and use at most 32 regular samples plus four exact breakpoints.

The arrangement ruler and compact transport controls read the canonical tempo and meter maps. Tempo changes by 1 bpm at the cursor's nearest quarter-note point; the meter control cycles 4/4, 3/4, 6/8 and 5/4 at that same point. Ruler labels follow each visible meter section's denominator-beat and bar landmarks, converted through the canonical tempo map; meter markers retain priority, with spacing and a 64-tick cap. Frame viewport, seeking and playback remain frame-based. Clip-move snapping is opt-in (off, quarter, eighth or sixteenth); pointer preview and drop use the same canonical frame/beat resolver and cancel when snap state or the captured revision/view changes. Keyboard moves and trims remain frame-based.

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
