# Operator navigation and rail context — 11 October 2026

Pulse2341 / [PR156](https://github.com/PORTALSURFER/hex/pull/156).
Independent native GPUI layout; reference assets are not shipped.

## finite scope and reference

The installed Live bundle was rechecked before this study:12.4.6
(2026-09-10_0de5c8fa9a), arm64, executable SHA256
`5e8832e06fe92d42ceebac4fad153e44422c90923f426dcb05c760c6d80b3949`.
The existing native screenshot in
`research/private/operator-visual-20261011/live-reference.png` in the primary
checkout and the official reference image retained by
[visual study002](operator-visual-002.md) show Envelope left and Oscillator
right below the graph, a dark central display, compact supported readouts and
four colored routing rectangles beside Time/Tone/Volume. The native capture
shows the Envelope graph, not a harmonic editing experiment. Exact numeric
transfer laws and complete oscillator content are not inferred from that image.

Acceptance for this PR is fixed section navigation, routing access from the
existing global rail, darker rail/detail surfaces, and context following every
supported oscillator-rail control. Actual harmonic painting/custom waveforms
remain Pulse2342/PR158; enableDSP2335 and activeLFO2325/PR154 are separate.
Hex retains lowercase Ioskeley Mono and its theme. Full Operator parity is not
claimed: unsupported richer envelope and oscillator settings are absent, Fine
and Time/Tone/Volume remain explicitly view-only under the current contracts.

## implementation boundary

Envelope and Oscillator now occupy one fixed16px navigation slot below the
86px graph region, with stable90px tab bounds. The routing tab is removed;
the complete bottom-right global rail activates the existing canonical routing
matrix, while its child widgets retain their gestures. Its tab stop and
Enter/Space activation have a descriptive lowercase tooltip. Existing GPUI
custom-control accessibility limitations remain; a tooltip is not proof of
screen-reader coverage.

The supported waveform preview and wave/phase/velocity/feedback/fixed-multiplier
settings use that same graph/navigation region rather than moving tabs after a
variable control stack. No painter, new waveform, new DSP or persistence format
is introduced. Existing shared widgets, parameter codecs and canonical
control bindings remain. The waveform preview uses a grid layout to give its
existing shared canvas a nonzero content box; it is read-only.

A shared context guard captures project generation, canonical track and device
identity. Pointer activation, canonical numeric Begin, Fine's local Begin, and
focus on shell controls select the corresponding oscillator; Envelope mode
stays Envelope, while other contexts return to Oscillator. Focus subscriptions
live with the device controls and are pruned on removal/owner/generation changes.
Selection never steals the activating widget's focus. Invalid canonical drafts
still block transitions and remain repairable. Hidden-preview cancellation can
revert its own unfinished draft, while stale input cannot select another owner.

## regression evidence

The two decisive rendered regressions were run against the original plan commit
`9e850045f1b002f46fe245155b3db9f4875afdef` over main`1e5b569` before the fix.
Both failed: the Envelope tab moved54logical pixels vertically after selecting
Oscillator, and clicking FineB left selected oscillatorA. Raw log is retained
privately as `artifacts/operator-navigation-evidence/before-regression.log`.
The restored implementation passes the full13-test Operator subset, including
existing invalid-draft, gesture, Undo, save/reopen, fixed-mode and envelope tests.
New coverage checks allA/B/C/D content bindings, section/switch/fixed-mode geometry,
Time-tile and keyboard routing activation, Fine pointer/focus/typed/keyboard
input, canonical state preservation and hidden-owner/reused-generation refusal.
These source tests do not establish native pixels, physical deadlines or audio
acceptance.

## native evidence and remaining gates

Disposable separately identified baseline app, built from the unchanged plan
commit, used one empty track and an Operator dropped on its Sine instrument;
transport remained stopped. The before captures are2480x1522 at2x scale in
ignored `research/private/operator-navigation-20261011/`:
`before-envelope-a.png`, `before-oscillator-a.png`,
`before-fine-b-keeps-a.png`. Existing user Hex/Live projects and unsaved windows
were preserved. Captures visibly reproduce the jump and FineB keepingA.

The baseline also showed `device preview source did not match its prepared
revision` after navigation, with an apparent canonical revision advance2→4.
That independent investigation is Pulse2347; this PR does not repair the
preview-source lifecycle. Native post-patch navigation must explicitly retest
canonical revision/history and that status before acceptance or merge.

Shared-patch integration and native keyboard acceptance remain open. Required
missing or inconclusive evidence is not treated as complete visual acceptance.
User checklist: switch Envelope/Oscillator repeatedly onA–D; use coarse/fine,
level and fixed/ratio fields; inspect correct center values; click the whole
routing/Time/Tone/Volume rail and activate it by keyboard; type/cancel/Undo/save
without unintended parameter changes. Listening and harmonic painter acceptance
remain with their owning tickets.


## post-patch native checkpoint

Source `ec871d1afc73fc6acc831fd6edc17bc6680fe6ce` passed all 16 local CI steps
in 268.373 seconds, clean and unchanged, `stale=false`. Report:
`artifacts/local-ci/20261011T085821Z-ffac53aa/summary.json`; source digest
`4a71eea7137d168b21ca530a13ef3765450444cafd11fe09b9aef12647abd946`.
The separately identified after app uses its actual `CFBundleExecutable` (`hex`),
SHA256 `3847c15e288b8921264a2bf628d7fa274f67e7191f9db6ebedd9ddbbe656a54c`.
An earlier preparation copied the binary to the wrong executable name; that run
was discarded, the correct executable replaced, and the app restarted before
these captures. No result from the interrupted preparation is counted.

After captures in the same ignored evidence directory are 2480x1522:
`after-envelope-a.png`, `after-oscillator-a.png`, `after-fine-b.png`,
`after-routing-rail.png`, `after-envelope-c.png`,
`after-oscillator-d-fixed.png`, `after-envelope-a-undo.png`.
Compared with the before captures, the navigation stays at the same graph-bottom
position across Envelope/Oscillator, A–D selection and fixed mode. Compared with
the actual Live reference, labels/order and dark graph/rail surfaces now follow
the observed layout. Hex uses its own typography, wider compact fields and
limited supported controls; exact Live pixel/content parity is not established.

Pointer checks selected B from Fine, C from coarse, D from fixed, and A from
level, and opened Routing through the Time rail. The empty fixture was created
by adding one track, removing its default Sine, then adding Operator. Canonical
revision stayed 3 through view-only navigation; fixed D intentionally advanced
it to 4 and Undo to 5. No preview-source error appeared in this run. The owned
session log is retained as `after-session.log`. This does not close Pulse2347
or establish why the baseline error occurred.

Native keyboard input remains inconclusive: an Up/Tab/Return automation batch
opened an unexpected discard dialog (cancelled), and a separate Tab/Return
attempt did not visibly open Routing. A simultaneously started unrelated review
app was detected in the diagnostic inventory; its log is not this fixture's
provenance. Headless GPUI keyboard/focus regressions pass, but a human check in
the isolated app is still required before this task's native gate is accepted.
No save/reopen or listening acceptance is inferred from these pointer captures.
The existing matrix unit readouts visibly crowd adjacent cells; separate
Pulse2352 owns their compact formatting, without expanding this navigation PR.
PR156 remains draft with required native keyboard acceptance pending.
