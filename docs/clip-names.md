# clip names

Pulse #2660 / PR276, requested 17 October 2026. Click an Arrangement clip or
Session clip cell to select it, then press Command-R. The existing clip name
becomes a selected inline text draft. Enter commits one canonical rename;
Escape discards the draft. Clicking another control commits a valid draft
without taking focus back. Empty names are refused, keeping the original.
Text editing owns spaces, letters, arrows, clipboard and deletion keys; they do
not trigger transport, Solo, note movement or clip deletion. Selection, playback
and source data are independent of naming. Native visual/input acceptance is
still required; no measured Ableton parity is claimed.

Arrangement audio and MIDI clips show a compact 16 px name strip. Clip bodies
retain their existing selection and drag behavior. Session uses its existing
name label. Names truncate to the available width; the editor scrolls its text
horizontally to keep the cursor visible. Drafts retain raw user spelling/case,
including Unicode; steady labels use the existing lowercase display boundary.
The native IME/selection/clipboard implementation is shared with browser search.

`RenameClip` targets a stable ID in either domain. Existing name validation
requires nonblank text without control characters and at most 1024 UTF-8 bytes.
Names live on canonical Arrangement clips and independent Session clips, enter
history and snapshots, and survive trims, splits, duplication, time-range copies
and Arrangement-to-Session copying. New and migrated Arrangement clips default
to `clip`. Source asset paths and names remain independent. An unchanged rename
adds no history. Drafts capture generation/revision/ID/original name and cannot
write into a replacement project or overwrite a newer edit.

Schema26 retains the complete schema25 envelope and adds a required
`clip_names` array with exactly one `{clip, name}` entry per Arrangement clip.
Missing, duplicate, unknown or invalid names refuse admission. Session names
retain their existing fields. Schema1–25 decoding retains its historical
contracts and supplies default Arrangement labels. Explicit old-schema writers
refuse authored Arrangement names rather than dropping them. Schema26 files
require the updated application; older builds cannot open them.

Source tests cover canonical validation/history, copy/split preservation,
roundtrip and malformed/migrated storage, rendered selection/rename/typing/
commit/cancel and stale draft rejection. Native checklist for the user:

- Select audio and MIDI clips, press Command-R, type a name, Enter; verify it
  appears in place. Escape restores the previous name. Click elsewhere to finish.
- Type spaces, `s`, arrows and Backspace; transport/Solo/clip placement stay put.
- Rename Session audio/MIDI cells without launching them; Undo/Redo and
  save/reopen retain their independent names.
- Check long/Unicode names, narrow clips, selection feedback, drag, trim and
  time-range gestures in a small window.

Linux GPUI source/rendered tests do not establish native macOS pixels, IME input
or listening acceptance. The user owns those checks under the repository policy.
