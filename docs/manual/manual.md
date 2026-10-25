hex is a native music application for arranging audio, editing midi and working
with instruments and effects. this guide covers the workflows available in the
packaged beta. hex is active alpha software: a beta package identifies a frozen
build; it does not mean every workflow or device has completed native testing.

# get started

1. open hex and choose **file → new project**. use a new project or a copy of
   existing work while evaluating a beta.
2. check the audio status in the toolbar. hex uses your system's default audio
   output. choose the desired output in macos before opening hex.
3. press **command+i** to import audio, or open the browser with
   **command+option+b**. choose an audio file and place it in an audio track.
4. press **space** to play or pause. press **escape** to stop. adjust track level
   carefully when trying unfamiliar source material or devices.
5. choose **file → save as…** and save under a new name. close hex normally, then
   reopen that saved project to check the result.

shortcuts apply when the relevant workspace has focus. a text field, browser
search, dialog or device editor can handle the same key differently.

# install and update

the macos package includes `hex.app`, this manual in pdf and offline html,
`RELEASE-NOTES.md`, and `TESTING.md`. read the release notes and checklist for the
specific version before testing it. the html manual works without an internet
connection; ordinary external links still require one.

quit hex normally before replacing the app. keep one installation at its usual
location. replacing an app on disk does not update an already running process.
keep projects, media and preferences when updating; they are separate from the
app bundle. do not delete user data to resolve an installation problem.

for repository-managed testing, use the documented release installer to verify
and install the package at the configured development-app path. for a first
manual installation, choose a permanent location for `hex.app`; subsequent updates
replace that same copy. a package intended for distribution is signed and
notarized. if macos refuses it, preserve the error and obtain a verified package
from its supplier rather than changing system security settings.

the current release command produces macos arm64 packages. it does not establish
intel or windows support. automatic updates and scheduled nightly installation
are not provided by this packaging system.

# projects and recovery

**file → new project** starts a project. **file → open project…** opens a saved
project. **recent projects** provides previously used paths. if current work has
unsaved changes, finish the save or confirmation dialog before replacing it.

**save** updates the current project. **save as…** chooses another destination.
when hex warns that saving a migrated project will make it unreadable by older
readers, choose **save as…** to keep the original. only choose overwrite when you
intend to replace that original. supported format migration happens while opening;
opening alone does not authorize rewriting the original project.

keep the original project and its media if opening fails. an unsupported or
malformed format needs its reported recovery path; changing the filename does
not convert it. compatibility with every historical alpha file is not implied.

## missing media

use **file → open and relink media…** when audio files have moved. follow the
**locate missing media** prompt and select the corresponding original recording.
check the loaded clips before saving a new copy. do not replace a missing file
with unrelated audio merely to dismiss the prompt.

## recover work

the file menu offers three recovery actions:

- **recover latest untitled project…** looks for recoverable work that had not
  been given a project destination.
- **recover saved project edits…** restores available edits associated with a
  saved project.
- **recover previous save…** opens an available earlier saved revision.

recovery depends on the files that remain available. preserve the original and
any recovery files if an operation fails. inspect the recovered project, then
use **save as…** to keep a separate copy before continuing.

# arrange and console

use the header controls to switch between **arrange view** and **console view**.
arrange places clips along a time ruler. console provides the clip/scene workspace.
these views share the project; switching views does not create another document.
tracks can contain both audio and midi clips under the same mixer controls.

in arrange, select a track or clip before editing it. drag a clip to move it and
use its edges to adjust its extent. work at a zoom level where the intended edge
and grid position are clear. use **command+z** to undo and
**command+shift+z** to redo admitted edits.

with the computer midi keyboard off, select two or more clips on one arrange
track and press **g** to group them. select only audio clips or only midi clips.
hex keeps gaps and refuses a group that would cover unselected material. midi
clips become one finite midi clip with their played notes and volume curves.
audio clips become events inside one clip; original files and source processing
remain retained. grouped audio keeps its source timing at the grouping tempo.
use undo to restore the original clips in one step. grouping requires resident
audio; unsupported streamed sources refuse before the edit is committed.

with a positive arrange time selection, **g** instead groups only the included
pieces, splitting clips at both selection edges and preserving outside material.
each populated track gets its own clip, preserving track routing; empty tracks
are skipped. each track must contain one media kind with matching activation.
the final track's result becomes the selected clip. one undo restores all tracks.

in the lower audio editor, **g** makes the selected event or time span its own
clip and keeps the material on either side. select one visible, nonrepeating
span. text fields, device controls and the piano editor keep their own key input;
when the computer midi keyboard is enabled, **g** plays its note.

changing the project bpm keeps clips at their musical start positions. audio with
warp off keeps its natural playback speed. if a faster bpm makes an audio tail
reach the next clip on that track, hex crops that tail at the next start. original
media remains available, and undo restores the previous tempo and clip extents.

with the computer midi keyboard off, **l** or **w** sets and enables a loop around
selected clips, a time selection, or the selected main sections. pressing again
with the same bounds toggles looping off or on, retaining the range. with no
selection, these keys toggle the existing loop. **command+l** uses the same action.
for multiple sections, the loop spans the earliest start to the latest end,
including gaps. while the computer midi keyboard is on, plain **l** and **w** play notes.

drag a section header to move its musical content. hold **option** before or
during the drag to adjust only its marker; the music stays in place. option-resize
an edge to trim or remove overlapping section markers. grid snapping remains
active when enabled; **command** temporarily inverts it. after entering option
adjustment, releasing option keeps that drag marker-only. release outside the
section lane to accept the last valid preview, or press **escape** to cancel.

turn on **section autoplay and loop** in the top toolbar, then click the body of
an arrange section to play it repeatedly. the launch dropdown beside it also
sets quantization for console clip and scene launches: **none**, **8 bars**,
**4 bars**, **2 bars**, **1 bar**, **1/2**, **1/2t**, **1/4**, **1/4t**, **1/8**,
**1/8t**, **1/16**, **1/16t**, or **1/32**. **t** means triplet timing.

current playback continues while hex prepares the section. the prepared section
starts at the next selected boundary after it is ready; **none** starts as soon
as it is ready. a stopped transport starts immediately. clicking another section
replaces the pending launch. stop or turning autoplay off cancels a pending
launch; turning autoplay off leaves current music playing.

this section loop is temporary and does not change the saved project loop or
undo history. a project edit, normal loop change, or other replacement of the
playback graph restores the project's loop. autoplay stays armed for the next
section click. modifier clicks, renaming and dragging keep their editing actions.
at narrow window widths the top toolbar uses two compact lines to keep every
control, the full view name and audio status visible.
section playback currently requires resident audio mode and an applied arrange
playback graph. verified streaming mode is unavailable for this control. a section
past the bounded arrangement preroll range, or a source that exceeds existing
runtime storage or midi density limits, is refused while current playback continues.

a time selection shows a dark dot when its musical length is a power of two,
such as two or four beats. the dot follows the dragged top-right or top-left
corner and stays visible after release.

with the arrange timeline focused, **b** bounces selected material in place;
**command+b** keeps the originals and inserts bounced audio on a new track below
each source track. a nonempty time selection takes precedence over selected clips.
each selected track produces one audio clip spanning its selected clips and gaps.
clip bounces include instrument and effect release tails. a time selection renders
exactly its start-to-end interval, retaining earlier state needed for sound inside
that interval. press **escape** to cancel; one undo restores the entire operation.

stop playback and disable live midi input or audition before bouncing in place. hex keeps the stopped cursor and installs
a fresh playback state so live release voices cannot duplicate the baked tails.
undo and redo of an in-place bounce use the same stopped-edit rule.

bounce writes float audio before the track mixer. mixer automation, sends, returns
and main processing remain live. in-place bounce removes the baked devices, so it
refuses if that would change unselected material or a tail would overlap it; use
**command+b** instead. routed inputs, verified streaming and nonzero-latency device
paths currently refuse. a release that does not finish within ten seconds refuses
without applying an edit. source files remain unchanged. while editing text,
numeric values or midi notes, or using the computer midi keyboard, these arrange
bounce shortcuts do not run.

right-click a clip to switch its visual flavor. **use accent flavor** gives the
track's color a saturated fill; **use muted flavor** uses a darker, more saturated fill. selection
keeps its own outline in either flavor.

shift-click ordinary track headers to select a range; command-click toggles
individual members. drag any selected track name to reorder the selection as
one block, keeping its order and content. dragging an unselected name moves
that track alone. one undo restores the previous order.

with an arrange track header focused, **command+c** copies all selected tracks,
**command+x** cuts them, **command+v** pastes after the selected block, and
**command+d** duplicates it. empty tracks are included, and sparse selections
keep their project order. each cut, paste or duplicate has one undo step.
right-click a track header for these actions or **duplicate tracks without clips**:
this keeps the mixer, gain automation and complete device chain, including a/b
settings, while omitting both arrange and console clips. full copies include both
clip workspaces. original media stays shared; copied tracks and devices have new
identities. internal routes follow the copied group, and external inputs remain
connected. duplication preserves outside tracks' existing midi source choices.
track and clip/time clipboards are separate and clear when another project opens.


drag the bottom edge of a track header to resize that row. hold **option** while
resizing to give all ordinary tracks the same height. at the minimum height, rows
keep their names and clip titles; expand them to use the mixer and fade controls.
with the computer midi keyboard off, press **h** in arrange to toggle a compact
overview. press **h** again to restore each track's previous height. resizing in
the overview is temporary; it does not replace normal heights. these view sizes
last for the current editor session. return and main rows retain their own size.
with the computer midi keyboard on, **h** continues to play its note.

right-click a track header or the track area and choose **insert track** or
**insert return track**. return buses stay pinned above **main** in arrange;
their headers provide volume, activation and removal controls.

track volume uses the same numeric fader in resident and verified-streaming
projects. dragging previews the absolute gain while the project stays unchanged;
release commits one undo step and **escape** restores the current project gain.
automation owns an automated track's gain. held pan and return-gain previews
remain unavailable in verified mode.

the lower detail area follows the selected clip or device. selecting another
track can change the available controls. a pending edit or load may need to
finish before another document operation is available; check the displayed status
instead of repeatedly clicking a busy action.

in console, select the intended clip or scene before launching it. **return**
launches the selected console item when that workspace has focus. check the
transport and audible result; a selected item is not evidence that it is playing.

## arrangement automation view

press **a** or use the toolbar's **a** control to show track-gain automation over
faded clips. repeat to return to clip editing. switching views preserves clips,
selection, automation and undo history. **c** and **0** toggle contextual
activation outside automation view: the focused device, track, selected clips,
or selected time span. a partial time span splits clips at its edges. text fields
and the computer midi keyboard keep their own input.

click a gain curve to add a point, drag a point to change its time or value, and
release to apply the edit. **command** gives free beat placement. **escape**
cancels a held edit; **backspace** removes the selected automation point.
one undo restores a completed change. background clip edits are unavailable
while the automation lanes own arrangement focus.

this view currently shows track gain. recorded touch spans are displayed but
read-only: leave automation view and use the track's gain editor to clear the
lane before editing its base curve. device-parameter lanes are not offered here.

## clip loop resizing

hover the lower part of a clip's right edge to show the loop cursor, then drag
to repeat its source. the rest of the edge keeps ordinary trimming. audio and
midi clips use the same gesture; no loop icon is painted inside the clip. looped
clips have rounded bottom corners. drag the first repeat boundary to change the
cycle while keeping the outer duration; directed clips retain only the outer
repeat handle. **escape** cancels a held edit; one undo restores a completed drag.

## audio event fades

select an audio clip and choose **events** in its detail area. focus the event
waveform and hold **f** to show its fade controls. release **f** to hide default
controls; edited fades remain visible. the computer midi keyboard must be off.

drag an upper square to change a fade length. drag a diamond horizontally and
vertically to shape its curve. lower squares trim the event; they move freely by
default, with **command** opting into the event grid. double-click a fade control
to reset that event's fades. one undo restores each completed gesture.

the preview becomes prepared audio after release. **escape** or leaving the
editor cancels a held gesture. splitting retains the source fade, moving keeps
it attached to the sound, and an explicit trim fits it to the new event window.
event fades use the same supported resident clips as event editing; a refused
preparation leaves the project unchanged. original media files stay intact.

# find, preview and import audio

open the browser with **command+option+b**. press **command+f** to search.
use the browser's categories and locations to narrow the results. when a search
is entered, click **×** inside the field to clear it and type a replacement.
clearing keeps the selected category or location.

drag a folder from the browser results onto **places** to bookmark it. the
folder stays in its original location; bookmarking keeps the current browser
source and search. dragging the same folder again preserves its existing place
and icon. press **escape** or drop outside places to cancel.

the preview controls audition a selected audio file. the waveform can be used to
choose a preview position. stop preview playback before judging the main project's
output. auditioning a file and inserting it into the project are separate actions.

with browser focus, **up/down** moves through results, **right** previews the
selected result, and **left** collapses a folder. **return** activates the selected
item. the action depends on the result type and the current destination: check
the selected track and arrange cursor before inserting audio or loading a device.
do not assume double-clicking a sample inserts it into a track.

**command+i** opens audio import. imported material retains its own sample rate
and channel information; check duration, placement and playback after importing.
keep source recordings available with the project.

as projects grow, ordinary audio clips can play directly from their original
files while retaining their complete duration. single-file and bulk imports keep
this behavior; a bulk drop remains one undo step. keep those files available for
playback, save/reopen and export.

file-backed clips support placement, playback, clip gain and ordinary device
processing. processing that materializes audio, including pitch,
warp, directed content, source envelopes, finite repeat edits and grouping,
currently requires resident source audio. an unsupported edit refuses before changing the project or history.
opening a project keeps its supported resident processing alongside plain
file-backed clips.

# midi and sampler

press **command+shift+t** to add a track for notes. select the intended track and use
**command+shift+m** to create a midi clip. select the clip to edit its notes in the
piano roll. note position, length and pitch belong to the clip; selecting a device
is a different operation.

choose **envelopes** beside the piano roll to edit **clip volume**, in arrange or
console. click an empty place to add a point, drag a point to change its source
beat and gain, or press **backspace** to remove the selected point. the graph uses
linear gain: **0** is silence, **1** is unity and **4** is the maximum. straight
lines interpolate between points. the grid button controls snapping; hold
**command** to invert snapping or **shift** for a fine drag. **escape** or losing
window focus cancels a held edit. releasing commits one undo step, with the final
preview retained while the edit finishes. the reset control restores unity.
choose **notes** to return to note editing. these points belong to the clip and
are retained by save, reopen, undo and redo.

in arrange, drag a midi clip edge normally to trim its visible region. hold
**shift** while dragging either edge to stretch its notes: note positions and
lengths scale together, while pitch and velocity stay the same. the opposite edge
stays fixed. **command** temporarily changes grid snapping; shift can be pressed
or released during the drag. **escape** cancels the gesture. the note preview stays
visible until the edit finishes; undo restores the whole stretch in one step.

stretching also scales retained source offsets, loop markers and clip-volume
points. extremely small intervals that cannot retain a positive duration are
refused with the original clip preserved. per-note expression is not currently
stored by hex.

load a sample into the sampler on a selected midi track. sample loading requires
the supported project/track state. replacing the current instrument is currently
limited to the sampler's classic mode. check the selected track and loading status
before making another replacement. use the sampler controls to adjust the loaded
sample's bounds, pitch, level, loop and envelope where available.

after loading, enter a few notes, play the clip and confirm that it triggers the
intended sample. test undo/redo and save/reopen on a copy before relying on a new
sampler edit in an important project.

in settings, choose a midi input device and channel, then select a track with an
enabled instrument. the midi control in the status bar opens the same device and
channel choices. use refresh to discover or reconnect devices. hex remembers the
chosen device; an unavailable saved port never selects another device automatically.
choose off to disconnect. midi changes apply immediately; a save error means the
change has not been retained for the next launch.

hardware notes play the selected instrument with their original note-on velocity.
computer-keyboard notes use the same selected track and keep separate ownership.
selection audition waits while hardware input is selected. pressure, pitch bend,
modulation and held-note piano-roll step entry remain available. live input timing
is currently resolved at audio-block boundaries; sustain and mpe are not included.

## input quantization

use **in off ▾** in the transport to choose off, quarter notes (1/4), eighth notes
(1/8), sixteenth notes (1/16), or adaptive. the same setting controls recorded midi
note starts and browser sample insertion with enter. the browser's grid button
switches between off and adaptive; choose a fixed subdivision in the transport.

fixed subdivisions work independently of the arrangement snap toggle. adaptive
uses the arrangement's visible grid and density, so enable that grid first.
placement uses the nearest grid point, including backwards; exact ties move later.

recording captures the grid when you click record, before preparation. zoom,
density and later input changes cannot alter the retained take. finish or discard
the take before changing its input setting. monitoring remains immediate at the
recorder's stated next-block timing; quantization moves authored note starts when
the take commits. played musical note lengths are preserved, with cropping at the
clip end and a one-output-frame minimum for notes moved across tempo changes.
a final-frame note retains a valid output interval. off preserves original timing.
the take is still one undo/redo edit.

## record midi notes

select an active midi clip in arrange with an enabled instrument. enable the
computer midi keyboard or configure a hardware input, then start playback inside
the clip. click **●** in the transport to prepare and start midi recording. wait
for the recording status before performing. preparation releases existing monitored
notes while it installs the recording input. newly played notes are captured;
notes already held before recording are not added to the take.

click **●** again to finish while playback continues, or press **escape** to stop.
held notes end at the last delivered recording boundary. recording also finishes
at the clip end. the completed take becomes one undoable edit; undo and redo
restore the whole take. timing follows the next audio block, without arrival-time
or device-latency compensation.

keep the selected clip, input and instrument unchanged during the take. a refused
commit retains the take: click **●** to retry. **×** explicitly discards a retained
take. an interrupted take is marked incomplete and is never committed silently.
finish or explicitly discard it before opening, creating or closing a project.

this recorder supports finite nonlooping arrangement midi clips on resident
native output. disable the project loop before recording. console, streamed
projects, audio recording, loop overdub and per-note expression recording are not
available through this control.

# instruments and effects

select a track and its device to show the corresponding controls. the set of
available controls depends on the device. use the visible parameter labels and
readouts, and check the audio after changing a sound. drag an effect from the
browser onto a track header in arrange or console to append it to that track.
header drops accept an instrument only when its device chain is empty; an
occupied instrument slot is preserved. a device that looks familiar
does not imply complete compatibility with another application's device.

click a device header to focus its card, then press **0** to toggle its bypass.
this uses the same state as the header bypass control and supports undo/redo.
while a parameter field or browser search has focus, **0** remains text input.

in the eq graph, single-click empty space to create a band. the left and right
edges create low-cut and high-cut bands; the bottom interior creates a notch;
other empty space creates a bell. at the bottom corners, the edge cut takes
priority. a small curve beside the crosshair shows the shape before pressing.
keep the button held to position the new band and hear its effect during playback.
release to keep the band as one undoable edit. vertical movement changes q for a
cut or notch, or the gain of a bell. existing
nodes keep their own selection and drag targets. press **escape** to cancel.

each visible eq has its own live spectrum, including eqs on return and main
tracks. selecting another device keeps these spectra running. use **spectrum**
on an eq to turn only its analyzer off or on. a hidden device panel stops its
analyzer; missing or stale audio clears the live curve.

the beta release command packages the default desktop application. optional
native vst3 helper profiles are not included by that command, and this manual
does not promise a complete third-party plugin-hosting workflow.

# audio output and settings

hex opens the system's default output device with that device's default
configuration. **settings…** is available from the hex menu or **command+,**;
it does not currently provide an audio device, sample-rate or buffer-size chooser.

if the toolbar reports unavailable output, check the default output and device
connection in macos. the toolbar offers **restart default audio output · stays
stopped**. restart does not start the transport; confirm the status before playing.

if there is no sound, stop playback and check the source clip, selected track,
track level/solo state and system output. preserve the displayed error if the
device cannot open. an offline export succeeding does not prove that live output
or the physical device is working correctly.

# export audio

select the intended time range and press **command+shift+e**. the **export output**
dialog offers **main mix**, **selected tracks**, **shared returns**, and
**selected stems and shared returns**. select the tracks needed for a stem export
and choose a new destination in the native file dialog.

exports are wav files. main mix produces the combined output; the stem choices
keep the selected outputs separate. check the range and output choice before
exporting, then audition the resulting files. offline rendering can be available
even when live audio output is unavailable.

file-backed audio in ordinary and verified streaming projects uses offline
media verification during export. keep original media available until export completes. hex verifies those files
before reading and again before publishing; missing or changed media, unavailable
pages and cancellation leave the previous destination intact. choose an export
path separate from the original media. editing or opening another document does
not change an export already in progress.

# reversing clips

select one or more audio or midi clips in arrangement, then press `r` to reverse
what is audible inside each clip. press `r` again to restore its original direction.
one undo restores the whole selected group. text fields keep their normal typing behavior.

original audio files and midi notes remain intact. trim, loop, pitch, source gain
and warp settings stay editable. frame-based audio reverses in frames; musical
audio and midi reverse in the clip's beat clock, so tempo changes can change the
sound of musical audio. fades keep their timeline position.

directed midi clips cannot yet be transferred to session, and midi recording requires
a forward clip. directed clips retain the outer repeat handle; dragging their
first repeat seam is not available. for directed clips with fades, loop resizing
updates the waveform after the edit is accepted. an unsupported preparation refuses
the complete edit.

# useful shortcuts

| action | shortcut |
| --- | --- |
| reverse selected arrangement clips | `r` |
| new project | command+n |
| open project | command+o |
| save / save as | command+s / command+shift+s |
| play or pause / stop | space / escape |
| import audio | command+i |
| add track | command+t |
| add track for notes / midi clip | command+shift+t / command+shift+m |
| undo / redo | command+z / command+shift+z |
| toggle browser / search | command+option+b / command+f |
| compact track overview (computer midi keyboard off) | h |
| settings | command+, |
| bounce in place / to a new track | b / command+b |
| arrangement automation view | a / toolbar a control |
| contextual activation | c / 0 |
| export | command+shift+e |
| changelog | command+shift+l |

# report a beta problem

include the full version and source shown for the package, the macos version and
audio device, the steps that reproduce the problem, and what you expected to
happen. include the displayed error and whether the issue survives a normal quit
and reopen. keep original projects and media; share only material you have the
right to share.

`TESTING.md` is the finite checklist for this specific package. record actual
results against that version. passing automated checks does not replace native
interaction, listening, controller/device tests or physical audio deadline checks.
