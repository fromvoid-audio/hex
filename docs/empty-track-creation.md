# empty ordinary tracks

Ordinary AddTrack, AddMidiTrack and InsertTrack share the canonical insertion
helper. They now allocate only the track identity and start with no instrument,
no instrument position and no effects. Raw/prepared audio imports retain the
chosen media and clips without inserting Sine. Existing tracks/devices are not
rewritten. Track naming, selection, current-format snapshots and Undo/Redo
remain canonical. Insertion retains the merged non-gray color policy while
allocating only the new track identity.

Explicit device-drop creation remains explicit: InsertTrackWithDevice inserts
the chosen instrument through InsertInstrument, or the chosen effect through
InsertDevice. Dropping an effect no longer also seeds an unrelated Sine.
The default Sine state helper stays available for explicit authored instruments
and existing readers; no migration behavior is added or removed here.

The existing compiler accepts absent instruments. The new Service/prepared
Playback regression accepts a note-bearing empty MIDI track and renders exactly
silent PCM, then proves an explicitly inserted Sine is audible; Undo returns to
silence and Redo preserves the device ID and exact PCM. No OS audio device,
backend, callback, app, MTS/library or permissions are opened. Routed MIDI
receivers still require an actual instrument under existing admission rules.

Private model fixtures that test instrument moves/removal/routed MIDI now author
that instrument explicitly in one command, preserving their workload, IDs and
history assertions. The allocator regression checks the actual next identity and
strictly increasing high-water mark rather than the obsolete hidden Sine stride.
No test-only production default, skipped test or relaxed audio oracle is added.

The real Command-T test keeps naming/history assertions and verifies the empty
track's existing add button, native-device menu and drop region. Its private
startup uses one precreated test Environment for the view and actual catalog-job
leases. A held catalog-save control proves the root and four preference paths
survive view/caller retirement until the real job finishes and cleanup completes.
Text-focus protection is retained. The Command-T binding, header and shell remain
unchanged.
Current-format encode/decode tests preserve both empty and explicitly authored
tracks, with no autoload during repeated reopen.

Enter on a selected, registered browser instrument is an explicit device choice:
it appends a track with that exact instrument's registry default state using one
InsertTrackWithDevice command. Naming, created-track selection and one-step
Undo/Redo follow the existing new-track path. Missing, stale, unavailable or
non-instrument selections refuse before queuing; there is no generic empty-track
or Sine fallback.

MoveMidiClipToNewTrack and CopyMidiClipToNewTrack transfer a clip, not a track's
device chain. Their destinations deliberately remain empty and MIDI there is
silent until an instrument is explicitly added. Source instruments and effects
remain on the source track; clip content and one-step Undo/Redo are preserved.
Duplicating a whole track is a separate operation.

The existing Engine MIDI rendering fixture first exposed an implicit Sine
assumption (12000 instead of 14400 tail frames). After Source4's no-overlap
clearance, its authorized private setup explicitly authors Sine. The related
suite then exposed four further private fixtures: Arrangement/Session source
windows, direct Nyquist refusal and absent/bypassed transitions. The coordinator
extended the bounded fixture grant; those fixtures now explicitly author their
intended instrument while preserving both ordinary creation routes in the
absent/bypassed case. Every existing assertion remains intact. All four returned
cases and all 19 MIDI compile tests pass (the four are included in the 19).
No Engine production code, helper framework or test-only production default
changed. Original red and intermediate partial-suite reports remain source-bound.
Other cross-crate implicit-instrument fixtures are not claimed qualified by this
focused suite; final prospective full CI and integration belong to the merger.

Later coordinator-only native/user checklist: ordinary Command-T and menu tracks
show empty device chains with working add/drop affordances; imported audio keeps
its clip without devices; explicit instrument/effect drops add exactly the chosen
device; empty MIDI remains silent until explicitly instrumented; Undo/Redo and
save/reopen retain contents. Existing authored devices and independently assigned
track colors remain unchanged. Physical/optimized delivery and listening remain
separate from cloud source checks. No native app actions are authorized here.
