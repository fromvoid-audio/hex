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

the lower detail area follows the selected clip or device. selecting another
track can change the available controls. a pending edit or load may need to
finish before another document operation is available; check the displayed status
instead of repeatedly clicking a busy action.

in console, select the intended clip or scene before launching it. **return**
launches the selected console item when that workspace has focus. check the
transport and audible result; a selected item is not evidence that it is playing.

# find, preview and import audio

open the browser with **command+option+b**. press **command+f** to search.
use the browser's categories and locations to narrow the results. when a search
is entered, click **×** inside the field to clear it and type a replacement.
clearing keeps the selected category or location.

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

# midi and sampler

press **command+shift+t** to add a track for notes. select the intended track and use
**command+shift+m** to create a midi clip. select the clip to edit its notes in the
piano roll. note position, length and pitch belong to the clip; selecting a device
is a different operation.

load a sample into the sampler on a selected midi track. sample loading requires
the supported project/track state. replacing the current instrument is currently
limited to the sampler's classic mode. check the selected track and loading status
before making another replacement. use the sampler controls to adjust the loaded
sample's bounds, pitch, level, loop and envelope where available.

after loading, enter a few notes, play the clip and confirm that it triggers the
intended sample. test undo/redo and save/reopen on a copy before relying on a new
sampler edit in an important project.

controller input can support pressure, pitch bend, modulation and held-note
piano-roll step entry. the current controller-input path does not provide live
note monitoring, sustain, recording or mpe. do not treat the presence of an input
or keyboard control as a complete live-performance workflow.

# instruments and effects

select a track and its device to show the corresponding controls. the set of
available controls depends on the device. use the visible parameter labels and
readouts, and check the audio after changing a sound. a device that looks familiar
does not imply complete compatibility with another application's device.

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

# useful shortcuts

| action | shortcut |
| --- | --- |
| new project | command+n |
| open project | command+o |
| save / save as | command+s / command+shift+s |
| play or pause / stop | space / escape |
| import audio | command+i |
| add track | command+t |
| add track for notes / midi clip | command+shift+t / command+shift+m |
| undo / redo | command+z / command+shift+z |
| toggle browser / search | command+option+b / command+f |
| settings | command+, |
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
