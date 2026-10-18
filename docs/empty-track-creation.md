# empty ordinary tracks

Ordinary AddTrack, AddMidiTrack and InsertTrack share the canonical insertion
helper. They now allocate only the track identity and start with no instrument,
no instrument position and no effects. Raw/prepared audio imports retain the
chosen media and clips without inserting Sine. Existing tracks/devices are not
rewritten. Track naming, selection, current-format snapshots and Undo/Redo
remain canonical. The independent #2793 color policy must be retained when
combining its insertion hunk; this branch does not import that unmerged feature.

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
startup roots use the existing bounded browser constructor; text-focus protection
is retained. No production header, shell, keyboard binding or panel edit is needed.
Current-format encode/decode tests preserve both empty and explicitly authored
tracks, with no autoload during repeated reopen.

Known integration boundary: the original Engine MIDI rendering fixture assumed
AddMidiTrack inserted Sine (12000 instead of 14400 tail frames). After Source4's
no-overlap clearance, its authorized private setup explicitly authors Sine and
the unchanged targeted tail/PCM test passes. The full 19-case midi_compile suite
then passes 15 and exposes four more implicit-instrument fixtures (Nyquist,
Arrangement/Session source windows and absent/bypassed transitions). Those lie
outside the authorized setup hunk and remain pending bounded owner coordination;
no assertions or Engine production code were changed. This draft is not ready
while confirmed required failures remain. Dedicated merger owns final prospective
checks and merge.

Later coordinator-only native/user checklist: ordinary Command-T and menu tracks
show empty device chains with working add/drop affordances; imported audio keeps
its clip without devices; explicit instrument/effect drops add exactly the chosen
device; empty MIDI remains silent until explicitly instrumented; Undo/Redo and
save/reopen retain contents. Existing authored devices and independently assigned
track colors remain unchanged. Physical/optimized delivery and listening remain
separate from cloud source checks. No native app actions are authorized here.
