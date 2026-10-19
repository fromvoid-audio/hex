# compressor

The registered `hex.compressor` is an original zero-lookahead, stereo-linked
feed-forward processor using the shared Lattice stateful effect contract. Its
version-3 opaque state contains seven finite `f32` controls and a detector choice (32 bytes). Generic device
metadata supplies catalog display, editing, persistence, history, and parameter
publication.

The default peak detector follows the larger absolute channel sample. Its envelope uses a
one-pole coefficient prepared from attack time while the input rises and release
time while it falls. The detector level is `20 * log10(envelope)`, with a
documented `-120 db` floor at silence. For delta equal to level minus threshold,
the uncompressed region has zero gain reduction. Above a hard knee, reduction is
`(1 / ratio - 1) * delta`. Within a nonzero knee, the original quadratic curve is
`(1 / ratio - 1) * (delta + knee / 2)^2 / (2 * knee)`. At ratio 1 it reduces no
gain. Makeup is added to the reduction target.

Current gain in dB follows the target through a control-prepared 5 ms one-pole
smoother, then multiplies both channels by the same linear gain. Runtime state contains the scalar detector envelope, gain value, current mix
and remaining mix-ramp frame count. Parameter revisions can carry
these histories between same-rate compatible instances while keeping the
candidate's new settings and coefficients. Reset restores zero envelope and the
configured makeup gain and authored mix. Bypass advances the same state while preserving the
input.

The effect declares zero algorithmic latency and a zero-frame finite output
tail: its stored detector and gain can affect future nonzero input, but cannot
produce output on their own after the input becomes zero. `is_quiescent` therefore
describes output silence, not absence of state history. Invalid input frames pass
unchanged without changing detector state so the host can report the fault.
Finite values that overflow during gain multiplication are not silently clipped.

The processor uses an original standard soft-knee law; settings and numeric
responses have not been measured against Ableton Live's Compressor. There is no
lookahead, external sidechain, detector filter, auto mode or metering
in the original DSP slice. The RMS addition is specified below. The dedicated compact panel uses shared native controls;
native pointer and listening acceptance remain open.

## verification

The device tests cover known knee-law points, a one-sample detector/gain
step-response oracle, stereo linking, ratio-one behavior, malformed metadata,
partition invariance, reset, bypassed state advancement, invalid inputs, and
history copy/retarget compatibility. Engine and storage tests exercise catalog
resolution, ordered track/send/Return rendering, bypass, live parameter changes,
undo/redo, and opaque save/load. The audio callback allocation probe checks
initial and edited revisions, observes routed PCM, and measures admission plus
render for zero total and signed-net allocations. These source tests do not
establish native UI or listening acceptance.

## compact panel — pulse2314

The 326px panel uses the shared rounded fixed-height frame and shared editable
controls. Ratio, attack and release form the left column; threshold and manual
output (makeup) are inline fields above the curve, with knee and a percentage dry/wet field below. Ratio uses
the shared typed `n:1` display/parse codec; plain numeric entry remains accepted. The read-only
static curve comes from the actual original gain-reduction law at steady detector
levels. Input is -60..0 db and output is -66..24 db, including makeup, the amplitude-domain dry/wet blend and the soft-knee
dip at the lowest threshold. It is explicitly a static preview, not a live
gain-reduction meter or timing display. No unsupported modes or lookahead
controls are drawn. Track and Return instances share the same body.

Curve samples are bounded and cached by device state and effective local values;
drafts update before worker acknowledgement. Owner/project/removal retire cached
state. Headless tests cover Track/Return dimensions, a real Undoable parameter
drag and pending-draft retirement; visual/listening acceptance awaits the user.

## parallel dry/wet — pulse2851

Stable provider parameter 7, `dry/wet`, is a finite fraction from 0 to 1, default
1. For original device-input sample `x`, compressed post-makeup sample `w`, and
current mix `m`, output is `(1-m)*x + m*w`, computed in double precision before
conversion to PCM. This is an original linear-amplitude law, not a measured Live
law. At exactly 0 the original input is retained bitwise, including signed zero;
at exactly 1 the original fully-wet operation order/rounding is retained. Host
input trim precedes the device and output trim follows it, through unchanged
host processing. Detector and gain history still advance while fully dry or
bypassed. Invalid input retains the existing visible fault behavior.

A compatible parameter revision copies the current mix and ramps to its new
target over `ceil(sample_rate/200)` valid frames (at least 5 ms). The finite ramp
reaches exact endpoints. Same-settings speculative/scratch history copies also
copy remaining frames, so block boundaries and rollback do not restart the ramp.
Fresh/reset runtimes start at authored mix. No callback allocation, queue, extra
PCM buffer or host registry is introduced; only scalar runtime state and math.
The static curve and effective pending-draft cache use the same post-makeup blend;
it is a steady target preview, not a live detector/ramp/meter display.

Codec versions 1 and 2 are explicitly unsupported, including prior alpha payloads; no
historical conversion or automatic user-file rewrite is provided. Current state
continues through generic opaque preset/project/history operations. Independent
current payloads carry mix without a provider-specific A/B model. Actual combined
A/B UI acceptance requires the generic A/B integration, absent from the source
base for this slice; no unmerged A/B branch is imported here.

Focused verification covers dry/wet/intermediate PCM with nonzero makeup,
asymmetric stereo and signed zero, original fully-wet PCM fingerprints at six
rates, analytic mix ramps and irregular speculative twin partitions, reset and
bypass/dry history, current codec/host-trim preservation and invalid/legacy
refusal. Actual canonical Track/Return tests exercise rendered mix gestures,
Change/Commit/Cancel, draft preview, Undo/Redo and disposable current save/open.
These source checks do not establish native musical listening or full Compressor,
W12 or Ableton parity. Root owns future finite native control/listening checks;
sole merger owns final prospective full local CI and merge.

A disposable, non-repository allocation-counter 0.8.1 fixture measured zero total
allocations/bytes and zero signed-net allocations/bytes across process, bypass,
reset and history transfer for six sample rates and three mix values, with an
allocation-positive/net-zero sensitivity calibration. The existing actual routed
Track/Return callback allocation test also remains green at default mix. This is
finite source/runtime evidence, not a universal native audio deadline claim.

## peak / rms detector — focused w12 slice

Choice parameter 8 selects peak (default) or rms. Codec 3 contains the seven
existing finite float controls plus a checked `u32` mode, 32 bytes total.
Unsupported alpha versions 1 and 2 are refused, without conversion or file
rewriting. Dry/wet parameter 7 and its post-makeup law remain unchanged.

This is an original Hex law: each channel's mean-square history uses a fixed
10 ms exponential integration time, `a = exp(-1/(0.010*sample_rate))` and
`E[n] = a*E[n-1] + (1-a)*x[n]^2`. RMS detector input is
`sqrt(max(E_left,E_right))`: channels are averaged independently before taking
max, not summed or combined by instantaneous sample maxima. It is not a finite
rectangular-window RMS or a loudness meter. Periodic low-frequency signals retain
energy ripple. The fixed measurement time adds response lag before the existing
attack/release amplitude follower; it adds no audio delay or lookahead.

Peak keeps `max(abs(left),abs(right))` and its original arithmetic fast path.
Both energies stay warm in peak mode, fully dry output and bypass. A mode edit
interpolates the detector input over `ceil(sample_rate/200)` valid frames, with
exact endpoints, before the unchanged amplitude follower, knee/ratio, makeup,
gain-dB smoothing and parallel blend. The amplitude envelope is never reinterpreted
as squared energy. History transfer copies energy, current mode fraction and
remaining transition frames; repeated same-target speculative copies do not
restart the ramp. Rapid edits start at the current fraction. Reset clears energies
and envelope and uses authored mode immediately. Invalid frames advance no state.

The static curve's input axis is selected detector amplitude level referenced to
1 (dBFS), and output uses the corresponding steady constant-gain level after
makeup and dry/wet. Constant DC calibration gives equal peak/rms transfer curves;
there is no arbitrary 3 dB threshold correction. The curve does not predict a
waveform's peak level, time envelope, low-frequency ripple, startup or mode ramp.
Its caption follows effective pending mode. The existing 326px panel retains all
seven controls, adding a shared detector selector and reducing only plot height.

The [official Live 12 manual](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/#compressor)
documents peak/rms functionality and qualitative transient differences. It does
not establish Hex's integration time, stereo aggregation, transition law or
numeric equivalence. No native reference or listening parity is claimed. Sidechain,
lookahead, auto release, alternate follower modes and full Compressor/W12 completion
remain outside this slice. Combined generic A/B requires its actual integration
and tests; no provider-specific bank owner is introduced.

## combined generic a/b acceptance

The merged shared comparison UI and Service APIs have now been exercised with
both detector ID8 and dry/wet ID7 in distinct authored banks: A uses Peak with
25% mix; B uses RMS with 75% mix. Track and Return cases use the actual detector
selector, numeric mix field, card comparison menu and keyboard switch. Both copy
directions retain the active slot; switch, copy and preset load each have exact
one-edit full-project Undo/Redo checks. Current-format project save/open and preset
save/load preserve distinct banks, active variant, complete provider payloads and
global host input/output trims. Reopening through the same controller exercises
project-generation replacement rather than merely decoding an independent clone.

After each bank operation, the actual rendered detector caption must match the
selected bank and the opposite caption must be absent. The retained transfer
curve must equal the provider curve for that bank. Different mix values make a
stale curve observable; Peak and RMS deliberately share the static constant-level
calibration, so the tests do not invent a mode-dependent 3db shift. These are real
headless UI/worker/persistence checks, not native pixels or listening acceptance.
No production changes were required for this acceptance supplement. Original
Peak/DryWet PCM, RMS history, 5ms transition and allocation evidence is retained;
native behavior, listening and full Compressor/W12 parity remain open.

### Silent RMS energy retirement

After the unchanged per-channel exponential update, energies below `1e-30`
are set to exact zero. This power floor corresponds to -300 dBFS detector
amplitude, 180 dB below the existing -120 dB silence reference and far below
the minimum threshold/knee region. It prevents persistent `f64` subnormal
histories during digital silence, including warm Peak, fully dry and bypass
paths. The original recurrence above the floor, Peak arithmetic fast path,
10 ms integration coefficient and 5 ms mode transition remain unchanged.
Subfloor input increments are discarded; this is a local numerical floor,
not a noise gate, new control, global floating-point mode or codec change.

Real provider tone/impulse-to-silence tests cover six rates and eight processing
paths; independent exponential crossing, history/partition/reset and calibrated
allocation checks accompany the original PCM/analytic/persistence/A-B controls.
Observed subnormal persistence is source/runtime evidence, not proof of a native
callback deadline miss or an audible glitch. Native listening and optimized
hardware performance remain separate acceptance work.
