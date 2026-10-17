# Hex

An independent Rust DAW reconstruction project. The long-term goal is to study
Ableton Live through behavior experiments and targeted decompilation, then
reimplement the required workflows and audio behavior in Rust.

Reference: `/Applications/Ableton Live 12 Suite.app`, version
**12.4.6 (2026-09-10_0de5c8fa9a)**. Start with its arm64 slice.
See [reference inventory](docs/research/reference.md).

## Current state

The default GPUI desktop now edits a canonical audio project, with background WAV
import, cached waveforms, compiled CoreAudio playback, track gain/balance/mute/solo
and master gain, clip nudge/trim/delete/split/duplicate, editable fades with hold-f endpoint/curve handles and source-implemented pointer dragging, timeline zoom/pan/fit,
undo/redo, project
open/save/save-as and background PCM16 WAV export. The older four-track generated
Arrangement/Session prototype remains available with `--demo`.
App text uses lowercase and embedded Ioskeley Mono with compact Yield styling.
Canonical Session scenes and independent MIDI/stop cells persist in schema 11.
Every revision prepares their immutable inventory before edits/history/open commit,
with compatible source reuse and structural per-track work envelopes. Playback
retains that exact artifact while rendering Arrangement. A separate headless
SessionPlayback kernel supports mixed track routes, quantized launches and bounded
outgoing releases, loop-aware tempo and transactional phase-preserving revision
handoff. Prepared native Playback now delegates capable graphs to that kernel;
bounded action/publication queues retain transient handoff refusals, report
permanent rejections and reclaim graphs off the callback. The canonical editor
now has a compact Session grid and shared MIDI source editor, with atomic
clip/scene launch-and-play, ticketed rapid launch/toggle intent, ordered priority
Stop, and accepted-revision active/queued route indicators. Native Session
interaction remains unverified.

Arrangement musical audio includes pitch, beat anchoring, source gain envelopes
and six distinct bounded warp modes, with prepared output waveforms; see
[musical editing](docs/musical-editing.md) for limits and native gaps.

This is an early working foundation. Audio input recording, MIDI hardware I/O/expression,
device selection, plugin hosting and Live-set compatibility remain planned.
Typed stereo/MIDI routing, track-gain editing/touch recording and declared-latency
compensation are described in [routing](docs/routing.md); native interaction and
physical device acceptance remain separate. A bounded
fade-controller callback/target has been decompiled into private research storage;
its types/control-flow warnings remain unresolved and no audio behavior is matched. Project-open validation and
schema migration pass source tests; final native replacement acceptance remains open.
Token-checked existing-project replacement and create-new Save As are implemented
in the session and editor source; fresh native Save/Save As interaction acceptance
remains open. Asset relinking is deferred.

## Run

On macOS, `./launch` fetches the latest `origin/main`, fast-forwards a clean local
`main`, builds and installs the optimized app at the configured
stable path (default `artifacts/hex.app`), then opens that verified build. It refuses
dirty, non-main, ahead or divergent checkouts. If the update changes the launcher,
it re-executes from the updated source before continuing.

Normal launch always fetches and invokes the optimized build, even if an app is
already installed. It does not run tests or require an earlier CI report. Source,
artifact and configured signature checks still apply; receipts record tests as not
run. `./launch --test` adds full exact-source local CI before building.
`./launch --open` only verifies and opens the installed optimized app;
it does not update source, build or run tests. This mode can open an already running
installed app normally. The update/build workflow requires the native toolchain and
authenticated `gh` for PR provenance. Save and quit hex before using it; it refuses
running installed or raw `hex-desktop` processes and never force-quits. Build
provenance and a finite smoke checklist remain under `artifacts/launch/` and
`artifacts/local-ci/`; native
interaction and listening results remain user checks.

```sh
./launch          # fetch latest main, build/install and open; no tests
./launch --test   # also run full exact-source local CI before building
./launch --open   # open the installed optimized app without updating/building
cargo run -p hex-desktop
cargo run -p hex-desktop -- --demo
# Optimized stable test delivery after exact-source CI (see docs/test-builds.md):
sh scripts/bundle-macos.sh --help
python3 scripts/check-architecture.py
python3 scripts/check-ui-style.py
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
python3 scripts/local-ci.py --quick  # guards and formatting only; not full CI
python3 scripts/local-ci.py        # complete local validation pipeline
mkdir -p artifacts
cargo run -p hex-cli -- demo artifacts/hex-demo.wav
cargo run -p hex-cli -- edit-demo artifacts/hex-edited-demo.wav
cargo run -p hex-cli -- render-wav source.wav artifacts/imported.wav
cargo run -p hex-cli -- import-project source.wav artifacts/project.hex
cargo run -p hex-cli -- render-project artifacts/project.hex artifacts/project.wav
cargo run -p hex-cli -- recover-project artifacts/project.hex artifacts/recovered.hex
```

User-test builds use the optimized stable development app at `artifacts/hex.app`,
with exact-source CI, provenance and verified replacement without retained
rollback copies. The bundle script refuses to overwrite a running app, including macOS case-insensitive
`Hex.app` aliases. Save and quit before retrying; launch/relaunch manually after
delivery, or use `./launch --open` to open the installed app. See [native test builds](docs/test-builds.md) for the required arguments
and repository-owned build/install process. Pulse manages tickets only.

The demo is four seconds at 48 kHz and 120 BPM: one silent beat, followed by
seven repetitions of an enveloped 220 Hz tone. Output creation refuses to
overwrite an existing file. The engine preserves floating-point headroom;
WAV export clamps to the 16-bit range. Listening acceptance remains pending.

## Project map

- `crates/hex-project`: canonical editable metadata, stable IDs, validated commands and history.
- `crates/hex-warp`: pure bounded control-side stereo transformation.
- `crates/hex-media`: bounded WAV decoding and cached stereo peak summaries.
- `crates/hex-stream`: verified WAV worker, fixed page pool/cache and ownership queues; headless streamed engine rendering, with native integration pending.
- `crates/hex-session`: canonical controller, retained media and bounded background operations.
- `crates/hex-storage`: versioned metadata, create-new files and token-checked recovery replacement.
- `crates/hex-engine`: project compiler, platform-independent timeline and renderer.
- `crates/hex-devices`: prepared original sine/envelope DSP.
- `crates/hex-audio`: device streams and bounded audio controls.
- `crates/hex-library`: bounded background folder, WAV and project listings.
- `crates/hex-ui`: GPUI shell, compact left browser, lowercase display text, bundled Ioskeley Mono.
- `crates/hex-desktop`: startup and composition only.
- `crates/hex-export`: shared fixed-block linear PCM16 WAV export and create-new publication.
- `crates/hex-cli`: generated audio and headless WAV export through hex-export.
- [Pulse](https://pulse.iocore.dev/): active project and issue planning for Hex.
- [Plan](docs/plan.md): milestone contracts and acceptance gates.
- [Architecture](docs/architecture.md): implemented contracts and future boundaries.
- [Research workflow](docs/research/workflow.md): evidence and decompilation process.
- [todo.md](todo.md): request order and repository implementation evidence;
  Pulse owns the active queue and ticket states.

“Pure Rust” means Hex's application logic, scheduling, and DSP are Rust.
GPUI 0.2.2 uses the native macOS UI/GPU boundary and CPAL 0.17.3 uses CoreAudio;
these are intentional system interfaces. `rtrb` 0.3.4 provides a bounded SPSC
control queue. `hex-project` uses only the standard library; `hex-engine` depends on that model and std-only `hex-devices` DSP. All workspace
code forbids unsafe Rust; native dependencies may use FFI internally.

The toolbar keyboard toggle or M enables live computer MIDI audition on the selected
instrument track. A/W/S/E/D/F/T/G/Y/H/U/J/K/O/L/P/; span seventeen semitones from
MIDI60; Z/X shift octave and C/V change velocity. It works stopped and playing,
preserves text entry, and adds no recorded notes/history. Release/repress after
focus, Stop or graph changes. Positive-latency layouts currently refuse monitoring;
see [computer MIDI keyboard](docs/midi.md#computer-midi-keyboard--2642--pr257).

Desktop shortcuts: Space plays/pauses; Escape cancels an active clip drag, otherwise
stops and returns to start; the Stop icon always stops transport;
Command-I imports a WAV; Command-O opens a project; Command-S saves its associated
file, or opens Save As when untitled; Command-Shift-S always opens create-new Save
As. Command-Z and
Command-Shift-Z undo/redo. Select a clip, then Left/Right nudges it by a quarter
second, Shift-Left/Right adjusts its end, and Delete/Backspace removes it. Command-D duplicates it immediately after
its end; rapid repeated presses chain from returned canonical IDs. Command-E
splits a selected one-shot clip at the playback cursor, preserving its source
audio; looping clips and out-of-range cursors are explicitly refused.
Pointer dragging moves an audio clip within or between tracks using one undoable
edit on drop; Escape or dropping outside a lane cancels. Stale/project-busy drops
are refused. Snap grids, trim handles and drag autoscroll remain planned; fresh
native drag acceptance is open.
Selected-clip fade controls occupy the existing mixer strip, with 5 ms steps,
compact readouts and an envelope preview. Splits preserve the original envelope;
editing or trimming reanchors it. Native edit/save/undo/redo passed on an original
muted fixture. Listening, curve adjustment and Live fade-law parity remain open.
See [clip fade contract](docs/clip-fades.md).
Click the timeline ruler to seek; splitting waits for seek acknowledgment.
Up/Down selects tracks; S toggles exclusive solo and Shift-S additive solo.
Compact zoom buttons change the viewport; Alt-Left/Right pans and Command-0 fits.
Command-Plus zooms in and Command-Minus zooms out (also bound to Command-Equal
where the keyboard layout delivers it). Command-L sets/enables the loop from the
selected clip; L toggles it. Native loop playback honors this region; CLI export
renders the linear arrangement.
Command-Shift-E exports the current linear Arrangement to a new stereo PCM16 WAV
in the background. It captures the latest compiled revision, writes from frame
zero through the latest clip end, ignores the global loop, and refuses empty
projects or an existing destination. One export may be active at a time; closing
while it is pending warns that it may not finish.
Command-Q quits. Initial project rate follows the output device; mixed-rate WAVs
are converted on the background worker. Opening still requires the saved project
rate to match the active device. When output fails, the ⟳ toolbar action requests a
restart of the default output and restores the current project graph while
staying stopped. Device selection is not available. Recovery source checks pass;
physical device failure/restart and listening acceptance remain open.

See [feature roadmap](docs/features.md), [runtime design](docs/runtime.md), and
[performance baseline](docs/performance.md). The app is a local development build,
with no signing, installer, or distribution release.

The local validation runner retains per-step logs and a structured summary under
`artifacts/local-ci/`; full-run evidence is valid only for the recorded unchanged
commit and working tree. See [local CI](docs/local-ci.md) for its scope and limits.

Native export was accepted on a fresh development bundle: revision 1 exported
507,150 frames at 44.1 kHz, stereo PCM16. Its WAV matched an independent linear
render byte-for-byte. This confirms the tested fixture and path only; listening
quality and sustained export performance remain unverified. Progress, cancellation,
overwrite, stems, and user-selected ranges are not implemented.

[PhotoCraft](https://github.com/storytold/photocraft) is the engineering reference;
[adaptation notes](docs/reference-project.md) separate adopted patterns from future
work. Crate responsibilities are documented in each crate's README and checked
by the architecture policy. [UI rules](docs/ui-design.md) require lowercase app
text and embedded Ioskeley Mono; attribution is in ATTRIBUTION.md.

The scripted `edit-demo` trims the source region, moves the start to beat three,
reduces gain and exercises undo/redo before export. It demonstrates the control-side
model and compiler; the native editor now dispatches the same validated project commands.
See [implementation issues](docs/issues.md) for constraints and next acceptance steps.

Imported WAV support: mono/stereo PCM8/16/24/32, float32/64, extensible PCM/float,
G.711 A-law/mu-law and Microsoft/IMA ADPCM, 8–192 kHz, finite
samples only. Initial CLI ceilings are 64 MiB input and 8 million resident stereo
frames; project rendering enforces that frame budget across referenced assets.
Source-rate conversion uses a versioned windowed-sinc filter; same-rate PCM stays
bit-identical. Source decode and prepared playback have separate 8-million-frame
limits. Native playback/import remains resident. The headless `hex-stream` worker
and borrowed engine renderer now support bounded verified pages; native Session,
publication, history and export integration remains pending. No device hosting yet.
Track balance ranges from -1 to 1 with unity center; the original Hex linear
balance attenuates the opposite channel without boost or crossfeed. Master gain
ranges from 0 to 2 and applies after track summing without a limiter. These are
Hex-defined controls; Live pan-law parity has not been measured. Project schema 11
preserves original source rate/count, prepared playback length, conversion version,
solo, loop, balance, master gain, fixed clip-fade domains and ID high-water state.
Schema 1 migrates identity media; schema 1/2 tracks default to solo off, and
schema 1/2/3 projects default to no loop region. Schemas 1–4 default new mixer
fields to center balance and unity master gain. Schemas 1–5 migrate to neutral
clip fades; schemas 1–6 migrate loop source phase to zero. Schemas 1–7 receive
120 bpm and 4/4 canonical tempo/meter defaults. Schemas 8–11 preserve bounded tempo
and meter maps, including midbar meter changes. Existing audio stays frame-anchored
when these maps change; audio warping and beat-anchored audio remain future work.
See [docs/musical-time.md](docs/musical-time.md) for conversion and rounding rules.
Schema 9 tags audio versus instrument track contents and preserves headless MIDI
notes with musical clip anchors and stable IDs. Schemas 1–8 migrate all tracks as
audio. Schema 10 adds independent MIDI source/start/loop settings and finite
Arrangement duration; schema 9 migrates to unchanged one-shot defaults.
Schema 11 adds stable scenes and sparse Session cells with independently owned
named MIDI sources or explicit stop cells; schemas 1–10 migrate to an empty grid.
Source copies allocate fresh clip/note IDs, and commands/history preserve both
ownership locations. Session data, source preparation and a headless mixed-route
launch kernel and periodic loop-aware tempo are implemented. Native prepared
publication and action queues, compact grid and shared Session-source piano-roll
integration and accepted-revision active/queued grid indicators are implemented
in source. Native interaction remains pending.
Canonical source edits, looped playback/export and source-aware piano bounds are implemented;
prepared loop thumbnails and compact source controls are integrated; native
minimum-layout and listening acceptance remain pending. Instrument
tracks now render an original sine instrument through shared
native/offline playback, with bounded voices and checked candidate compilation.
Native cmd+shift+t creates a hybrid track; cmd+shift+m creates a four-quarter
MIDI clip. Selecting it opens a compact piano roll for insertion, pitch/onset/gate/
velocity edits and delete/undo. Musical clips support move/duplicate/delete and
Save/reopen. Direct body drag and both edge resizes preview locally and commit one edit;
off-roll release cancels. Native move/resize/undo and exact saved normal/short-note
metadata checks pass. The cached Canvas paints all visible notes, supports overlap
cycling and local zoom/fit/pan, and passes native 600-note and minimum-window
checks. Configurable fixed/free grids pass native author/move/resize/save checks.
Normal/draw modes, Shift/marquee/select-all selection and atomic group body move/
delete are implemented. Native group saved-value/history, no-op/off-roll and
focused single-note arrow checks pass. Native additive marquee, held-modifier gestures, held-Escape and measured
input/paint timing remain work. Group arrow movement/transposition, undo/redo,
boundary no-op and minimum layout pass native checks. Continuous Draw mode now
previews insertions/erasures locally and commits one mixed edit on release, with
pitch-lock/melodic controls and cancellation on stale context, focus or resize.
Source tests pass; initial native locked drawing passes visual/revision checks,
but pristine saved stroke/history/erasure acceptance remains open. See
[draw acceptance](research/observations/midi-draw-native-001.md).
Hold Cmd while inserting or dragging a note/edge to invert snapping temporarily;
Native saved-value checks cover both insertion inversion directions; held-modifier
dragging and mid-drag toggles remain source-tested with native acceptance open.
See [docs/midi.md](docs/midi.md) for rendering limits and remaining workflows.
Unknown fields/versions are
rejected. Save As refuses overwrites. Existing-file
Save Current uses a captured content token, one validated rolling recovery file,
and atomic replacement; an external content change causes a conflict. If a
create-new publication succeeds but cleanup or directory synchronization fails,
the save error names the already-published path and the editor conservatively
keeps the project marked unsaved. Native Save/Save As interaction acceptance remains open; missing-media relinking
and explicit journal recovery are source-implemented in Pulse25/PR151. Prepared
playback handoff connects the canonical editor to CPAL; graph
retirement and stream disposal stay away from the realtime callback.

`recover-project` explicitly copies a valid recovery sidecar to a new project
path after validating it. It preserves the existing target and sidecar; there is
no silent recovery on startup. The native editor checkpoints canonical edits to
one saved-project journal or the latest-only application untitled slot. Explicit
File-menu recovery opens dirty without overwriting saved documents; missing media
can be relinked after rate/length/decode/compile admission. See
[project recovery](docs/project-recovery.md) for ownership, tests and native gaps.

Opening validates every saved asset before replacing the project; failure leaves
the current session intact. A nonempty project requires invoking open again to
confirm discarding it. Saving an opened project into another directory resolves
relative asset references against its original document directory, preserving
path spelling/case without mutating current history.

The editor marks unsaved work and guards ordinary window-close/Cmd-Q with an
asynchronous cancel-first discard prompt, including pending operations. Undo
conservatively marks the project unsaved. Forced termination and app-level OS
quit are not crash-recovery mechanisms; a recovery journal remains planned.
Native and CLI defaults both cap resident audio at 8,000,000 stereo frames.

## Hybrid-track update — 9 October 2026

Every canonical track now accepts audio and MIDI clips under one stable track
identity and shared mixer controls. Schema 13 stores both clip collections;
legacy schemas 1–12 migrate their typed contents without splitting tracks.
Arrangement sources sum when overlapping; Session cells remain one source per
track/scene and may alternate audio or MIDI on the same track. Source-based
editing retains frame-anchored audio and beat-anchored MIDI.

Workspace tests/lints/build, independent exact-sum rendering and schema/history
acceptance, GPUI support tests and callback allocation probes pass. User native
acceptance and the review build are tracked in
[hybrid tracks](docs/hybrid-tracks.md) and todo.md. Original sine playback remains
the MIDI instrument; plugin hosting and instrument replacement remain future work.

## Lattice device foundation — 9 October 2026

Schema 14 adds canonical ordered native effect chains with stable instance IDs,
bypass, history and generic versioned state. Older schemas migrate to empty
chains. Device definitions own their codecs and register with lattice; storage
does not contain per-device branches. Unknown/newer device state round-trips,
and unsupported processing refuses compilation explicitly.

The first processor is original linear Utility, compiled into a shared stereo
matrix for Arrangement/Session audio and MIDI. This foundation does not yet
provide the desktop device panel, stateful DC/bass-mono controls, Operator or
plugin hosting. See [lattice](docs/lattice.md) for contracts and open acceptance.

## Return routing — 10 October 2026

Schema-17 return buses render regular-track post-fader sends through registered
zero-algorithmic-latency effects before the Main master gain in Arrangement,
SessionPlayback and prepared export. Fixed return scratch and effect runtimes
are built on control; compatible history survives prepared revision handoff,
with callback allocation probes. Typed track inputs and cumulative latency alignment
extend this topology; feedback remains refused. Native pinned return rows and
physical listening/deadline acceptance remain separate. See
[return tracks](docs/return-tracks.md).

## Source repository

Private development repository: https://github.com/PORTALSURFER/hex.
Build artifacts, private reference analysis and local caches remain ignored.
Distribution licensing and public release remain undecided.


Browser-added folders persist in `~/Library/Application Support/hex/browser.json`
through bounded background preferences load/save. Wait for saved feedback before
restarting; unavailable folders remain listed and settings failures are visible.
See [browser places](docs/browser-places.md). The latest source repair also
restores cross-track landing previews and tests repeated MIDI pickup; native
retest is pending.

Browser WAV drag now shows a cursor-attached clip outside Arrangement lanes and
an exact locked landing preview over a valid track, importing there on release.
Ordinary audio/MIDI clip drags use the same free/locked visual modes. Preparation
is bounded and background-only. This supports in-window dragging; outbound
file drag to other apps requires a separate native integration. See
[browser dragging](docs/browser-drag.md) for pending native checks and limits.

Browser WAV drops on a track divider create a new hybrid track there; drops below
the final track or into an empty Arrangement append a track. Dividers show an
insertion line; blank space retains the cursor ghost. New track, asset and clip
are one undoable prepared edit.
See [browser dragging](docs/browser-drag.md) for pending native acceptance.

Existing audio and MIDI clips can also create inserted/appended hybrid tracks
by dragging, preserving clip/source/note IDs and source tracks. Bottom blank
space shows line plus cursor ghost; dividers show line only. Sample insertion
uses the actual timeline origin and overlays no longer change workspace height.
Native retest is pending in the latest review build.

Arrangement drops onto existing tracks overwrite covered portions of audio and
MIDI clips, retaining uncovered fragments, in one compiled undoable edit. Blank
drop areas highlight subtly; empty projects have no cursor-height insertion
line. MIDI right fragments become finite one-shot sources and held notes
retrigger at the cut boundary. See [overwrite contract](docs/clip-overwrite.md).
Source checks pass; review app: `artifacts/hex-overwrite-drop-20261009.app`. Native
acceptance remains pending.

Arrangement navigation work adds an adaptive musical background grid,
pointer-anchored mouse-wheel zoom and middle-button horizontal/vertical pan.
Navigation is view-only; the subsequent snapping extension enables adaptive
clip snapping by default with Command override. Source verification
and native acceptance are tracked in [Arrangement navigation](docs/arrangement-navigation.md).

Adaptive clip snapping is enabled by default and shares the visible grid's
spacing. Command temporarily bypasses it (or enables adaptive snap when off).
Blank Arrangement drops preview clips on the prospective new-track row instead
of following the cursor freely. Review: `artifacts/hex-grid-snapping-20261009.app`;
[snapping contract and native checklist](docs/grid-snapping.md). 220 UI tests
pass/one ignored; native review remains pending.


The Arrangement overview now fits all tracks and audio/MIDI clips above the
ruler, with cached compact drawing, viewport outline and click navigation.
See [arrangement-overview.md](docs/arrangement-overview.md). Command preserves grid visibility; alternating
four-unit bands and the native input repair are in this review milestone.
Native visual and physical-input acceptance remain pending.


Audio and MIDI Arrangement clips now use compact bottom-corner resize targets
while retaining the upper clip body for movement. Resizing paints the proposed
clip and waveform/note geometry immediately, uses the selected grid and allows
Command free movement. One canonical edit is submitted at release. See
[clip-resizing.md](docs/clip-resizing.md); native acceptance remains pending.


The latest Arrangement style follows the supplied Live screenshot: flush
full-track clip height, a one-pixel black border, warm fills with dark signed
channel waveforms, and aligned grid lines through the clip. macOS resize edges
use original bracket cursors retained by the existing external GPUI boundary.
Waveform cache/query fidelity and open native timing checks are recorded in
[waveform-display.md](docs/waveform-display.md).
