# auto pan

`hex.auto-pan` is an original free-running stereo amplitude modulator registered
through Lattice. Its version-2 opaque state contains amount (`0..1`, default
`0.5`), rate (`0.05..20 hz`, default `1 hz`), and right-channel phase
(`0..360 degrees`, default `180`), plus the waveform choice described below.
The default left Sine LFO is
`(1 + sin(2π × phase)) / 2`; the right LFO uses the same clock plus its phase
offset. Each channel gain is `1 - amount + amount × LFO`. Phase starts at zero,
is sampled before each clock advance, and wraps once per cycle.

Preparation accepts sample rates from 8 kHz through 192 kHz. It calculates the
five-millisecond live-edit ramp length before callback use. Compatible same-rate
runtime transfer retains phase and every in-flight scalar ramp. An edited amount,
rate, or right-phase target independently ramps from its current value; phase
uses the shortest circular path, with an exact half-turn choosing positive 180
degrees. The ramp advances before calculating that sample's gains, reaches its
target on its final configured frame, and the clock then advances with the
current rate. Reset returns phase to zero and snaps the scalar values to the new
prepared targets.

Bypass returns the input bit for bit while advancing finite-sample phase and
ramps. Amount zero is exactly unity. A frame with a non-finite channel is passed
through unchanged and does not advance history. Runtime state is fixed scalar
storage, callback work has no allocation or lock, algorithmic latency is zero,
and the finite output tail is zero because modulation cannot produce sound from
silence. Runtime quiescence therefore describes output only; it does not discard
clock history needed for future input.

This is a limited original effect based on public control vocabulary, not a
measured recreation of Live's Auto Pan/Tremolo. Beat synchronization, retrigger,
spin, envelope/follower behavior, and modern modes are not
implemented. Its compact panel uses the shared rounded device frame and three
`CompactDial` controls for amount, free rate and right phase. Track and Return
instances use the same 144 × 184 px composition and canonical parameter gesture
path, including typed edits and grouped Undo. The periodic waveform selector and sampled static preview are described below;
the panel does not imply synchronization or other unsupported modern modes.

Headless UI coverage checks all three controls inside the frame at the 880 × 540
minimum window size, verifies matching Track/Return geometry, commits a valid
drag as one undoable edit, checks exact typed amount/rate/phase values and a
Return phase edit, and confirms controls retire on device and Return-owner
removal. Native appearance, input feel and listening
acceptance remain open for the user's review build.


## native review — pulse2322 / pr145

Source acceptance: the focused rendered headless test passes with one matching
case, covering geometry, all three bindings, drag/Undo, typed edits and both
retirement paths. Full final-commit local CI is recorded in PR145 and Pulse2322.
PR145 originally deferred the optional static modulation preview; the later
pulse2852 slice below adds a pure sampled preview without a new cache owner.
DSP behavior and the 256-processor/memory admission limits are unchanged.

User checklist for the fresh combined build:

- Add Auto Pan to a Track and a Return; check the three labels/readouts and
  rounded 144 × 184 px frame at minimum window size and with rail scrolling.
- Drag amount/rate/phase, release outside the dial, and Undo once. Type valid
  values, try an out-of-range value, and check reset and Shift adjustment.
- Play an original stereo source through each owner, check audible modulation
  and bypass, then remove the device/Return and Undo removal.

Native pointer, typography, appearance and listening results remain pending.
The unrelated Operator stop report (Pulse2305) remains open and unreproduced.

## periodic waveform choice — pulse2852

Current codec version 2 contains the three original floats plus stable Choice
parameter 4 (`u32`), 16 bytes total. Unsupported alpha version 1 is refused;
there is no migration or automatic user-file conversion. The default is Sine.
The original Sine arithmetic, phase clock and amount/rate/right-phase meanings
are preserved, including exact zero-amount unity and finite-input bypass history.

For normalized phase `p` in `[0,1)`, the original unipolar LFO laws are:

- Sine: `(1 + sin(2πp))/2`, unchanged.
- Triangle: `1 - abs(2*((p+1/4) mod 1)-1)` (rises from 1/2 at phase zero).
- Saw Up: `p`; Saw Down: `1-p`.
- Square: 1 on `[0,1/2)`, 0 on `[1/2,1)`; original symmetric 50% duty.

All feed the existing channel gain `1-amount+amount*LFO`. These are explicit
original laws, not measured proprietary formulas. The [official Live 12 manual,
section 29.3](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/#auto-pan-tremolo)
was checked for waveform vocabulary and phase relationships; it does not specify
numeric square-duty/default laws. Installed reference metadata is not a new
native comparison. Random, synchronization, retrigger, spin, shaping, follower
and modern harmonic/vintage modes remain outside this finite slice.

Choice revisions retain the original phase and scalar ramps, crossfading five
fixed scalar weights over the existing prepared five-millisecond interval. A
rapid revision starts from current weights; same-target scratch/history copies
retain in-flight ramps. Fresh/reset state uses the authored waveform immediately.
This avoids a sudden choice switch; periodic square/saw edges are intentional
and remain sharp. A square modulator has odd harmonics and can gate/click on a
sustained carrier. This is not a band-limited or alias-free quality claim. No new
clock, queue, PCM cache, allocation or history owner is introduced.

The existing 144px panel retains three dials and adds the shared Selector plus
sampled static left/right gain plots. A bounded pure provider helper supplies 129
samples per normalized cycle through the existing Engine export facade. It does
not instantiate or run a processor in paint. Rate changes cycle duration, not
normalized geometry. The plot shows authored/effective pending targets, not live
phase or in-flight crossfade weights; connecting sampled points is a static
preview, not proof that square/saw edges are smoothed in DSP. Popup hover/dismiss
is non-editing; selecting an option uses the existing atomic canonical parameter
command and one Undo step. The original numeric gesture path remains unchanged.

Targeted verification includes independent quarter-cycle stereo amplitude
oracles, signed boundaries, original Sine PCM fingerprints at six rates,
steady-stream crossfade oracles, irregular speculative partitions/reset/bypass,
rapid retarget and exact clock retention, validation/codec and sampled-preview
checks, and finite Fourier measurements of intended harmonic content. Real
Track/Return tests cover all control bounds, selector dismissal/commit,
Undo/Redo, pending preview and disposable current-format save/open. Generic A/B
payload ownership is retained; actual combined A/B acceptance awaits its generic
integration. Native appearance, listening and Live comparison remain open under
the coordinator's single Mac lane; this does not complete Auto Pan or W12.

## retained static preview and combined a/b acceptance

The compact panel retains its provider-sampled preview in the existing device
panel state. Its key is the stable device ID, project generation, active A/B
variant, full canonical provider state and all four effective pending parameters.
Unchanged redraws reuse the curve without cloning/patching provider state or
resampling the 129 points per channel. Equal invalid float drafts compare by bits,
so an error result can be retained without displaying an older successful curve.
The existing project/owner transitions clear previews; removal or type replacement
retires entries. Pending-value retirement reveals the canonical curve immediately.
This is a private panel cache, not a second device model or an audio callback path.

A rendered regression measured eight provider evaluations for eight unchanged
notified redraws before caching and zero afterward. These are actual helper-call
counts in headless GPUI, not native frame-budget or listening measurements. Both
Track and Return tests now use the merged generic A/B UI and Service APIs with
shape ID4: triangle in A, square in B, both directional copies without switching
the active bank, exact one-edit Undo/Redo, and current-format project/preset
save/open preserving distinct banks and global host trims. Reopening through the
same controller retires the old generation and renders the correct bank. Private
cache-boundary tests cover each effective parameter, pending retirement, errors,
type replacement and removal; actual UI cases cover canonical worker completion.
The original Sine PCM/phase/5ms transition and runtime-allocation evidence remains
separate. Native appearance, input, listening and full Auto Pan parity remain open.
