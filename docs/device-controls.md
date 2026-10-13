# Native lattice device controls

Pulse ticket 44 owns the reusable GPUI toolkit, ticket 65 owns bounded live
parameter gestures and grouped undo, and ticket 17 owns the real device panel.
The first toolkit implementation is source-complete: 14 targeted widget tests,
strict crate Clippy, architecture/style guards, formatting and desktop build
pass. All 14 full local CI steps passed on clean `7f0bdea` in 163.297 s
with unchanged source digest and `stale=false`
([PR #24](https://github.com/PORTALSURFER/hex/pull/24)). Native acceptance
remains open.

## Boundary

`hex-widgets` owns knobs, horizontal/vertical sliders, editable numeric values,
toggles, selectors and compact device frames. It depends on pinned GPUI and std,
with no canonical project, DSP, storage or device-I/O dependency. The caller
supplies validated display specifications, formatting/parsing, lowercase text,
style and event bindings. App composition inherits the verified Ioskeley Mono
font and routes text through `hex-ui::theme::text` before passing it to widgets.
Inject colors so the planned four-color theme does not require a second palette.

Control entities retain local draft values and focus across renders. They emit
begin/change/commit/cancel events; they neither write files nor create history.
Device descriptors drive general controls; adding a device does not add storage
branches or require a competing UI editing model.

Device metadata may report a parameter unavailable for its current validated
state; the generic default enables valid parameters. This availability controls
the UI affordance only: canonical parameter patching and automation may still
address the stored value, so authored values remain available while inactive.
Utility disables width, mid-side and mid-side balance in left/right channel
mode, then re-enables them in stereo/swap without resetting those values.
Malformed states or availability errors are shown as unsupported device state
rather than silently enabling a control.

The device panel resolves a bounded registered-type catalog for its add menu
and a complete descriptor/value/availability snapshot per changed device state.
The control-side engine API validates availability against current metadata;
the UI caches this snapshot by device state rather than resolving it on each
paint. Optional device-owned display names are presentation metadata; stable
type IDs and opaque persisted state are unchanged. Unknown or unsupported
states report errors instead of displaying defaults.

## Interaction contract

- Validate finite ranges/defaults and typed input. Linear, positive logarithmic
  and zero-centered bipolar mappings have explicit domains and inverse mappings.
  Clamp pointer adjustments; refuse invalid typed values without changing state.
  Bound numeric text and IME replacements before shaping. Modifier changes
  during a drag must preserve the value at the transition.
- A stationary click on a value enters text editing; dragging adjusts it after
  a movement threshold. Knob double-click resets to the supplied default.
- Capture an active pointer gesture through release outside the control bounds.
  Escape cancels; Enter commits text; focused arrow keys adjust values. Text
  selection, clipboard and IME remain ordinary focused input behavior.
- Supply immediate local feedback. A stale external value must not overwrite an
  active draft; the controller handles canonical/audio acknowledgement identity.
- Keep compact knobs, thin slider tracks with usable hit regions, small value
  rows and contiguous device frames. Preserve readable text and visible focus.

Shift fine adjustment is an initial original interaction policy. Exact Live
transfer laws, modifier behavior and numerical increments are unverified until
reference experiments establish them. The visual target is the user-supplied
Live device screenshot; rendering and assets are independent implementations.

### numeric text shortcut ownership

An active numeric text draft advertises `hex_text_editing` alongside its numeric
control context. Editor printable shortcuts (space, `s`, Shift-`s`, and `l`) are
excluded while that context is focused, so unit strings retain their characters
without changing transport, Solo or loop state. Direct Solo actions also refuse
that text-edit context. Numeric clipboard, selection, Enter/Escape and existing
modified-command finalization paths remain unchanged. Focus on a numeric control
outside text editing still permits the normal Solo shortcuts.

Rendered editor-to-worker regressions cover lost characters, canonical Solo and
history isolation, uppercase/space, clipboard/cancel, owner finalization, Enter
and single Undo. Linux headless tests do not establish physical macOS keyboard,
IME, bundled-font rendering or native visual acceptance.

### linear-amplitude gain presentation

Parameter descriptors may declare `LinearAmplitudeDecibels` when their
canonical Float is a finite, nonnegative linear amplitude. The stored Utility
gain, DSP preparation, history, and project state remain linear amplitude;
generic controls display and parse dB from that metadata. Exact zero displays
as `-inf db`, unity as `0.00 db`, and the Utility maximum as `+35.00 db`. Typed
`-inf db` is the exact-zero sentinel; malformed/non-finite input, nonzero dB
values that underflow the canonical f32, and values beyond the descriptor range
are refused. The descriptor presentation contract does not add a project schema
field or a Utility-specific codec.

The Float descriptor's bounds, default, edit events and unit still describe
canonical values (`linear` for Utility GAIN). `ParameterPresentation` selects
the generic display/input conversion; it does not rewrite those domains.

For a control whose range crosses -18 dB, the current original pointer curve
allocates the lower 25% of normalized travel to a linear-amplitude ramp from
the minimum to -18 dB, then maps the upper 75% linearly in dB to the maximum.
Ranges wholly below -18 dB remain linear amplitude; ranges wholly above it use
linear dB travel. Keyboard arrows step 1 dB at and above the split, Shift-arrows step
0.1 dB, and the lower range uses the existing normalized step sizes. These are
explicit provisional interaction choices, not measurements of Ableton Live's
control law. The Live 12 manual documents Utility gain's range and 1 dB arrow
steps above -18 dB, but not this pointer curve or fine increment.

## Controller dependency

The parameter actor admits one active gesture per project. Transient changes
prepare an overlay on the latest canonical state without mutating history.
Commit includes the final typed value and performs one prepared canonical edit;
cancel restores the latest canonical sound. Independent parameter edits are
retained. Preview source identity must distinguish project generation, gesture
token and sequence from canonical revision. Coalescing cannot discard terminal
commit/cancel or weaken bounded queue ordering. Save/export finalize before
capture; Undo finalizes and then undoes. Target deletion/project replacement
cancel the overlay. Ticket 65's controller is source-complete in
[PR #25](https://github.com/PORTALSURFER/hex/pull/25), with all 14 clean-commit
CI steps passing on `d48ad24`; panel binding and native acceptance remain open.
Close and Quit must also protect a changed local draft whose canonical project
is still clean, including a retained terminal request. Existing Cancel and
Discard decisions remain authoritative.

## Acceptance

Source checks cover mappings, malformed numbers, drag cancellation/commit,
focused text input and actual rendered control events, followed by clean local
CI. Native device-panel interaction and listening belong to the user's review
build: fine adjustment, click-to-type, reset, outside-bound release, keyboard/IME,
undo, reload and compact minimum-window geometry. Automated checks do not prove
native input feel or measured one-to-one Live compatibility.

The targeted suite also covers fine-adjust modifier transitions, exact logarithmic
endpoints, reset event ordering, disabling an active drag, focused shortcut
consumption, 256-byte text/IME limits and UTF-16 marked-range conversion. These
headless rendered checks do not replace the native checklist above.


## panel integration in progress

One compact bottom detail slot switches between closed, clip and devices while
retaining selection. The focused track supplies the ordered registered chain.
Controls are cached by stable device/parameter identity with subscriptions held
once; complete validated parameter snapshots are cached by immutable device state.
Only implemented registered devices appear in the add menu. Unknown/malformed
state reports an error instead of default-resetting it. Generic float controls
are the first panel; Utility-specific layout and stateful controls follow in
ticket 18. Linear-amplitude dB presentation is separately tracked in Pulse2151.

Validate widget construction specifications before GPUI's entity factory, then
consume a private validated configuration infallibly. Invalid data must be reported
before construction, not converted into a disabled/default control. Explicitly
finalize valid typed numeric drafts before save/export/history barriers or panel
transitions; refuse invalid text and retain the field. Close/Quit protects local
text drafts even before actor admission. Cancel keeps them; explicit Discard
remains possible. External target deletion/project replacement cancels obsolete
controls instead of committing to an absent target. Preserve a terminal's local
value until the gesture and queued worker/barrier requests are idle, so an intermediate
acknowledgement cannot retire a newer optimistic value. A pointer drag that returns
to its origin still sends its terminal without creating history; bipolar midpoint
values use canonical positive zero. Owner finalization prepares and admits a typed
value synchronously before clearing its draft, retaining it on queue refusal.

Source verification passes: 300 rendered UI tests (two existing ignores), nine
panel tests and 18 widget tests, plus strict UI/widget Clippy, architecture/style
guards and formatting. These include typed-value save/reopen/undo, clean-base
close protection, refused admission retry, deleted targets and optimistic
acknowledgement ordering. Full clean-commit local CI and native review remain
separate acceptance gates.

### fixed-height chain geometry

The bottom device panel is fixed at 213 px. Its header, top border and 3 px chain
padding define the usable chain height; regular device frames are 286 px wide and
derive their body height from the shared frame header and border tokens. The
add-device menu overlays the chain, so opening it does not resize the panel or
device cards. The chain itself keeps its horizontal scroll. Native wheel and
touchpad feel still requires review.

The built-in Utility uses a 144 × 184 px rounded device frame inside that rail,
with the shared 4 px corner radius, a 14 px compact header and 12 px header
actions. Its two 64 px columns reproduce Live's compact input/output control
composition: channel and polarity, width or mid/side, mono and bass mono controls
on the input side; gain, balance, mute and dc on the output side. Bass mono cutoff
and its headphone audition control share one compact row. Compact buttons and
readouts are 14 px high, with 26 px dial rings and 8 px compact labels. The
168 px body fills the frame below its header. Right-clicking width opens the
shared editor popup to switch width and mid/side; unavailable left/right modes
disable the width and mid/side controls without changing their stored values. These
controls reuse the same parameter entities, edit gestures and history path as
other lattice devices. Numerical/audio parity with Live remains unclaimed.


## panel native review checklist

Use a fresh identified review build; source tests do not establish these native
results. Select a regular track and open the devices tab, then add Utility from
the registered add menu. Check real gain/balance/width/channel/polarity/mono/mute
changes while playing an original asymmetric stereo source.

- Drag a value, including release outside the control/window; check immediate
  display, audible application and one Undo back to its prior value. Return a
  drag to its starting value and switch tabs; no gesture should remain active.
- Click a value to type, use Shift fine adjustment and keyboard focus, and save
  without first pressing Enter. Reopen and check the final value. Invalid text
  should refuse save/hide and stay editable. Cancel and Discard close choices
  should retain their defined behavior, including a clean-base local draft.
- Add a second instance; reorder, bypass, remove and Undo. Check that controls
  follow stable instances, and parameter Undo updates toggles/selectors visibly.
- Switch focused tracks, Arrangement/Session and clip/device tabs. Preserve clip
  selection and MIDI focus behavior; obsolete targets must not retain drafts.
- Check the 880×540 minimum window, browser resizing and horizontal device-chain
  scrolling. Keep the track region useful and fields readable.

Utility DC/bass mono and Operator remain separate source and reference
acceptance gates. The new gain presentation has source coverage only; record
native numeric entry, drag law and audible results in Pulse instead of inferring
native correctness from source test counts.

### Browser insertion and ordered-chain drag

The left browser's devices category lists only registered native definitions and
starts metadata-only drags; it does not construct DSP. Dropping an effect on a
card's left or right half inserts before that stable device ID, while the empty
chain tail appends. A browser instrument drop always targets the front position
and is refused when the focused track already has an instrument; replacement
remains an explicit menu action. Dragging an existing card by its header grip
moves that stable device identity to the indicated position, including across
tracks. The rail renders the project's canonical ordered chain, and drop hit
regions do not add spacing or resize the fixed-height panel.

Drops are bound to the source project generation and revision. The controller
resolves the currently focused track, validates class, source identity and
insertion anchor, and refuses while a parameter edit is pending or active; the
user can retry after the edit settles. Stale, canceled and outside drops clear
the preview without publishing history. The Session actor repeats the generation,
revision, active-gesture, target and anchor checks before applying the typed add
or move command. These contracts are source-tested; native pointer feel and
cross-track behavior remain on the user review checklist.

Starting a native-device drag from the browser reveals the device pane and
selects device details so the drop receiver is visible, even when the pane was
closed or clip details were active. This view change preserves the focused track
and clip selection. A drag with no valid track still shows the truthful empty
device pane and cannot create a track implicitly. Releasing without a target or
canceling clears the drag but leaves the pane open; ordinary track focus keeps
its existing visibility behavior.

All native instrument, effect and Return frames use `hex-widgets`' shared
`DEVICE_CORNER_RADIUS` (4 logical pixels). Compact headers share the same frame
renderer; no device chooses its own outline radius. Header backgrounds round
their top corners explicitly because GPUI overflow masks are rectangular.
Numeric readouts share `CONTROL_CORNER_RADIUS` (3 logical pixels). Frame styling
belongs to this library; device modules own parameter composition and bindings.

### shared Lattice visual system

Utility is the reference testbed for the native Lattice look and feel. Its
parameter composition is device-specific; its frames, knobs, sliders, editable
values, switches, selectors and popup behavior belong to `hex-widgets`. Improve
those reusable controls rather than introducing Utility-only appearance paths.
All native devices inherit library styling and interaction improvements; new
device panels only compose library controls and bind validated descriptors.
Operator uses a 760px compact composition inside the fixed-height 213px device
rail. Its four colored oscillator strips stay at left, the envelope/oscillator/
routing detail occupies the center, and four compact LFO, filter, pitch and
global rows occupy the right. The amp-envelope page opens by default; choosing a
different oscillator preserves that page. Its editable curve and six values
remain canonical device controls with gesture, Undo and persistence behavior.
The Operator shell's coarse/frequency, fixed and level controls remain
canonical. Its four fine controls and ten right-shell dials use the shared
`MicroDial` widget and hold bounded per-device view state only. Right-shell
toggles and selectors update their local display state and never submit project,
history, save or audio requests. These widgets await DSP and canonical binding
under Pulse2325 (LFO),2326 (filter),2327 (pitch and fine tuning), and2328
(global controls); they do not claim those behaviors are implemented.

The global algorithm icon opens the routing page, where the canonical algorithm
selector is mounted once alongside the source/target/output matrix. All 77
supported descriptors remain reachable; the four diagonal FM routes remain
intentionally absent. The oscillator page keeps the selected waveform cycle
preview at full detail width and exposes its existing waveform, phase, velocity,
feedback and fixed-multiplier controls. Fixed-frequency shell rows preserve the
ratio and multiplier values. Preview state survives owner changes, cancels
active drafts/popups while hidden, and retires on device removal or project
replacement. Native visual acceptance remains with the user; none of this
establishes Live visual, sound or control-law parity.

The reference pass in Pulse2332 uses 12px A/B/C/D selection badges, 9px fixed
markers inside 14px hit regions, and open 270-degree shared dial rails with a
short value arc and needle. Micro-dial labels sit above the dial, with smaller
values beside its lower edge. Idle readouts have no field border; focused and
invalid editors retain their visible feedback. Rectangular value fields scrub
vertically (up increases, down decreases); horizontal slider tracks keep their
horizontal gesture. Click-to-type and fine sensitivity remain available.
Compact selectors fit their 14px
frames; the filter slope uses directly selectable 12/24 segments. LFO/filter
rows reserve a selector column, while pitch/global reserve only a narrow icon
column beside three dials. Those preview controls still own local state only.

The center display uses a dark background and flat square-marked page tabs
beneath the plot. Envelope labels sit above the supported six values. Times
format as ms/s and amplitudes as dB without changing their pointer mappings or
canonical units. The plot samples the existing original linear attack and
exponential decay/release laws; it does not add a new envelope law. Unchanged
geometry is cached, and fixed exponential factors avoid per-redraw powers.
Pointer Change also updates retained graph/readout entities in the input turn,
without requesting a full editor rebuild per movement. Terminal/cancel/error
paths retain existing redraw and gesture behavior.
See [reference comparison](../research/observations/operator-visual-002.md) for
provenance, measured scope and remaining appearance/latency gaps.

Pulse2337 uses a fixed three-stage plot informed by the user's maximum/minimum
Live screenshots. Attack contributes0–1/3 of the width, decay contributes0–1/3
after the attack endpoint, and release occupies the final third from2/3 to1.
The sustain line spans the remaining gap. At maximum20/60/60s the nodes land
at1/3,2/3,1; minimum0/1ms/1ms places attack/decay at0 and release at2/3.
Attack changes move decay cumulatively; release remains anchored in its section.
There is no automatic zoom, fit, rescale or recenter, and no explicit zoom action
is needed to reach the full supported range. An original invertible logarithmic
display mapping makes short times usable: attack uses a1ms log1p pivot, while
decay/release map logarithmically from1ms to60s. These intermediate coordinates
are not measured Live numerical parity; native comparison remains open.
Pointer feedback, gesture cancellation, outside release, Undo and save/reopen
still use canonical parameter gestures. See [Operator state](operator-state.md)
for legacy normalization. [Live's manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)
documents breakpoint dragging but does not specify this intermediate mapping.

Reusable density variants are permitted; variants must not encode a particular
device identity. Native visual acceptance of the shared system remains with the
user. Manual review should check the three-column arrangement and right-shell
spacing at the fixed rail height, reach the amp envelope and routing pages, and
confirm local right-shell controls remain responsive after switching tracks.

Auto Pan uses the same shared rounded frame and `CompactDial` entities for its
amount, free rate and right phase on both regular tracks and Returns. Its fixed
144 × 184 px frame stacks the three controls inside the existing device rail;
canonical gestures, typed values and Undo continue through the common device
controller. No preview cache or device-specific interaction path is required.

The Operator oscillator page also shows the selected built-in waveform's ideal
single-cycle shape and exposes per-oscillator velocity sensitivity as a
percentage; level descriptors use the shared linear-amplitude dB presentation.
The cycle is a compact shape preview, not a display of the band-limited table
or a promise of identical spectral output. Changing oscillator/page selection
finalizes numeric drafts and moves focus out of hidden selector popups. Invalid
numeric drafts keep their current page and prevent the fixed-frequency switch
from hiding an invalid ratio field. Per-device page state is cleared on project
replacement and device removal.

## knob reset lifecycle — 11 October 2026

Pulse2319: a shared knob double-click closes pending text editing, discards
invalid input/IME selection and releases a pending pointer press, including when
the canonical control value is already the descriptor default. The reset emits
one normal Begin/Change/Commit only if it changes the value. The first click's
existing focus-blur commit of valid text remains separate; reset does not undo
that completed edit. Subsequent arrow steps and caller value synchronization
remain available. This applies to every Lattice knob without device-specific
code. The rendered regression covers valid and invalid input, no-change reset,
post-reset movement, external synchronization and keyboard adjustment. Native
pointer sanity acceptance remains for the user; this is not an Operator playback
stop diagnosis.

### value-field pointer lifecycle — 11 October 2026

Pulse2340 / [PR155](https://github.com/PORTALSURFER/hex/pull/155) keeps the arrow
pointer over shared rectangular numeric fields, including focused text editing.
The insertion caret, validated typing, vertical adjustment, Shift precision,
reset and canonical gesture/Undo protocol remain available. A stationary click
never hides the pointer. Crossing the existing three-pixel drag threshold hides
it for value-field adjustment, including horizontal movement at an unchanged
value and adjustment at a domain bound. Knob/slider control surfaces retain their
existing pointer styles.

The frame-owned GPUI cursor override restores on release (including outside the
window), Escape, lost button state, disable, focus/window/application loss,
widget removal and window removal/destruction. macOS uses a cached transparent
NSCursor instead of incrementing a process-wide hide count. Removing a widget
clears the override with the next frame; deactivation and window removal restore
immediately. Other platforms currently fall back to arrow; macOS is the accepted
initial platform and this is not a cross-platform invisibility claim.

Automated evidence: all39 widget tests pass. Two new rendered regressions cover
hover/click/typing, threshold crossing without a value change, outside release,
Escape, button-state loss, disable, blur, deactivation, root replacement and
window removal. The hover regression fails with the original I-beam behavior.
Existing sensitivity, reset, text/IME validation and gesture-phase tests remain.
The test-platform readout establishes cursor delivery and override ownership,
not physical cursor appearance or native event delivery.

Native acceptance reported by the user on 11 October 2026: arrow on value
fields, hidden during dragging, reliable restoration, typing and Undo all pass.
The user explicitly authorized merging PR155. Checklist supplied for testing:
1. Hover and click numeric fields in Utility and Operator: arrow pointer, working
   typing/selection/insertion caret; Enter validates and Escape cancels.
2. Drag vertically, change Shift during drag, reach a value bound, and release
   outside the field and window: pointer hidden only during adjustment; restored
   afterward; one Undo restores the pre-drag value.
3. Repeat with Escape, focus loss, application switching, window closing and
   device removal. Confirm the arrow stays visible afterward in this and another
   window/application. Preserve unsaved user projects; use an isolated fixture or
   review app. Full local CI and native appearance/delivery are separate gates.

## shared knob artwork (Pulse2345 / PR157)

Every native knob appearance uses the shared renderer: black unfilled rail,
progressive secondary-accent fill and light needle. Explicit DialLayout selects
a lower-right readout gap, bottom gap, or center-up pan with a reset triangle.
Orientation is selected by semantics, independently of density and bipolar scale.
Ordinary knobs fill from their normalized minimum, including logarithmic and
amplitude/dB mappings. Bipolar zero is a neutral center with no fill, and signed
values fill from center toward their position. No value/DSP/gesture law changed.

Enabled focus, pointer interaction and numeric drafts use four short neutral-gray
L corners around the complete tile. Typed drafts retain caret/selection and
background; invalid drafts retain red error feedback. Numeric focus/keyboard
handling and the separately reviewed cursor lifecycle remain intact.

The MicroDial and MiniDial label-above tiles are58x40logical pixels with an18px
dial; the width follows the measured installed Live Coarse class. Mini labels
remain visible. Compact and Standard footprints remain unchanged. Device layouts
consume exported shared metrics. Operator shell/preview/frame widths are derived
from those tiles; inline routing fields retain46px. Utility Balance chooses Pan
by its display codec; other bipolar controls retain their own orientation.
Pointer capture has absolute positioning so
pressing cannot insert a flex gap or shift the dial/readout.
The isolated `cargo run -p hex-ui --example knob_review` fixture has no audio,
project or settings I/O and shows all densities at normalized minimum/middle/
maximum initially; each column remains editable. Generic density/semantic row
headings do not rename actual devices. It is a rendering/gesture fixture, not
canonical device acceptance. Exact installed Live pan/Return geometry remains
a required reference gate; the public Live12 Mixing manual supports the reset
triangle behavior, not pixel equivalence.

Reference provenance, before/after native captures, regression sensitivity and
remaining actual-device/reference acceptance are recorded in
[knob focus observation](../research/observations/knob-focus-001.md).

## Operator navigation and context — pulse2341

Envelope and Oscillator navigation now has one stable slot below the graph;
routing remains canonical and is reached through the entire global rail.
Every supported oscillator shell control, including view-only Fine, follows its
stable project/track/device context on pointer, focus and keyboard adjustment.
Canonical parameters, shared widgets and draft/history contracts remain intact.
See [the bounded reference and acceptance study](../research/observations/operator-navigation-003.md).
The harmonic painter is a separate2342feature; this layout does not claim
complete Oscillator-view parity. Full local CI and native pointer comparison pass at the documented checkpoint.
Human native Tab → Enter acceptance failed because the production editor's
Session action intercepted Enter. The rendered regression now installs actual
editor keybindings. The global rail has a dedicated Routing key context/action
for Enter and Space, plus a lighter focused background. Latest supplied crops
supersede black with a rounded neutral dark gray center and dark rectangular
rail rows separated by lighter gray gutters. Updated exact-source CI and native
acceptance remain required; PR156 stays draft. Crowded matrix readouts are
tracked separately as2352.

Human confirmed the repaired native Routing keyboard activation in gray68.
Latest visual correction keeps Operator header, border and outer surface one
light-gray color; selected/focused rows equal that surface exactly. Dark row
gutters widen to 2px while the 168px body and shared frame/control dimensions
remain unchanged. New native appearance acceptance remains required.

11 October 2026 follow-up native feedback: the human reports the connected001 appearance is generally correct, but rail/center content needs more space from section edges and the dark separator between rails and center must go. The focused correction removes the two explicit dark rail borders and adds four logical pixels of horizontal content inset to both rail rows and the center/tab content. It preserves the compact frame/body height, forty-pixel controls, two-pixel row gutters, exact outer-gray selected backgrounds, and accepted Routing activation. Strengthened rendered row/control containment checks cover the inset requirement; updated validation and native appearance remain pending.

The connected001 isolated review process also crashed during a user-reported window move. The preserved macOS report identifies bundle `org.portalsurfer.hex.review2341.connected.001a00c`, PID 72869, main-thread EXC_BAD_ACCESS/SIGSEGV at 0x120 in Core Animation animation collection/transaction flush. This proves the app crashed, not its underlying cause. No agent closed or controlled that app during the human check. Separate Pulse #2362 owns native window lifecycle investigation and a focused fix; PR156 remains draft, with no crash fix or native safety claim here.

Source verification of the inset correction: all thirteen targeted Operator UI tests pass. The strengthened rendered inset regression fails against previous connected001 runtime source (left control less than four pixels from its row edge) and passes the correction. Retained ignored evidence: `artifacts/operator-navigation-evidence/inset-before-regression.log` and `inset-after-tests.log`. Full exact-final-source local CI and updated native appearance verification remain pending; no crash safety claim. Separate crash coordinator: `01a12a7e-1c78-79f2-841e-605cae703e02`, Pulse #2362.

## device card selection — 13 october 2026

Left mouse-down anywhere in an instrument, track effect or Return effect card
selects that device. The shared frame uses the accent border/title bar with dark
title text; parameter controls keep their existing colors. Selecting another
card replaces the selection, which persists after release and redraw. Capture
routing allows knobs, toggles and drag grips to retain their own input handling.
Selection is transient editor state keyed by DeviceId; it sends no project edit
and clears when the panel owner changes, the selected device disappears or the
project generation changes. Right-click leaves the selection unchanged.

Focused rendered GPUI regressions cover card clicks, selection persistence,
removal, Return control gesture admission and owner/project retirement. Native
macOS visual acceptance remains pending: click an instrument then an effect,
click/adjust a Return effect knob, and verify only the clicked card is highlighted
while its controls still work.
