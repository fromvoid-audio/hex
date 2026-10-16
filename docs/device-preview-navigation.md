# device preview navigation

Pulse2347 investigates a native report of an unexpected project edit and
`device preview source did not match its prepared revision` during Operator
Envelope/Oscillator navigation. The original private screenshot and optimized
macOS fixture are unavailable in the cloud; its precise native event ordering
has not been reconstructed here.

The portable reproducer adds a track with Sine, drops Operator onto that
instrument through the browser, authors ratio1.2345, switches detail pages,
focuses its rounded1.23 readout without typing, then changes page. Previously,
numeric finalization reparsed1.23 and advanced the canonical revision despite
no user input. Both blur/Enter and owner-barrier finalization now retain the
exact value for an untouched draft. Explicit typed or composed input remains
an edit, including deliberate input of the displayed rounded value. Invalid
draft refusal, pointer edits and lifecycle cancellation remain unchanged.

An audio application notification can also arrive after its gesture commits,
cancels or is superseded by a new generation/sequence/owner. Such preview
notifications are discarded without changing current state or its error.
Current-gesture previews still require matching candidate generation and
canonical source revision; mismatched current sources remain invalid and
produce the existing error. Canonical candidates and audio admission retain
their existing behavior.

Regression evidence includes the actual browser replacement and navigation,
exact canonical/revision preservation, explicit rounded-value typing and
one-step Undo, delayed application after terminal and newer owner/sequence
events, rejected candidates and invalid current-source refusal. The unchanged
numeric implementation fails the rendered revision-preservation regression;
restoring the former stale-preview error classification fails its focused
notification regression. These are source-level counterexamples, not a claim
that every event in the original native report has been observed.

Full exact-source local CI and native optimized-build retesting are separate
requirements. Native input, audio timing, reference and listening acceptance
remain pending; no DSP, codec, engine ownership or layout change is made here.
