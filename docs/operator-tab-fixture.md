# operator tab fixture isolation

Pulse 2753 / canonical PR363 owns this test-only correction. The historical
17 October run `20261017T113521Z-9eb9ff71` on candidate `23732ba0` reported a
long-running Operator Tab test with a native stack in browser preview/resampling.
That stack is historical evidence, not a new reproduction of a production hang.
The prior native BPM/Operator pass also remains valid for its original source.

## source boundary

The old `actor_editor_window` used `EditorView::new`: browser state chose ambient
home/Music roots and real preference paths, then immediately started preference,
listing and sample-cache workers. Replacing browser fields after construction
cannot isolate that initial work. Injecting only a browser preference path also
leaves the initial Music root ambient.

The Operator Tab fixture now owns a unique disposable directory and injects
browser, recent-project and theme paths plus the initial browser directory/places
**before** `browser_scan_initial`. The override and entry point are compiled only
for tests with `ui-test-support`; the existing constructor signatures and normal
constructor choices remain intact. No process-global environment, real preference,
library, production Tab binding or production audition behavior is modified.
Other fixtures keep their existing constructors.

Both the missing-preferences and saved-additional-place variants execute the
original traversal assertion body unchanged: more than 20 forward targets,
rendered oscillator/page/routing membership, finite wrap, reverse traversal with
no trap, focused target and unchanged project/revision. The browser stays open.
Each variant supplies only 32-frame 44.1 kHz WAVs. Ordinary browser discovery,
cache filtering and waveform/preview code remain enabled; there is no skip,
preview stub, artificial audition disable or traversal-count reduction.

A preloaded derived catalog also contains an unrelated WAV under a different
**disposable** folder. Initial directory/places are checked immediately, before
pumping asynchronous work. After startup and after both traversals the persisted
catalog must contain exactly the controlled selected-root media; the stale
outside-root entry must be absent. Combined with isolated preference/cache paths,
this bounds the media identities available to preview without accessing a real
machine library. The two settings variants are injected files, not mutations of
ambient HOME or running another test against a user's settings. This is input
confinement evidence, not an OS-level file-access trace or physical timing proof.

## validation and limits

The first new focused run `20261018T142932Z-6b5cc4db` failed compilation: a changed
constructor signature missed two existing profile callers and a root accessor
was private. The repair preserves those callers and checks the fixture's places
without editing browser-owned modules. No passing claim is made for that run.

On checkpoint `ac4807f4`, `20261018T143055Z-7c24c3cf` passed both traversal tests
(2 passed, 0 ignored). A deliberate isolation negative control then admitted the
unrelated disposable folder as another initial root; run
`20261018T143207Z-b74054c6` failed both initial-root assertions, as intended
(0 passed, 2 failed). Its patch SHA-256 is
`12cc53d9a36655671ea65646b2a231576ad16aada3a57f0ca917ad4b04bc2b3a`.
It accessed only the disposable fixture, and was restored after the run ended.
This is a sensitivity control, not a claim to recreate the old Mac stall.
Original logs, failed receipts and patch bytes are retained for handoff.

Final canonical-head focused/browser compatibility, strict feature Clippy and
quick guard results belong in the PR/Pulse handoff with original summaries and
hashes. These partial checks do not replace the dedicated merger's final
prospective-tree full local CI. No hosted CI or self-merge is requested.

The finite coordinator-only native follow-up is the two tests selected by:

```sh
cargo test -p hex-ui --features ui-test-support --locked operator_tab_cycle -- --nocapture
```

Require two positive test lines, zero ignored, both complete traversals, default
native stack and the actual immutable source identity. Use only the disposable
fixtures; no installed-app launch, real library/settings changes, backend/input/
permission operation or listening claim. Root owns scheduling; this cloud task
performs no Mac work. Native qualification of the new fixture remains open until
that exact-source result is returned; no general suite or MTS rerun is implied.
