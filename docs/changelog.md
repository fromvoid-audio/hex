# end-user changelog

The editor's **changelog** button beside the log control reads notes embedded at
build time. `cmd-shift-l` opens the same reader; Escape closes it and returns
focus to the editor. Tab or Shift-Tab reaches the visibly focused close control;
Enter or Space activates it without moving focus into controls behind the reader.
It performs no network or model calls. Notes show their declared
coverage revision; later unrelated changes do not silently extend that coverage.
Missing notes are visibly unavailable rather than a fabricated release summary.

Before freezing a delivery candidate, commit the implementation and run:

```
python3 scripts/local-ci.py --prepare-changelog --changelog-base BASE_COMMIT --changelog-through END_COMMIT
python3 scripts/changelog.py validate --through END_COMMIT
```

The first preparation requires an explicit baseline. Later preparations default
to the preceding coverage end. An explicit follow-up baseline must equal that
end; gaps and overlapping retained batches are refused before inference and
independently during offline history validation. The existing configured `codex exec` performs the
AI generation with a bounded Git diff, schema, and prompt; no provider or model
configuration is changed. Generation failures retain previous valid notes and
print the retained diagnostic directory. Commit `changelog/generated.json` before
running candidate checks. Never generate or edit tracked notes after qualification.

Ordinary local CI validates the schema and immutable declared input range offline.
It does not call a model, require notes to cover unrelated subsequent commits, or
claim missing notes are a generation success. Explicit `--through` validation
fails for missing or stale requested coverage. Full source patches are private
preparation inputs, not shipped in the compact end-user dataset.

For a bounded capability test, `export --base BASE --source END --output NEW_DIR`
exports the exact input, schema, prompt, and hashes. An exported packet alone is
not generated notes or proof of unattended inference. Cloud inference currently
has recorded proxy failures; successful configured Mac execution is separate
route evidence. Native viewer acceptance and final candidate qualification remain
separate from generator validation.

Actual configured Mac CLI output from an exported packet can be consumed with
`import-result --packet DIR --result RESULT --execution EXECUTION --evidence-commit SHA --evidence-archive ARCHIVE_PATH`.
`ARCHIVE_PATH` names a regular tar.gz blob inside the immutable evidence commit,
not an arbitrary local file. Import reads that local Git object without extraction,
verifies its bounded regular-only members against `PAYLOADS.json`, and binds
`result.json`, `execution.json` and `original-input/{input.json,schema.json,prompt.txt,INPUTS.json}`
to the supplied bytes. If the object is absent, explicitly fetch the known evidence
ref first; import never fetches or calls a model. A missing local object does not
invalidate remotely retained evidence. Ordinary offline CI does not require
historical evidence refs in every checkout.
This validates exact exported bytes, range, schema, prompt, original result hash,
and successful execution before atomic publication. This supplementary evidence
transport does not pretend the unavailable cloud inference route succeeded.

Exports include all retained note text in the deduplication prompt. Newly owned
packet directories are private (`0700`) and packet files are `0600`, independent
of a usual `022` umask; existing directories are refused without changing their
permissions. Successful automatic preparation removes its owned temporary source
packet. Failures retain and identify diagnostics. Imported completion timestamps
must be real UTC ISO timestamps before publication.

Git diff inputs pin Myers, the indent heuristic, 50% rename detection with a
1,000-file limit, ordinary `a/` and `b/` prefixes, quoted paths, three context lines,
zero inter-hunk context, no custom ordering/color/external diff/textconv, and the
original fixed eight-character index metadata convention. Git emits full object
IDs first; only exact Git index header lines are shortened, so object-database
ambiguity cannot extend them and source hunk content is untouched. The original 274,953-byte
configured-Mac input is byte-identical under these settings; retained input,
prompt, schema and real generated notes are not rewritten or regenerated.

Git output is capped while reading the subprocess stream, before the complete
patch can be buffered. Imported exit codes and attempt counts must be JSON
integers, not boolean aliases. Publication captures the exact previous dataset
bytes before generation or import verification, then takes a nonblocking
checkout-local Git-metadata lock and compares those bytes before atomic replace.
The lock is never held during inference. A competing successful publication or
active publisher produces a visible retry refusal and preserves successful notes;
retry from the now-current history rather than forcing an overwrite. This lock
does not affect audio callbacks, app state or unrelated local CI.

## Reader presentation — Pulse2849 / PR451

The native reader projects validated embedded entries into features, improvements
and fixes, omitting empty categories and retaining complete text and order within
each category. Each batch keeps its version and explicit notes-through source.
Full original base/source, generation time, generator, origin and evidence commit
remain available on the coverage tooltip when present in the dataset. This is a
presentation projection; generation, import, validation, CAS and packaging do not
change. The original seven-note dataset and its cdba26a8 → cb8befc4 coverage remain
unchanged; newer source work is not implied to be included.

The centered reader is capped at 640px, with compact header/close and wrapped note
rows. Existing palette roles distinguish headings: features uses secondary,
improvements text and fixes primary. A whole note row, including its padding,
provides the hover region. Only that row's text becomes secondary and its surface
becomes hover; dimensions, neighbours, scroll and heading colors remain stable.
Rows have an ordinary arrow cursor and no action or tab stop. The single keyboard
close target, shortcuts, editor-focus restoration and read-only key isolation are
preserved. Native drawing still uses bundled Ioskeley Mono through theme::text.

Headless GPUI checks cover completed inherited text paint style/font, row-local
pointer enter/move/leave, stationary-pointer scrolling, row-ID reuse, widths 480,
880 and 1280px, wrapped/unbroken long text, empty/unavailable/malformed data, all
seven original note texts, and the existing opener/close keyboard/scroll cases.
These checks do not establish physical macOS GPU pixels or accessibility parity.

Finite coordinator-owned optimized native check on the final combined build:
open beside Log and with cmd-shift-l; confirm the three grouped headings, seven
notes and original coverage. Hover the padded/wrapped parts of neighbouring notes,
leave and scroll beneath a stationary pointer; only the current note should gain
emphasis with no movement or lingering highlight. Resize within supported window
sizes and confirm readable wrapping, clipping and stable Close. Check Tab and
Shift-Tab reach only Close, Enter/Space activate it, Escape restores editor focus,
and Operator's existing Tab cycle works after closing. No save/quit/relaunch,
listening, generation or user-file operations are needed for this changed case.
