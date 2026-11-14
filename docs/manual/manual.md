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

space starts from the beginning of the time selection, or from the arrange edit
cursor when there is no time selection. effects start with fresh tails. native
mutable instruments retrigger notes whose gates cross the cursor and keep their
original note-off times; earlier voice history is reset. starting does not replay
the song before the cursor. section launches and exact-history seeks have their
own preparation behavior.

normal pause and stop fade the output briefly. seeking during playback fades
out at the old position and in at the new one. pause releases held arrangement
and session instrument notes. when resuming from that position, the instrument
plays the next note starts without replaying those held notes.

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

on macos, saving over a project can remove custom access rules on that file.
use **file → save as…** to keep the original and check the new file's access
rules before sharing it.

keep the original project and its media if opening fails. an unsupported or
malformed format needs its reported recovery path; changing the filename does
not convert it. compatibility with every historical alpha file is not implied.

## quit with work in progress

quitting from the dock uses the same confirmation as quitting from the app
menu. if hex asks whether to discard unsaved work, choose **cancel** to return
to the project and save it. **discard** closes the project and can cancel an
export in progress. finish a recording or resolve a retained take before
quitting.

## missing media

when you open a project whose audio files have moved, hex shows the
**missing files** panel instead of an error. each row lists the file name, its
last known location and where the project uses it (clips, scenes, sampler and
other devices). **file → open and relink media…** opens the same panel for any
saved project.

- **locate…** picks the file for one row. other missing files from the same old
  folder are found automatically when they exist at the same relative location
  under the new folder.
- **search folders…** searches the folders you choose, including subfolders,
  for files with the same name. a file is accepted only when its length and
  sample rate equal the saved recording. when several files match, pick one;
  a file that differs is shown as a mismatch and is not used. you can cancel a
  search at any time.
- **open with missing offline** opens the project now. clips that use a missing
  file stay in place but play silence, and **missing files: n** shows in the
  status bar. saving keeps their original locations. click **missing files: n**
  to open the same panel for the open project: find the files with **locate…**
  or **search folders…**, then **relink**. relinking is one edit, so **undo**
  makes those clips silent again; files you have not found stay offline. a missing sampler or other device sample, or a missing
  file used by a session clip, still has to be found before the project opens.
- **relink all found** appears when the browser's scanned sample folders
  contain files with the same names. it applies only when you click it: files
  with exactly one matching candidate are found, and files with several
  matches list them for you to pick. add the new sample folder to the browser
  first so it is scanned.
- **open** becomes available once every file is found. the project then opens
  with the new locations as unsaved changes; your project file stays unchanged
  until you save. **cancel** keeps the current project.

hex only reads your sample files; it never moves, renames or deletes them.
matching uses the file name, length and sample rate, so check the loaded clips
before saving: two different recordings with the same name, length and rate
cannot be told apart yet.

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

choose **view → track performance indicators** to show six compact segments in
track headers and arrange return headers. hover an indicator for its sampled
share of audio processing time and the scale. group indicators show only the
group's own work; a shared instrument belongs to its receiver. main processing,
old detached tails and shared overhead remain in the global meter. external
worker cpu is excluded. dim segments mean a fresh reading is unavailable, not
zero work. some playback modes cannot supply these timings and show unavailable. the indicators are diagnostic: they do not measure latency or prove
which track caused a dropout.

solo takes precedence over track mute or deactivation. a muted track sounds while
soloed; turning solo off restores its muted state. solo does not change the stored
track activator setting.

in arrange, select a track or clip before editing it. drag a clip to move it and
drag its left or right edge within the clip header to adjust its extent; hold
**shift** to stretch instead. lower down, the edge belongs to the clip body and
its fade handles.

when moving a clip near its original time, it snaps back to that exact position,
even on another track or with the grid off. move farther away to use the chosen
grid. hold **command** to bypass both the original-position target and grid
snapping for the move.
work at a zoom level where the intended edge
and grid position are clear. use **command+z** to undo and
**command+shift+z** to redo admitted edits.

with the computer midi keyboard off, select two or more clips on one arrange
track and press **g** to group them. select only audio clips or only midi clips.
hex keeps gaps and refuses a group that would cover unselected material. midi
clips become one finite midi clip with their played notes and volume curves.
audio clips become events inside one clip; original files and source processing
remain retained. when an existing group has repeats or outer processing, hex
renders its audible span to one ordinary audio clip. this includes partial repeats
and gaps, keeps original media, and leaves track effects and routing live.
rendered grouping supports one track at a time; press escape to cancel preparation.
grouped audio keeps its source timing at the grouping tempo.
use undo to restore the original clips in one step. grouping requires resident
audio; unsupported streamed sources refuse before the edit is committed.

with a positive arrange time selection, **g** instead groups only the included
pieces, splitting clips at both selection edges and preserving outside material.
each populated track gets its own clip, preserving track routing; empty tracks
are skipped. each track must contain one media kind. ordinary audio groups preserve
inactive events, and midi grouping preserves inactive notes, inside an active clip.
rendered groups with outer processing still require matching clip activation.
the final track's result becomes the selected clip. one undo restores all tracks.

the lower clip editor shows the accepted arrange playback position inside the
inspected clip, following its trim and repeat phase. the moving line is separate
from the edit cursor and saved play-start marker. it stays at the accepted
position when paused and hides outside the clip or while its current audio
mapping is unavailable. the audio source tab shows the corresponding original
source position; the piano roll follows the midi source position. zooming and
scrolling change the visible window without moving playback. in the console,
the piano roll and the audio cell's waveform show the same line while that
launched cell owns its track, wrapping with the cell's loop; it hides when the
cell stops, ends or another clip takes the track.

hover the saved play-start marker to highlight its grab target. drag the audio
arrow or midi ruler triangle to choose where playback starts, keeping the clip's
position, length and loop ends. hold cmd/control to invert snapping; escape
cancels. holding **2** selects time even at the marker. reversed repeats can keep
the start editable when musical timing is off; moving it preserves their trim
and loop bounds. audio with looping off falls silent after the remaining source
ends. some audio views currently show a read-only marker while start editing for that audio view is unavailable.

the audio editor's events and source tabs keep independent zoom and scroll
positions. use the wheel vertically to zoom around the pointer, or horizontally
to scroll. the plus and minus controls zoom around the visible cursor. you can
reveal blank time beyond the clip without changing its content; the view extends
to four times the content length, or at least sixteen beats. **z** fits the event
selection or source range, and **x** returns to the previous view.

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
selection, these keys toggle the existing loop. **command+l** toggles the active
transport loop without changing its bounds. when starting playback with space,
playback starts at the time-selection start or the edit cursor, even when an
enabled loop lies elsewhere: starting before the loop plays into it and then
repeats it, and starting after its end plays on without looping. only when there
is no selection or edit cursor does an enabled loop supply its beginning.
moving or resizing the loop during playback takes effect immediately with the
same rule: inside the new loop the playhead wraps at its new end, before it the
playhead plays into it, and after it playback continues. some first-pass loop
edits keep a held arrangement instrument note sounding, when the note and its
release finish before both loop ends and playback can retain the same source
history. when playback needs to restart at the playhead, effect tails reset and
notes under the playhead start again from there.
at a normal loop wrap, arrangement audio fades briefly at the end so the next
pass can keep its original attack. this fade belongs to the loop boundary.
for multiple sections, the loop spans the earliest start to the latest end,
including gaps. while the computer midi keyboard is on, plain **l** and **w** play notes.
setting new loop bounds from selected main sections also turns on section focus
after the edit is accepted. focus shows tracks that intersect the selected
sections, excluding gaps between them. repeated loop toggles leave focus unchanged;
press **q** or use the section focus toggle to restore all tracks.

**shift-click** a section block to select every section from the last clicked one
through it; **command-shift-click** adds or removes a single section. both work in a
section's lower body too. ordinary lower-body dragging still selects time.
drag a section header to move its musical content; hold **option** to insert a copy
before or after another section, or beside the source. past the last section,
move or copy to the snapped pointer beat; a new section marks the intervening gap.
a move closes the old interval and preserves music in the gap. move/copy and the
gap marker share one undo step. command toggles snapping for a move; use control
while option-copying, since option-command edits only markers. changing option
during the drag switches between moving and copying. the source stays in place when copying, and
one undo restores the entire operation.

while dragging a section, the cyan bar marks insertion at an edge or in a gap.
a highlighted section body marks replacement: the target's music is removed and
the source content takes its place. later music ripples by the length difference;
a move also closes the source's old interval. the result carries the source name
and color. option preserves the source and gives the replacement a new identity.
dropping on the source body does nothing. option-command marker edits do not
replace content. the indicator stays with a pending edit until its result arrives.

with the section lane focused, select one section and press **command+c**, then click
the lane to place the edit cursor and press **command+v**. paste inserts at the nearest
section boundary before or within the song. after the last marker, it lands at the
cursor and gives any intervening marker gap a new section. all tracks are copied,
and one undo removes the whole paste. the section cursor keeps the snapping chosen
when you clicked it; command+v does not invert the grid again.

the section clipboard stays available after successful copies of the same section. place the
cursor again for another paste. other project edits, undo or redo make the reference
stale; copy the section again before pasting. opening another project clears it.
right-click a section for **copy section** and **paste section at cursor**.

section edits apply while playing. when an edit changes the tempo, a sampler's held
notes or a device's latency, audio briefly restarts at the playhead: effect tails
cut and sounding notes start again, and playback keeps running.

hold **option-command** before or during a drag to adjust only the marker; the music
stays in place. option-command-resize an edge to trim or remove overlapping section
markers, or use **option-command-delete** to remove the selected marker only.
releasing the keys keeps that drag marker-only. **command** alone temporarily inverts
section snapping; **control** remains an alias, including during marker-only edits.
release outside the section lane to accept the last valid preview, or press **escape**
to cancel.

click a section's play button, or turn on **section autoplay and loop** in the
top toolbar and click a section body. hex sets the arrange loop marker to that
section, turns the loop on and starts playback at the section start.
**enter** launches the focused section button. **space** controls play and
pause even while the button has focus. the launch dropdown in the toolbar also
sets quantization for console clip and scene launches: **none**, **8 bars**,
**4 bars**, **2 bars**, **1 bar**, **1/2**, **1/2t**, **1/4**, **1/4t**, **1/8**,
**1/8t**, **1/16**, **1/16t**, or **1/32**. **t** means triplet timing.
when a console launch replaces a playing audio clip, the outgoing clip fades
briefly as the new one begins.

current playback continues while hex prepares the section. the prepared section
starts at the next selected boundary after it is ready; **none** starts as soon
as it is ready. a stopped transport starts immediately. clicking another section
replaces the pending launch. stop or turning autoplay off cancels a pending
launch; turning autoplay off leaves current music playing.

the loop marker controls repetition after the launch. turn the loop off or move
its edges to change what repeats. the marker is saved with the project and
changing it is undoable. autoplay stays armed for the next section click.
modifier clicks, renaming and dragging keep their editing actions.
at narrow window widths the top toolbar uses two compact lines to keep every
control, the full view name and audio status visible.
section playback currently requires resident audio mode and an applied arrange
playback graph. verified streaming mode is unavailable for this control. a source
that exceeds runtime storage or midi density limits is refused while current
playback continues.

hold **2** to force time selection over arrange clips, audio events, piano-roll
notes or velocity bars, even with midi draw mode enabled. release **2** to return
to ordinary editing. a range already being dragged continues until mouse release.
an i-beam cursor marks the time-selection tool while **2** is held.
text and numeric fields keep normal number entry.
the clip editor shows the selected clip. clicking away or making an arrange time
selection clears its contents until a clip is selected again.

a time selection shows a dark dot when its musical length is a power of two,
such as two or four beats. the dot follows the dragged top-right or top-left
corner and stays visible after release.

with the arrange timeline focused, **b** keeps the originals and inserts
bounced audio on a new track below
each source track. **command+b** does the same and then deactivates the bounced
source clips or time selection, exactly as **c** or **0** would; one undo restores
both the bounce and the sources' activation. a nonempty time selection takes precedence over selected clips.
each selected track produces one audio clip spanning its selected clips and gaps.
clip bounces render instrument and effect release tails, but the new clip keeps the
original clip's start and length; drag its right edge out to reveal the tail. a time selection renders
exactly its start-to-end interval, retaining earlier state needed for sound inside
that interval. press **escape** to cancel; one undo restores the entire operation.

use **shift+b** on a time selection across tracks to render one mixed audio
clip on a new track. the mix includes the selected tracks' devices, gain and
pan, and includes each selected group's submix once. **command+shift+b** also
deactivates the source clips inside the selected interval. one undo reverses
the bounce and the deactivation together.

select a time range on a group row to bounce its submix. **b** inserts an audio
track beside the group and leaves the original tracks playing. **command+b** on
one group row adds the same bounce and silences the original group's output
only for the selected interval. child clips stay intact, and group effects keep
processing through the muted interval. both commands include nested audio and
midi tracks and print the selected group's effects before its mixer. the new
track carries the group's mixer and sends. one undo removes the bounce and, for
**command+b**, restores the original interval's playback.

while dragging arrangement clips, hold **b** as you release to drop a bounce of them
at the drop position instead of moving them. the dragged clips stay where they were,
the bounced clips keep their original lengths with the tail in the file, and one undo
removes the drop. the drop shows as pending until rendering finishes. let go of **b**
before releasing to move normally. a bounce drop that would overlap its own source, or
that lands between tracks, is refused.

disable live midi input or audition before bouncing in place. bounce in place works while
playing: the render runs while the song continues, then audio restarts at the cursor on the
baked clips, so release voices and effect tails cannot duplicate the baked tails. when stopped,
hex keeps the stopped cursor. undo and redo of an in-place bounce follow the same rule.

per-track bounce writes float audio before the track mixer. mixer automation, sends, returns
and main processing remain live. only the track-header **bounce in place** action replaces material in place and
removes the track's devices. it covers the entire track, regardless of a smaller
clip or time selection. on a group, it prints the descendants' arrangement
material and release tail to one new playback child. the group's mixer and sends
remain live. its printed devices are removed, and the original descendants are
muted in place with their own devices, arrangement clips and console cells
preserved. undo restores the group devices and each child's previous mute state.
unsolo group children before bouncing in place. an external shared source or
sidechain key can prevent a group bounce. **command+b** also refuses when the
selected group supplies a sidechain key.

for clip and time-selection editing, use **g** to group
material instead; grouping retains its existing source and processing behavior.
in the focused midi editor, **b** keeps toggling draw mode. a bounce needs a
supported routing and storage mode. device latency is aligned using the admitted
compensation graph. clip and whole-track bounces keep up to 30 seconds of release
after the selected span. if the tail is still sounding then, hex cuts it at that limit and
finishes the bounce. a noise floor at or below -80 dbfs peak in both channels
counts as silence for tail length. hex retains up to 50 ms after the last louder
tail sample, without trimming the selected span or changing retained audio. it checks
the bounded tail for later echoes before trimming, so a quiet gap does not hide a
repeat. persistent noise can still require rendering the full 30 seconds. source
files remain unchanged.
while editing text, numeric values or midi notes, or using the computer midi keyboard,
these arrange bounce shortcuts do not run.

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


press **e** in arrange to split audio and midi clips at both edges of the selected
time range, across its selected tracks. existing cuts inside the range remain;
boundaries in gaps or at clip edges add no pieces. the range stays selected and
one **command+z** undoes all cuts. without a time range, **e** splits at the edit
cursor. main section focus keeps its section-split behavior; the audio event
editor uses its own local selection. **command+e** is not assigned.

the colored name area shows each ordinary track's name. click its blank area to
select and focus the header; existing command/shift selection and **command+r**
rename shortcuts also apply. drag the header width edge to show more of a long
name. narrow headers truncate the label and keep its full-name tooltip.

drag the bottom edge of an ordinary track header, or the top edge of pinned main,
to resize that row. hold **option** while resizing to give visible ordinary tracks
and main the same height. at the minimum height, rows
keep their names and clip titles; expand them to use the mixer and fade controls.
with the computer midi keyboard off, press **h** in arrange to toggle a compact
overview. press **h** again to restore each track's previous height. resizing in
the overview is temporary; it does not replace normal heights. these view sizes
last for the current editor session. main shares the compact overview; return
rows retain their fixed height. hold **option** and scroll vertically over an
ordinary track or main to resize only that hovered row. wheel up grows it; wheel
down shrinks it. nested controls keep their own wheel behavior. overview wheel
sizes are temporary, just like overview edge resizing.
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

## linked clip copies

with whole clips selected, **d** makes independent copies and **command+d** makes
linked copies. a small link icon in the clip title identifies each alias. linked
audio shares its events, pitch and warp edits; linked midi shares its notes.
each copy keeps its own arrangement position, trims, loop window and fades.

press **shift+i** or choose **make unique** from the clip menu to detach the selected
copies. their current content and sound stay unchanged. **apply settings to aliases** in
the clip menu copies the picked alias's local settings to every member of its
group, preserving each arrangement position. there is no privileged original:
removing the first clip leaves the other aliases intact. these edits support
undo, redo and saving the project.

in console, duplication uses the next empty cell on the same track and adds a
scene when necessary. note editors, text fields, track headers and the computer
midi keyboard keep their contextual shortcuts.

## arrangement automation view

press **a** or use the toolbar's **a** control to show automation across
the entire arranger, including unselected tracks. audio and midi clips fade into
the background while the curves own editing. the toolbar control stays highlighted
while this mode is on; the key and toolbar always switch the same view.
repeat to return to clip editing. switching views preserves clips, selection,
automation and undo history. **c** and **0** keep contextual activation for focused
devices and track headers; in clip editing they affect the selected clips or time
span. a partial time span splits clips at its edges. automation lanes prevent
activation of the retained background clips. text fields and the computer midi
keyboard keep their own input.

in the focused midi editor, **c** or **0** toggles the selected notes. inactive
notes remain editable and are silent during playback; copies retain their state.

click empty space in a lane to add a point, drag a point to change its time or
value, and release to apply the edit. drag across empty space to draw a
selection rectangle around several points; hold **shift** to add to the
selection or **command** to toggle points in and out of it. dragging any
selected point moves the whole selection, and **backspace** removes every
selected point. editing another track's curve keeps the background clip
selection. **command** gives free beat placement while dragging a point.
**escape** cancels a held edit or rectangle. one undo restores a completed change. background clip edits are unavailable
while the automation lanes own arrangement focus.

right-click a supported device parameter and choose **show automation** to use
the track's main automation row, or **add new automation lane** for a separate
lane. a track can show several device lanes at once. from the track-header menu,
choose **automatable parameters…**, then the device and parameter. main and
return headers offer the same picker for their effects.

**show used automation lanes** reveals lanes with authored points. **show all
automation lanes** also restores empty lanes you have already shown and the
current parameter preview; it does not assign every available parameter.
press **shift+a** to show all these lanes across the arranger at once.

device parameters are not assigned automatically. the main row follows the last
supported parameter you touch or show. a dashed line previews its base value
without changing playback. adding the first point creates a solid automation
curve. right-click a lane for **clear automation** to remove its points and return
to the base value, or **hide lane** to keep the curve while hiding its display.
point edits and clearing support undo and redo; authored curves survive saving
and reopening.

editing a supported parameter manually overrides its curve while retaining the
points. right-click the parameter or its lane and choose **re-enable automation**
to resume that curve at the current position. the transport's re-enable control
restores all overridden device curves. override selections support undo and redo
and are retained when saving, reopening and rendering the project. clicking a
control without editing it leaves automation running.

supported device controls are utility's parameters, saturator's four parameters,
auto pan's amount, rate, phase and waveform, and rattle's drive. native effects on tracks, main
and returns also offer host **input gain** and **output gain** lanes. instrument
and source-owned effect lanes, other device controls and vst lanes are not yet
available. recording device movements and shared modulator assignments are not
implemented.

recorded track-gain touch spans are displayed but read-only: leave automation
view and use the track's gain editor to clear the lane before editing its base
curve.

## audio clip pitch

for an ordinary audio clip, drag the **pitch** dial beside the warp controls.
its readout is in cents; 100 cents is one semitone. click the readout to type a
value between -4800 and +4800 cents. **up/down** steps by 100 cents;
**shift+up/down** steps by one cent. double-click the dial to reset to zero.
**escape** cancels a held draft; one undo restores a completed change.

with several clips selected, pitch changes remain relative to each clip's
existing value. in **re-pitch** mode the dial is unavailable: tempo determines
the audible pitch while the stored transpose value is retained.

pitch and warp reconstruction use the resampling version saved with the clip.
new material uses the current version, which reduces aliasing when you raise
pitch substantially. opening an older project preserves the version used by
its existing pitched or warped material.

## clip loop resizing

hold **option** over a clip's right header edge to show the circle cursor, then
drag to repeat its source. the modifier you hold when the drag starts decides the
gesture for the whole drag: none trims, **shift** stretches, **option** repeats
(option wins if both are held). dragging the right edge of a clip that already
repeats also extends its repeats. audio and midi clips use the same gesture; no
loop icon is painted inside the clip, and the fade-out handle keeps the
bottom-right corner. looped clips have rounded bottom corners. the first repeat
boundary uses left/right arrows; drag it to change the cycle while keeping the
outer duration; directed clips have no first-boundary handle. inside a time
selection the clip header moves or copies the range instead. **escape** cancels
a held edit; one undo restores a completed drag.

clips and sections keep their actual timeline width when zoomed out. zoom in
to resize very short clips when their edge handles no longer fit.

shortening or deleting end material keeps your current arrange position and zoom.
when you pan or zoom afterward, obsolete empty tail outside the new view retires;
scrolling cannot return to that retired space. arrange keeps at least eight
seconds of normal space beyond content. **command+0** fits the normal navigation
extent. **z** fits the selection or content in time and the selected track span
vertically in arrange. **x** restores the prior time view, track heights and
vertical scroll, including former visible trailing space. minimum row heights
can leave a large selection scrollable.

## arrangement locators

place the insert cursor or select a time range, then press **i**. a locator appears
at the cursor or the start of the range. pressing **i** again at the same frame
keeps the existing locator. computer-keyboard midi keeps priority.

click a locator triangle to play from its position. drag the triangle to move it
on the arrangement grid. hold **command** to invert snapping. double-click the
name to edit it, or select the locator and press **command+r**. clear the name to
restore automatic naming. select a locator and press **backspace** to delete it.

with the arrangement focused, **command+option+left** and **command+option+right**
move to the previous or next locator. while stopped, they change the next play
start without starting playback; while playing, they seek from the scheduled
playhead and keep playback running. navigation stops at either end of the list.

inside a section, locators use its colour and take names such as **verse.1** and
**verse.2** from left to right. custom names still count in the sequence. outside
sections, locators are grey and use names such as **locator 1**. section edits
that move, copy, or delete music carry locators with it. marker-only edits
change the section bounds around the locators. locators are saved with the
project and support undo and redo.

## create fades from a time selection

select a positive time range at the beginning or end of an audio clip and press
**command+f**. the selected portion becomes an s-shaped fade-in or fade-out; the
opposite authored ramp stays unchanged. dragging a new fade also starts with an
s-curve. drag the diamond to reshape it, or double-click the diamond to
straighten an s-curve. a range entirely inside a clip, or one
covering both ends, is refused with a message.

across a join between two ordinary audio clips, **command+f** extends the source
handles into the selected interval and creates a complementary outgoing/incoming
crossfade. the existing sound stays at its original song time. clips need enough
source material; loops can supply their repeated source. gaps, more than two
clips, midi material and retained or warped crossfade joins are unsupported and
leave the project unchanged. individual edge fades support ordinary retained and
warped views; directed outer views are currently unsupported.

one undo restores the whole command, including both crossfade clips. source files
remain intact. a crossfade permits at most two overlapping audio voices, with
opposing fades covering their intersection. removing a required fade while the
clips still overlap is refused; undo the creation or separate the clips first.
crossfaded clips can be split or duplicated together. grouping overlapping clips
into a source-event container is currently unsupported.

outside a focused audio time-selection context, **command+f** keeps browser search.
plain **f** keeps its momentary fade-handle behavior.

## audio event fades

the audio event editor marks the clip's playback start with a fixed vertical
line and a small right-pointing arrow at the top. it follows the clip's source
crop and loop phase, independently of the edit cursor and loop brace. zooming
or scrolling can move the marker out of view; it returns when that source
position is visible again. the marker is a read-only guide.

select an audio clip and choose **events** in its detail area. focus the event
waveform and select an event, then press **c** or **0** to change its activation.
inactive events keep their source and position and contribute silence. hold **f**
to show its fade controls. release **f** to hide default
controls; edited fades remain visible. the computer midi keyboard must be off.

for a grouped event, **src** selects an event inside its retained source. use
the adjacent left and right arrows to choose another source event, and **c** or
**0** to change its activation. use the up and down arrows to move through nested
sources. choose **src** again to return to the grouped event.

drag an upper square to change a fade length. drag a diamond horizontally and
vertically to shape its curve. lower squares trim the event; they move freely by
default, with **command** opting into the event grid. double-click an s-curve
diamond to straighten that ramp. double-click an endpoint to reset the event's
fades. one undo restores each completed gesture.

the preview becomes prepared audio after release. **escape** or leaving the
editor cancels a held gesture. splitting retains the source fade, moving keeps
it attached to the sound, and an explicit trim fits it to the new event window.
select one event edge and press **command+f** to fit that source fade to the
selected interval. this uses the retained source clock, including cropped loops
and warp. joins between source events are unsupported; use eligible arranger
clips for a crossfade.

event fades use the same supported resident clips as event editing; a refused
preparation leaves the project unchanged. original media files stay intact.

# find, preview and import audio

open the browser with **command+option+b**. press **command+f** to search when
focus is outside a selected audio time range; click the search field to search
while retaining an arrangement selection.
use the browser's categories and locations to narrow the results. when a search
is entered, click **×** inside the field to clear it and type a replacement.
clearing keeps the selected category or location.

drag a folder from the browser results onto **places** to bookmark it. the
folder stays in its original location; bookmarking keeps the current browser
source and search. dragging the same folder again preserves its existing place
and icon. press **escape** or drop outside places to cancel.

adding a sample folder starts a background scan; the samples count shows its
progress. the scan records each sample's size and a content fingerprint so hex
can later find moved files. it only reads your samples; it never moves or
changes them. if a place's folder has been moved or renamed, the place shows
**missing**. right-click it and choose **relocate…** to point it at the new
folder; its position and icon are kept and its samples reappear after the scan.

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

when a new import has a different sample rate from the project, hex uses the
current conversion to prepare playback. a saved import keeps its original
conversion version when the project is reopened.

as projects grow, ordinary audio clips can play directly from their original
files while retaining their complete duration. single-file and bulk imports keep
this behavior; a bulk drop remains one undo step. keep those files available for
playback, save/reopen and export.

the **disk** indicator lights when streamed data arrives too late for playback. hover
over it to see how many playback callbacks were affected on the current output.
it clears three seconds after the last reported increase. short fades ease the
transition into and out of missing audio.

on an arrangement audio clip, hold **option** and drag vertically on the top edge
between the trim corners to adjust clip gain. in the audio event editor,
**option**-drag the top edge of an event to adjust its gain. drag down to reduce
gain or up to increase it. hold **shift** during either drag for fine
adjustments. the displayed db value previews the change. release commits one
undo step, and **escape** cancels. when the clip belongs to a whole-clip
selection, selected audio clips change by the same db amount, retaining their
relative levels. held **2**, existing time selections and fade controls retain
their input priorities.

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

older projects can contain midi clip gain and volume curves. hex retains these
settings for playback. when midi clips are grouped, their levels become one
clip-level curve; its changes also affect any notes still releasing from earlier
clips. midi clip audio gain and volume curves cannot be edited in the piano roll.

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

adding a latency-producing effect can require a fresh playback graph. for an
audio-only arrangement without active console launches or recording, hex may
briefly stop, rebuild and resume at the current position; effect tails reset.
if the current performance cannot be rebuilt safely, press **escape** to stop
and restore the current project sound before playing again.

click a device header to focus its card, then press **0** to toggle its bypass.
this uses the same state as the header bypass control and supports undo/redo.
while a parameter field or browser search has focus, **0** remains text input.

select one or more effects in a device chain and press **d** to duplicate them
after the selection. the copies keep their settings and become the selection.
press **cmd-z** to undo the duplication.

during playback, track gain, eq edits and supported device bypass changes apply
to the running audio. supported latency-bearing effects keep the dry signal
aligned when you bypass them. reverb and delay tails may shorten or stop.

if a sounding instrument has a long release, hex can keep that release through
supported receiver paths while you switch the instrument off and back on.
some processed, routed or compensated paths require playback to stop first.
if hex reports that source history cannot transfer live, the previous audio
continues. stop playback before changing bypass on that path.

reverb combines resonant input filters (20 hz–20 khz, q 0.1–10), early reflections and a diffusion network.
use predelay to separate the room from the source; size, decay, diffusion and
scale shape its space. spin moves the early reflections, while chorus modulates
the late network. low/high controls shape frequency-dependent decay. reflect and
diffuse set the two wet components; stereo sets their width. dry/wet blends the
room with the original. on a return, start at 100 percent wet and use the send.

freeze holds the network. flat removes its decay shaping and cut blocks new
excitation. stop and seek clear the room; loop wraps continue it. bypass passes
dry while history advances privately. removing a device drains its old history;
a removed frozen tail persists until stop. retained tails use bounded host banks;
a full bank refuses another edit rather than discarding an existing tail. smooth
sets none, fast or slow parameter transitions; density selects scattering depth.
all controls use undo/redo and survive save/reopen. reverb's internal parameters
do not yet support automation lanes; its host input and output gains can be
automated on tracks, main and returns. the functional control set does not
establish sound parity with another application's reverb.

the delay effect has independent left and right timing. **sync** selects a
multiple of a sixteenth note; **time** selects 1–5000 ms. sync is bounded to
16 seconds, including offsets and modulation. **link** shares timing, mode and
division changes; the two percentage offsets remain independent. **feedback**
sets repeat gain from 0–95%, and **dry/wet** blends the original and delayed sound.

**repitch** glides to new times, **fade** crossfades taps and **jump** changes
them immediately. **ping pong** alternates repeats between channels. **freeze**
ignores new input and cycles the existing history; turn it off to let repeats
decay. the filter graph edits frequency horizontally and octave width vertically.
expand **lfo** for timing modes, waveform, morph, delay/filter modulation,
**hi quality** interpolation and **eq loudness** mix law.

bypass keeps delay history warm while passing dry audio. removing a playing delay
releases freeze and lets its existing repeats decay; stop and seek clear history.
loop wraps preserve repeats. changing sample rate prepares empty history at the
new rate. settings use ordinary device undo and save/reopen; runtime audio history
is transient. delay's internal parameters do not yet support automation lanes;
its host input and output gains can be automated on tracks, main and returns.
the control set follows the reference device, while the filter, interpolation
and modulation algorithms are original hex implementations.

overdrive filters the signal before distortion. use **frequency** and **bandwidth**
to choose the input band; the graph shows this filter response. **drive** adds
distortion, **tone** adjusts its brightness, and **dynamics** restores more of the
input's level changes as it increases. **dry/wet** blends the original signal with
the processed sound. the effect is available on track, return and main chains.

rattle is a one-knob drive. its compact panel pairs a percent drive dial with
eleven orange diamonds. the diamonds tighten as drive rises and follow its
automation while the lane reads. at **drive** 0 it passes audio unchanged; raising
**drive** blends in a progressively harder, grittier and noisier saturation whose
level is compensated for typical programme, so it gets dirtier rather than much
louder. rattle processes at twice the sample rate and reports 11 samples of
latency, which hex compensates. drive changes glide over about 20 ms and can be
automated. bypass passes audio unchanged while keeping its history warm.

in the eq graph, double-click empty space to create a band. the left and right
edges create low-cut and high-cut bands; the bottom interior creates a notch;
other empty space creates a bell. at the bottom corners, the edge cut takes
priority. a small curve beside the crosshair shows the shape before pressing.
hold the second click and drag to position the new band and hear its effect
during playback. release to keep the band as one
undoable edit. vertical movement changes q for a cut or notch, or the gain of a
bell. existing nodes keep their own selection and drag targets. press
**escape** to cancel.

click empty space in the eq graph to clear the selection. drag from empty space
to select every node inside the rectangle. **shift-command** toggles those nodes
against the selection.
dragging a selected node moves the whole selection, keeping each node's
spacing, and **backspace** or **remove** deletes every selected node with its
automation. one undo restores either edit.

in the fixed eight-band eq, hold **shift** to draw a marquee. dragging a selected
band moves the group in one undoable edit. **backspace** disables the selected
bands; their fixed slots remain available. one undo restores their on flags.

each visible eq has its own live spectrum, including eqs on return and main
tracks. selecting another device keeps these spectra running. use **spectrum**
on an eq to turn only its analyzer off or on. a hidden device panel stops its
analyzer; missing or stale audio clears the live curve.

every spectrum view in hex, the eq backgrounds and the project spectrum, uses
the same analyzer. a sine reads at its true level, so a full-scale tone at 1 khz
touches 0 dbfs. the curve is tilted by 4.5 db per octave around 1 khz, so a
typical mix looks roughly level instead of sloping down to the highs. the curve
is smoothed over 1/12 octave, rises at once on new audio and falls over about a
quarter of a second. a thin, fainter line above it holds recent peaks for one
second and then sinks slowly. the project spectrum shows this peak line for the
main output only.

the beta release command packages the default desktop application. optional
native vst3 helper profiles are not included by that command, and this manual
does not promise a complete third-party plugin-hosting workflow.

# audio output and settings

choose **view → track latency indicators** and hover **lat** in a track header
to see delay in samples and milliseconds. local devices shows the longest
processing path within that track. post-fader arrival includes upstream routes.
default route compensation is the alignment added on the way to the parent
group or main. sends can have separate alignment. main output arrival includes
main devices. a pending or unavailable reading has its own marker. these figures
describe the applied audio graph and exclude interface and round-trip delay.

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
**selected stems and shared returns**. the **export format** dialog then offers
**16-bit pcm**, **24-bit pcm** and **32-bit float**. select the tracks needed for
a stem export and choose a new destination in the native file dialog.

exports are wav files. main mix produces the combined output; the stem choices
keep the selected outputs separate, each in the chosen format. 16-bit files are
dithered and 24-bit files are rounded; both clamp samples above full scale, and
the status line reports how many samples clipped. 32-bit float files keep levels
above full scale and never clip. the format is chosen for each export and is not
saved in the project. check the range and output choice before
exporting, then audition the resulting files. offline rendering can be available
even when live audio output is unavailable.

when you confirm replacing an existing wav, hex keeps the previous file and
shows its location after export. inspect it and remove it yourself when you no
longer need it; retained files use disk space.

file-backed audio in ordinary and verified streaming projects uses offline
media verification during export. keep original media available until export completes. hex verifies those files
before reading and again before publishing; missing or changed media, unavailable
pages and cancellation leave the previous destination intact. choose an export
path separate from the original media. editing or opening another document does
not change an export already in progress.

an interrupted export can leave hidden `.hex-export-<pid>-<number>.tmp` files
beside the destination. inspect and preserve any files you need; hex does not
automatically remove them. older versions can remove matching files when
exporting to the same folder, so keep possible recovery files elsewhere before
using those versions.

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
| split time-selection edges / edit cursor | e |
| new project | command+n |
| open project | command+o |
| save / save as | command+s / command+shift+s |
| play or pause / stop | space / escape |
| previous / next arrangement locator | command+option+left / command+option+right |
| set loop from selection / toggle matching loop | l / w |
| toggle active transport loop | command+l |
| import audio | command+i |
| add track | t |
| add track for notes / midi clip | command+shift+t / command+shift+m |
| undo / redo | command+z / command+shift+z |
| toggle browser | command+option+b |
| selection fades / browser search outside audio selection | command+f |
| compact track overview (computer midi keyboard off) | h |
| settings | command+, |
| bounce selected material to a new track | b |
| bounce to a new track and deactivate the sources | command+b |
| bounce a time selection to one mixed track | shift+b |
| bounce a time selection to one mixed track and deactivate the sources | command+shift+b |
| drop a bounce of dragged clips | hold b while releasing a clip drag |
| arrangement automation view | a / toolbar a control |
| show all arrangement automation lanes | shift+a |
| contextual activation | c / 0 |
| export | command+shift+e |
| changelog | command+shift+l |

# known limitations

these limits apply to the current 0.1 alpha. they describe tested behavior, not
promises about later versions.

- **platforms.** packages are built and checked for macos on apple silicon
  only. windows and intel macs are not supported.
- **recovery is explicit.** hex keeps a recovery checkpoint next to a saved
  project, but it never restores one by itself. use the recovery actions in
  **file** (see recover work) and save the result under a new name.
- **true peak limiting.** the limiter's true peak mode keeps every sample at or
  below the ceiling. on fast, heavily gated material, a standard itu-r bs.1770
  true-peak meter can read up to about 0.3 db above the ceiling. lower the
  ceiling slightly if a delivery specification is strict.
- **limiter on a track or return.** a limiter on a track or return costs much
  more processing than one on main. on dense projects this can cause dropouts;
  prefer the main limiter where it achieves the same result.
- **media changed while playing.** if a streamed audio file is deleted or moved
  while it plays, hex keeps playing the data it already verified until you stop
  or reopen the project; reopening asks you to relink it. if a streamed file's
  contents change, hex refuses to play that source rather than play different audio.
  restore the original file or relink it.
- **midi clip trimming** is not supported yet.
- **audio device settings.** hex uses the macos default output and its default
  configuration; there is no device, sample-rate or buffer-size chooser.
- **performance.** timing figures in hex's development notes were measured
  headless on one apple silicon machine. they do not guarantee real-time
  playback on your device, and long sessions with around 100 active tracks
  have not been soak-tested.
- **native testing.** interaction, listening and device checks for each package
  are run by people, not by automated checks. see `TESTING.md` for the package's
  status.

# report a beta problem

include the full version and source shown for the package, the macos version and
audio device, the steps that reproduce the problem, and what you expected to
happen. include the displayed error and whether the issue survives a normal quit
and reopen. keep original projects and media; share only material you have the
right to share.

## inspect diagnostic logs

select the log button beside the version to open the diagnostic log window.
the current session opens first. use previous session and next session to browse
retained sessions, and refresh to read records written since the last refresh.
the time column is in utc.

use the category and time controls and the search field to narrow the visible
records. the time window is measured back from the latest event on the displayed
page; search for `level=error` to find error records. older unclassified records
appear under uncategorised. in a large session, use older lines and newer lines
to move between pages. filters apply to the displayed page, and live history can
shift between page reads.

click a row to inspect it below the list. unusually long rows are clipped in
the detail view. to share a section, click its first row, shift-click its last
row, and select copy selection. the copied excerpt includes the original rows,
available session context and warnings, and is limited to 1 mib. the viewer
reports missing, truncated or unreadable data; oversized legacy segments may
be clipped. logs and excerpts can contain paths or other private information,
so review them before sharing.

`TESTING.md` is the finite checklist for this specific package. record actual
results against that version. passing automated checks does not replace native
interaction, listening, controller/device tests or physical audio deadline checks.
