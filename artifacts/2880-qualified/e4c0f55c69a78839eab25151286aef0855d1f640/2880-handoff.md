# pulse2880 / pr482 — duplicate range selection

Canonical source e4c0f55c69a78839eab25151286aef0855d1f640,
tree edd6648d0b8c64ebba04fd7e71dfff69c4197a41, base plan
 e5afe429363869ecdbf04c38a497a29c2c8a402a (zero-diff mainc187).
History76142263 original2RED ->bb16e3b2 first2GREEN ->7657063d late-ownerRED
->e4c0f55c preserved. Five files+608/0; only23 production/comment lines in
controller/time_selection, plus3 test-module lines, new531line tests and docs/todo.
No Project/Session/Engine/worker/queue/public API/schema/DSP changes.

Actual pointer-created ranges followed by Command-D/Option-drag and real Service
ACK retained destination coordinates but also reselected the first original clip.
The existing generic selection::reconcile fallback was called after range
completion deliberately set selected_clip=None. The narrow controller guard
preserves None only for valid completed ranges with a current cursor generation;
point/no-range/stale fallback and explicit Created/whole-clip selection stay intact.
A separate actual late-ACK control reproduced a newer range on track5 being
replaced by the older copy destination on track1. Completion now applies its
selection intent only while the original source range/scope/current generation
still owns selection. The accepted canonical command still commits and is one
Undo; newer selection or Escape cancels only its view intent.

Five new actual-input/Service cases cover three successive Command-D copies over
single/multiple rows; Option-drag then Command-D; partial audio+MIDI with trailing
gap, exact asset/offset/note boundaries, fresh IDs, rendered range/late frames;
newer range before ACK; and Escape before versus after accepted submission.
Full canonical snapshot Undo/Redo preserves monotone IDs. Fixtures create one Arc
Environment before constructor/workers and put actual imported WAV under that
same root; no independent BatchFiles lifetime, fake Event or canonical overwrite.

Qualification uses exact clean main10-stage receipt plus a SAME-source1-stage
group supplement. Counts/hashes/durations are authoritative in metadata.json.
The original combined command included a wrong group path that matched zero;
its36 passing cases are not counted as group coverage. Only the missing correct
editor::drag::group_drag_ui_test module is supplemented, without repeating the
unchanged36/strict checks. Cheap7 run before expensive work. Full required current-
main qualification/review/merge remains sole merger work, not source acceptance.

Negative provenance: original two REDs ran dirtye5afe test files later committed
76142263 with original production; later ACK RED ran dirtybb16 test-only extension
later committed7657063d. Exact committed test bodies remain available in Git;
current clean qualification independently binds shipping source. Expanded first
attempt's two failures were fixture assumptions, not product REDs: pointer free-
snap projection was24029..120143 rather than guessed24000..120000, and a pending
edit intentionally omitted the drag surface, so no header click executed. Corrected
oracles use actual projected frames/canonical Beat conversion and an actual lower-
body new range, whose ACK theft truly failed. Keep those raw failures separate.

Native coordinator checklist: in a qualified build select partial mixed tracks/
gaps, Command-D three times, Option-drag then Command-D, observe the destination
range after ACK and later frames; Undo/Redo. Select another range while pending;
Escape before versus after submission. No app/native operation, installed-build
or physical paint/user acceptance claim. Pulse remains InProgress until actual
merge; GitHub readiness/review request is independent. Historical PR367 preserved.
