# App text and typography

All app-owned visible text is lowercase: product name `hex`, labels, menus,
buttons, tooltips, status/errors, units, headings, and accessibility text.
Render strings through `hex-ui`'s `theme::text`; external backend errors and
source labels are normalized at this display boundary. Do not lowercase paths,
IDs, file contents, saved user names, protocol keys, or third-party API identifiers.
Technical documentation and attribution preserve proper source names.

All app-rendered text uses **Ioskeley Mono**, sourced from
[ahatem/IoskeleyMono](https://github.com/ahatem/IoskeleyMono), release v2.1.0.
Normal-width unhinted regular/medium/bold/italic files are embedded in the binary.
The internal font family is exactly `Ioskeley Mono`; this identifier is not a
visible uppercase label. Registration and family availability are checked at
startup; no silent switch to a system font is allowed if loading fails.
Retain the OFL notice and attribution when packaging or updating the assets.

The root view inherits `theme::FONT_FAMILY`. Do not introduce local font families
or depend on system-installed fonts. Native title text is hidden; `hex` is drawn
in the app header. macOS-owned menu chrome, permission dialogs, and system file
pickers are rendered by the OS, outside the app's typography control.

Run `python3 scripts/check-ui-style.py` after changing app text or styling.
This source guard checks text-child routing, font selectors, and bundle names;
it is not a substitute for inspecting rendered glyphs or future custom renderers.
New widgets must preserve the text helper/font policy rather than bypass it.

## Compact workspace

Keep Ableton Live's tight, compact density with minimal dead space. Prefer small
controls, dense rows, fine dividers, small gutters and contiguous panels. Give the
window to musical content: tracks, clips, waveforms, meters and editable device
parameters. Avoid oversized headers, decorative card padding and empty panel
interiors. Preserve readability, focus visibility and usable interaction targets;
compactness does not justify clipping text. Panels should collapse and resize,
with the timeline/mixer consuming remaining space.

The current accepted visual implementation baseline is
[UI iteration 3](../design/frame/iterations/03/README.md), Frame r70, approved
8 October 2026. It preserves the compact Live composition with warm charcoal
surfaces, primary orange #f06449, softer secondary cyan #70e8e3, soft violet
#9994bd and clearer blue #719cce. Concept clips use harmonious gray fills.
Six saved views cover Arrangement stopped/playing, Session stopped/playing,
the custom clip-color picker and Arrangement time selection. Solid meters have
orange upper levels and red peaks. Prototype meters and selection are visual
states, not evidence of native behavior. User-swatch editing/persistence remains
separate work. Use the snapshot's exact tokens and geometry when implementing;
verify actual capabilities and the rendered native window. Iteration 2 is retained.

## Native iteration

The native window follows the reviewed Frame direction while exposing only the
implemented canonical audio workflows. The generated four-track/two-scene
prototype remains separate behind --demo. Prefer small icon
actions with lowercase descriptive tooltips and shortcut hints, flat track/mixer
geometry, a collapsible narrow browser and a compact inspector. Live guides
composition; Yield guides palette/type. The timeline should use remaining space
for a continuous grid rather than decorative cards or oversized headers.

The canonical editor uses a 38 px transport, 24 px ruler, 44 px virtualized track
rows, 176 px right-side headers and a 22 px status strip. Track names truncate
to one line with full lowercase tooltips. The timeline must receive the remaining
row width explicitly; intrinsic header measurement must never hide imported clips.
Cached peak summaries preserve extrema when grouped for display.

Frame r51 uses four clip families with seven named brightness and saturation variants and reserves separate bright success,
warning and error tokens. This is design-only; native status behavior is unchanged.

The current canonical headers keep 44 px row height and 176 px width: a bounded
name line above compact gain/mute/solo controls. Up/Down navigates tracks without
wrapping and reveals offscreen selections. Viewport toolbar controls and ruler
use the same project-frame interval; offscreen clips are culled. Cached waveform
extrema follow visible source ranges, including loop wraps and exhausted tails.
They do not provide sample-level accuracy at extreme zoom. Native checks passed
name visibility, zoom button, keyboard pan/fit and seek/split.

Frame r61 uses the supplied Yield reference colors: deeper charcoal, off-white
waveforms/text and red-orange controls. Secondary accents are quiet.

Frame r61 brightens background layers slightly and adds subtle red warmth, with
darker dividers; accent/text/clip colors and layout remain unchanged.

Frame r61: orange primary, vivid green secondary; Yield-style library selection;
slightly brighter warm charcoal and darker tag rectangles.

Frame r63 hierarchy: orange primary, luminous green secondary, raspberry tertiary.
Warm backgrounds and Yield library selection styling remain.

Frame r63 nudges the secondary accent toward cyan-blue; other colors stay exact.

The native transport includes a compact export icon with the lowercase tooltip
`export wav · cmd+shift+e`. It uses native create-new path selection and reports
completion separately from the current project revision. Export progress and
cancellation controls do not exist; do not imply that a pending job is complete.

Frame r64 adds state-based simulated meter fills and peak markers in both views.
Cyan-green segments and yellow near-peak signals preserve the theme; stopped
states clear the fills. This is illustrative design playback, not live metering.

Frame r65 replaces meter segments with opaque continuous bars, with orange
above 80% of the illustrative well and red above 94%. These thresholds describe
the prototype artwork, not calibrated dB values.

Frame r66 uses ice blue #79aebc instead of strawberry for tertiary details,
and secondary cyan #00e8df. Orange and warm charcoal remain unchanged.

Frame r68 adds an Arrangement time-range selection study with translucent cyan
fill, aligned ruler highlight and start/end edges. Selection is a visual state;
native drag, time bounds and editing contracts are unchanged.

Frame r69 softens the secondary cyan to #35e8e1. Selection tint, meters and
focus share this token; other theme colors and layout remain unchanged.

Frame r70 further softens secondary cyan to #70e8e3; other tokens are unchanged.

Native transparent macOS titlebars reserve 80 logical pixels at the leading edge
of the transport row for OS traffic lights and nearby window chrome. The canonical
editor and demo share this theme constant; other panels retain compact padding.
Keep native close/minimize/fullscreen behavior. Do not paint app controls beneath
window controls or build duplicate traffic lights merely to avoid the inset.

The canonical transport includes a compact `⟳` action with lowercase tooltip
`restart default audio output · stays stopped`. It requests the real default output
and reports pending/ready/failure state; it does not imply device selection or
automatic reconnection. Keep edits and saves available while audio is offline.
Source checks and a rebuilt bundle pass. Fresh native inspection confirms the
reserved titlebar area and compact mixer; track balance, main gain, toolbar undo
and a stopped default-output restart work in a one-track empty-audio project.
The OS screen-sharing pill occupied the traffic-light area, so normal window-control
interaction/fullscreen sizing remain open. Listening and physical device failure
recovery remain separate acceptance. Toolbar action icons dispatch through their
own Window, avoiding dependency on the globally active window during background
inspection or activation changes.

Selected-clip fades occupy the existing 26 px mixer strip. Small minus/plus and
reset controls use lowercase tooltips; compact millisecond/second readouts expose
exact frame counts on hover. Busy controls dim and explain deferred admission.
Inherited split envelopes show a small `split` indicator and explain reanchoring.
The thin cyan envelope is clipped to visible timeline content, with at most
32 regular samples and four exact original-domain breakpoints; neutral and narrow
clips skip the overlay. Native inspection at 1240 × 760 confirmed the readouts,
inherited indicator and changed envelope without adding another panel. Minimum
window sizing, dense clips and listening remain separate acceptance.

The compact 205px piano roll uses a 38px labeled pitch strip and one shared body
height for keys, grid and notes. MIDI 60 is c3, matching
[Ableton's Live12 release notes](https://www.ableton.com/en/release-notes/live-12/).
The p/t/g/v readout denotes pitch, clip-local onset and gate in quarter-note beats,
and velocity. Icon controls have lowercase tooltips; click grid to insert a
quarter-beat note, click note to select. Native alignment and complete selected
readout were checked at 1240×760; minimum-window and dense interaction are still
open. At most 512 visible notes are drawn and a density notice exposes omitted
geometry. This is an interim bounded editor, not full dense-note virtualization.

Note bodies now drag time/pitch; visible true boundaries have small 6px handles
and resize cursors. Cropped viewport boundaries remain body targets. Motion
previews one note locally, release inside the roll commits one edit, and release
outside cancels. Native normal/short-note move, both resizes and undo were checked
at 1240×760. Keep the note absolutely positioned; adding relative positioning
would override its pitch/time coordinates. Held-Escape/dense/minimum gates remain.

The native transport output status must fit its remaining width and ellipsize
as one line when constrained; keep its full device/rate/channel description in
a themed lowercase tooltip. In GPUI 0.2.2 use the verified one-line clamp path
for this flexible text instead of no-wrap truncation, which kept an intrinsic
measurement during layout. Native default and 880 × 540 checks pass.

Session composition uses track columns followed by the scene launch column at
the right edge, consistent with the [Live Session manual](https://www.ableton.com/en/live-manual/12/session-view/).
Keep scene names/select and small launch icons in that compact fixed column,
aligned between header and rows. Keyboard neighbors include this scene column:
Right from the last track selects the scene, Left returns to the last track,
and outer edges clamp. Up/down keep scene-only selection in the scene column.
Enter launches selected clips/scenes through the normal accepted-graph gates;
piano-roll focus retains note editing. Source and GPUI test-platform action
dispatch/geometry acceptance pass, including 880 and 1440 logical-pixel widths.
Native minimum-size/layout interaction remains pending.

Scene-only selection exposes small duplicate/move-up/move-down icons in the
existing toolbar, with lowercase tooltips and Cmd-D/Cmd-Alt-Up/Down shortcuts.
Reuse canonical command admission and fixed scene-column geometry; do not add
a panel or labeled action row. Pending worker edits must not freeze navigation.
If the user changes selection or mode while a copy is being created, its
completion preserves the newer selection. Reordering retains the stable scene
ID rather than selecting by its old row number. GPUI worker/key regressions pass;
native layout/input acceptance remains open.

## Canonical left browser

The default editor reserves a compact left browser below the full-width transport
and above the status bar, beside either Arrangement or Session. Start open at
280 logical pixels; use 22-pixel result rows, small navigation icons, a narrow
resize handle and a collapsed rail. Reserve useful workspace width when resizing.
Ruler and Session geometry must use the remaining workspace width.

Display samples, available instruments and projects truthfully. WAV sample and
project labels render lowercase through `theme::text`, while original paths keep
their filesystem casing. The search field uses the bundled font and native text
input; its focus must isolate typing/editing from transport and project commands.
Folder navigation and current-folder filtering are the initial usable scope;
recursive indexing, preview playback, browser-to-timeline drag and persisted
places require separate implementation and acceptance.


## Arrangement landing preview — 9 October 2026

Audio and MIDI dragging uses a compact cyan in-track landing rectangle, clipped
to the viewport and resolved to the actual snapped destination. The next
user-selected browser-drag update restores a floating cursor preview outside
lanes; inside lanes, only the locked landing preview is shown. Keep
source kinds in their canonical frame/beat units. See clip-drag.md for the
interaction contract and remaining native user checks.


## arrangement grid and navigation

The adaptive grid is a subtle continuous timeline background, excluding track
headers and aligned to musical ruler positions. Stronger bar/meter boundaries
provide context; subdivisions should remain legible without competing with
clips and waveforms. Keep zoom/navigation directly on the timeline rather than
adding large controls or extra panels. See `arrangement-navigation.md`.


The Arrangement overview now fits all tracks and audio/MIDI clips above the
ruler, with cached compact drawing, viewport outline and click navigation.
See [arrangement-overview.md](arrangement-overview.md). Command preserves grid visibility; alternating
four-unit bands and the native input repair are in this review milestone.
Native visual and physical-input acceptance remain pending.


## minimal append feedback — 9 October 2026

The user superseded empty-area background highlighting after accepting clip
snapping. Remove the full-area tint for browser samples and existing audio/MIDI
clips. Retain the bottom line of the last track and the snapped clip preview.
With no tracks, show only the prospective clip preview; there is no last-track
line. Preserve grid shading, snapping and drop destinations. Native review
remains the user's responsibility.


Audio and MIDI Arrangement clips now use compact bottom-corner resize targets
while retaining the upper clip body for movement. Resizing paints the proposed
clip and waveform/note geometry immediately, uses the selected grid and allows
Command free movement. One canonical edit is submitted at release. See
[clip-resizing.md](clip-resizing.md); native acceptance remains pending.


## invisible full-height clip edges — 9 October 2026

The user superseded bottom-corner resize handles. Audio and MIDI clips now use
invisible full-height left/right edge strips with the native horizontal resize
cursor. Remove orange rectangles; derive narrow hit widths from rendered clip
width while preserving a central full-height move region. Live previews, grid
snapping, Command override and canonical release semantics remain unchanged.
Native top/middle/bottom edge hover and resizing acceptance remains pending.


The latest Arrangement style follows the supplied Live screenshot: flush
full-track clip height, a one-pixel black border, warm fills with dark signed
channel waveforms, and aligned grid lines through the clip. macOS resize edges
use original bracket cursors retained by the existing external GPUI boundary.
Waveform cache/query fidelity and open native timing checks are recorded in
[waveform-display.md](waveform-display.md).
