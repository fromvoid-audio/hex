# Fundamental DAW functionality

The target is an editable, recordable audio/MIDI DAW with Session and Arrangement
workflows. A row is complete only when its acceptance workflow passes. Live parity
is a separate versioned reference gate, not implied by implementing the feature.

| Area | Current state | Next acceptance workflow |
| --- | --- | --- |
| Hybrid tracks | Unified audio/MIDI collections under one track/mix; Arrangement source-sum, schema-13 roundtrip/history, Session switching, worker/UI and allocation checks pass | User native mixed-source import/edit/save/launch and listening acceptance |
| Native desktop | GPUI shell, scrollable workspace | Resize, focus, accessible keyboard and controls |
| Audio output | CPAL default output, visible error, source-integrated restart of the default output with stopped graph/cursor restore | Native failure/restart acceptance, device selection, disconnect recovery and long stress run |
| Transport/tempo | Canonical Play/pause/Stop, ruler seek, persisted loop region and bounded tempo/meter maps; frame audio unchanged by tempo edits | Playable musical clips, beat-anchored audio, tempo ramps, scrub, drag loop handles, metronome |
| Arrangement | Native imported clips, track-scoped slow white/cyan edit cursor/top-bottom markers, steady time-area selection with endpoint focus and cut/copy/paste/delete/move, and cursor-based audio/MIDI split, nudge/trim/delete/duplicate, linear fades with split continuity, audio/MIDI same/cross-track pointer moves with exact in-track landing previews, and audio/MIDI edge resizing with live previews, grid/Command modifiers and thin corner handles, cached peaks; viewport zoom/pan/fit | Native thin-handle/live-resize acceptance, dense interaction acceptance, snap for nudges, drag autoscroll, adjustable fade curves and measured Live fade comparison |
| Session | Canonical stable-ID scenes, sparse independent MIDI/audio one-shot/explicit-stop cells, copy/move/edit/history, schema-13 persistence, immutable prepared inventory, structural per-track maxima and exact precommit revision publication; headless mixed-route launch/stop/return kernel with bounded outgoing releases, transactional rendering, periodic loop-aware tempo and accepted phase-preserving revision handoff; source-accepted native prepared action/publication queues with dual clocks, capability fallback and off-callback reclamation; compact paged canonical grid, atomic clip/scene launch-and-play, ticketed rapid toggle intent, ordered priority Stop, shared Session MIDI editor and compact cached-waveform audio source controls; accepted-revision current/queued route markers, including same-cell retriggers; fixed demo playback remains separate | Native audio source/import/edit/save/launch interaction, warped audio loops and source envelopes, global/per-clip quantization controls beyond the kernel's immediate/four-quarter modes, mixed-route physical admission, natural horizontal scroll, follow behavior and recording to Arrangement |
| Mixer | Canonical gain, original Hex linear balance, mute, exclusive/additive solo, master gain and output peaks; schema-17 returns/sends with Arrangement, SessionPlayback and prepared-export routing, declared-latency admission and cumulative path alignment, typed Track/Return inputs, compatible history and bounded retirement | Native pinned return rows, listening, physical deadline acceptance, calibrated meters and measured Live pan-law comparison remain open |
| Audio recording | Planned | Arm input, monitor, record, stop, recover on disk failure |
| Browser | Default-open compact folder/WAV/project browser, current-folder search, original sine shortcut, background scans, collapse/resize, persistent added places and in-window WAV drag-to-Arrangement with conditional floating/locked preview and divider/end/empty drops that create a hybrid track; existing audio/MIDI clips can also move into a newly inserted/appended track; automated checks and native startup/contrast inspected | User new-track insertion/undo/reopen acceptance; browser drag, persistence and clip dragging accepted; preview playback, recursive indexing and outbound OS file dragging |
| Audio assets | Bounded native/background and headless WAV import | WAV import, external assets, missing-file recovery, streaming |
| MIDI | Beat-anchored clips with canonical source/start/local loop settings and finite Arrangement playback/export, note commands/history, strict schema-13 persistence and one-shot schema-9 migration, original sine playback and shared native/offline export; native clip previews, creation, compact all-visible Canvas piano roll with cached geometry, overlap cycling, zoom/fit/pan, body/edge gestures, native-accepted fixed/free grid and Cmd insertion inversion, source-tested modifier gestures, and parameter/history/save/reopen editing; normal/draw modes, Shift/marquee/select-all, atomic group body move/delete and arrow movement/transposition with native saved-value/history/cancellation/minimum-layout checks; bounded continuous drawing and group parameter buttons implemented in source | Native continuous drawing/group parameter acceptance, additive marquee/held-modifier gestures, measured input/paint timing, native cropped/dense loop thumbnails, source controls and loop acceptance, ongoing Session looping, controllers/expression, input/output, recording, broader instrument/device support |
| Instruments/effects | Original sine/FM4 and Operator with four periodic waveforms, independent envelopes, fixed tuning and oscillator velocity; generic versioned state/history; Utility DC/bass-mono filters; original registered eight-band EQ with stereo/LR/M/S banks and a compact graph panel; reusable device controls and fixed-height rail. Operator panel has left oscillator strips, a default amp-envelope center and a Live-inspired right shell. Four fine controls and the ten right-side dials/toggles/selectors are local UI state only; Operator effective feedback uses an original bounded 16-voice mutable runtime; zero effective feedback retains analytic playback | Native visual/listening acceptance and actual supported seek/loop/export acceptance; DSP and saved control integration for the right shell (Pulse2325–2328), live mutable parameter changes on held notes, higher feedback polyphony, remaining Operator behavior and measured reference laws; EQ response comparison/parity; plugin hosting. Runtime handoff rejection preserves applied audio while canonical desired edits may already be saved (Pulse2180) |
| Routing/latency | Acyclic regular-track → Return → Main audio sends with post-track-fader taps; registered Return effects require explicit zero algorithmic latency | Audio/MIDI routes beyond returns, general graph validation, feedback and measured compensation |
| Automation | Bounded canonical track-gain point editing and delivered-quantum touch recording; shared sample-offset playback, strict persistence, loop-normalized single-edit history and compensated origin clocks | Optional optimized native editing/recording/listening checks; general device-parameter automation remains future work |
| Warp/resampling | Versioned control-worker windowed-sinc source-rate conversion | Warp markers, measured time stretching, device-rate conversion |
| Projects | Schema-14 hybrid audio/MIDI, independent Session metadata and generic native-device state persistence, strict schemas 1–13 migration; native Arrangement open/save, create-new Save As; token-checked Save Current with one rolling recovery copy; explicit prior-version CLI recovery; unsaved state and repaired native close guard | Native Session/device save/reopen after UI integration, broader native Save/Save As/relink/recovery acceptance; latest-only untitled and saved-journal source contracts are in [project recovery](project-recovery.md) |
| Editing history | Canonical native metadata undo/redo | Undo/redo all project edits, selection, shortcuts, clipboard |
| Export | Shared streamed CLI and native background PCM16 stereo WAV; native create-new export captures a compiled revision and renders the linear arrangement | Progress/cancellation, overwrite, stems, arbitrary ranges, deterministic export acceptance |
| Plugin hosting | Planned, separate boundary | Host chosen formats, state, latency, failure behavior |
| Live sets | No importer/parity | Minimal own .als fixtures, explicit unsupported-feature reporting |

## Ordered usable milestones

1. Editable audio project: stable IDs, WAV import, compiled playback, basic clip
   editing, project persistence, undo/redo. No more hard-coded media.
2. Recording/mixing: input selection and monitoring, recording workers, track
   creation/deletion, sends/returns, meter calibration, device recovery.
3. MIDI and Session: piano roll, original instrument, per-track quantized launching,
   scenes, capture to Arrangement, MIDI recording and routing.
4. Production: automation, sends, latency compensation, resampling, warp,
   effects, plugin hosting, project exports and crash recovery.
5. Reference compatibility: measured behavior matrix and scoped set/device import.

For every milestone reserve automated tests, device stress, and manual workflow
acceptance. A “blazing fast” claim needs workloads with track/device/clip counts,
sample rate and buffer settings, and comparative results on the same machine.

## Foundation update — 8 October 2026

The canonical audio model now backs the default native editor. Import, edits,
history, media resolution, compilation and save execute on the session worker.
Short native checks exercised WAV import, nudge/trim, playback/Stop and save-new;
the resulting schema matched the expected edits. Cmd-O loading is implemented
and tested headlessly, with missing/mismatched media preserving the existing
session. Subsequent rebuilt native opening passed.

The older four-track/two-scene prototype is available with --demo. Its Session,
tempo and mixer behavior must not be mistaken for canonical editor functionality.
Native visual review found collapsed virtualized-row widths; the width constraints
and subsequent rebuilt imported waveform inspection passed.

Rebuilt native checks also exercised schema-1 opening, repaired full-width
waveforms, 48→44.1 kHz import, repeated duplication, close cancellation and
schema-2 saving. Native keyboard selection, exclusive/additive solo and confirmed
discard subsequently passed. Ruler seeking, one-shot splitting, zoom buttons, pan/fit and schema-3 saving also
passed. Sustained device checks remain
separate; source tests do not imply Live behavior parity.

Session-owned Save Current and create-new Save As, including conflict and
post-publication warning handling, are implemented and covered by workspace and
dependent tests. Cmd-S/Cmd-Shift-S dialog and admission wiring is present in the
native editor source. Fresh native Save/Save As interaction acceptance remains
open; this source coverage does not establish native dialog behavior.

Session organization update — 9 October 2026: atomic scene duplicate/reorder
commands preserve the current MIDI source/note fields, Stop cells and sparse empty
slots. Copies receive independent sources and fresh IDs; reordered scenes retain
IDs and cell contents. Project/history and worker compile/save/reopen/render
acceptance pass. Scene-only compact controls and Cmd-D/Cmd-Alt-Up/Down dispatch
through the existing worker; GPUI tests cover completion, navigation/mode changes,
failure and piano-focus refusal. Native acceptance remains open. Per-note expression, scene tempo/
meter overrides, capture, clipboard and multi-selection remain future work.
Session arrows/Enter and the pinned rightmost scene column pass GPUI test-platform
key/geometry acceptance at 880/1440 px. Native interaction/listening remains open.

Arrangement drag drops now overwrite covered portions of destination audio/MIDI
clips with one compiled history edit. Blank append/empty sections highlight
subtly. MIDI right fragments use finite one-shot projection, with boundary-held
notes retriggered; see `clip-overwrite.md`. Source tests pass; native acceptance
is pending in `artifacts/hex-overwrite-drop-20261009.app`.

Arrangement has an adaptive musical grid and view-only pointer navigation:
mouse wheel zooms at pointer; middle drag pans time and virtualized track rows.
Source-tested (216 UI tests/one ignored), native review pending; see
`arrangement-navigation.md`. Explicit edit snap is unchanged.

Subsequent snapping extension enables adaptive clip snapping by default, sharing
paint and placement spacing. Command temporarily inverts snap during moves and
sample imports, including stationary changes and mouse-up. Blank/zero-track
drops preview a prospective appended row; dividers stay line-only. Source checks
pass (220 UI tests/one ignored); native acceptance remains pending. This is the
documented grid/modifier subset, not full Live magnetic-target parity.


The Arrangement overview now fits all tracks and audio/MIDI clips above the
ruler, with cached compact drawing, viewport outline and click navigation.
See [arrangement-overview.md](arrangement-overview.md). Command preserves grid visibility; alternating
four-unit bands and the native input repair are in this review milestone.
Native visual and physical-input acceptance remain pending.

The unified bottom clip view supports collapse/reopen with retained selection,
Arrangement audio source waveforms and position/length/source/play-start/loop
controls, MIDI source controls alongside the note editor, and existing Session
source panels. It does not yet provide audio clip gain, pitch, warp/time-stretch,
direct numeric entry or draggable detail markers. See [clip-view.md](clip-view.md).
Native visual/interaction acceptance remains pending with the user.
