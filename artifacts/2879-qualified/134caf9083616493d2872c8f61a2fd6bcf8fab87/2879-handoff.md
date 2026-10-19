# pulse2879 / pr481 — focused BPM arrows

Canonical source134caf9083616493d2872c8f61a2fd6bcf8fab87,
tree9c7f5a2a7ba126c9374f88e39bee2d6c913ed9df, baseplan
e67e18813c8b32c30a6b122f6dd56172653011ee (zero-diff mainc187).
Six files +430/-11: only existing shared NumericControl and project/sample input
owners change behavior. No model/Service/Engine/schema or global modifier change.

Both BPM owners explicitly request 1/0.1 displayed-unit steps and opt into
Command-fine key context. Shift is coarse for these owners; default numeric
controls keep their existing Shift-fine policy. Valid unmarked text drafts step
locally until existing Enter/blur commit. Invalid/composing text is untouched;
Escape restores the acknowledged value. Formatting six decimals between repeated
steps preserves canonical micro-BPM precision. Inclusive1..1000 step clamping
and existing typed-value refusal and revision/generation/clip guards remain.

Original actual GPUI Up120 -> expected121 stayed120: baseline receipt ran dirty
plan with only test additions later committed8f3e763d. First repair passed same
input+real Service commit/one UndoRedo. Expanded tests passed project bounds,
100 fine steps each direction, Shift coarse, invalid/IME/cancel/rootfocus.
Sample actual canonical edit passed before the first test's Undo unwrap(None)
failed: unedited sampleBPM is genuinely None (tempo-map fallback). That is fixture
assumption, not product RED. Production unchanged; corrected test compares full
actual Project before/after on one UndoRedo. The new tests create one Arc
Environment before constructors/workers, generate/import WAV under same root,
and use real Init/import/warp/tempo Service ACKs. No fake Event or state install.

Final clean source qualification is the single12-stage receipt, exact counts and
hashes in metadata.json. Cheap guards/fmt/diff precede tests/strict. Source-bound
logs cover actual emitted UI selectors, all shared widget lib controls, strict
UI default/feature and widgets. Full prospective current-main required checks,
fresh review and merge belong to sole merger. No native app/control/listening,
installation, full21 or user acceptance claimed.

Pulse stays InProgress until actual merge. GitHub review request is not approval.
Historical336/387 preserved, no duplicatePR. Note30355's source_sha incorrectly
expanded d14bd8e8; corrected30356 actual d14bd8e835dfd25ecead3160490fc8fe42252b2b.
Original raw diagnostics/source history remain available; final clean receipt
independently binds all shipping bytes.

Root finite native checklist on qualified optimized source: project and enabled
sample BPM Up/Down1, CommandUp/Down0.1, Shift coarse, repeated fine precision,
Enter/blur one UndoRedo, Escape, invalid/IME text, limits1/1000; unfocused arrows
retain workspace behavior. No native job requested/executed by this source task.
