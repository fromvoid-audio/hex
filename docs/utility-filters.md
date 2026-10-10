# original Utility filters

Pulse ticket 18's next DSP prerequisite is an independently testable, fixed-size
processor in `hex-devices::utility_filters`. It is **not admitted to the native
device registry or desktop playback yet**. Canonical settings, state migration,
summed-track processing, captured-release ownership, panel controls and measured
Live filter equivalence remain dependencies. The first linear Utility codec and
its registered processing stay unchanged.

## finite contract

Preparation validates sample rates from 8,000 through 192,000 Hz and finite
bass-mono cutoffs from 50 through 500 Hz. It prepares coefficients outside
processing. The processor owns only fixed arrays and scalar histories; construction,
processing, reset and history transfer have no heap owners or scheduling. Finite
PCM input is a caller contract. Large signals can still overflow final f32 output;
there is no limiter or input sanitization masquerading as audio processing.

The original DC blocker is a first-order 5 Hz high-pass, independently applied
to both channels. Bass mono preserves the mid signal and applies a fourth-order
Linkwitz–Riley high-pass to side. Its audition uses the matching fourth-order
low-pass on mid, output identically to both channels. These chosen algorithms
are explicit original policies, **not measured Ableton filter designs**. The
[public Utility manual](https://www.ableton.com/en/live-manual/12/live-audio-effect-reference/#utility)
informs the controls and 50–500 Hz cutoff inventory, not exact coefficients,
order, phase or transfer functions. No proprietary code/assets were copied.

Histories use f64 direct-form physical sample history, so a compatible sample-rate
configuration change retains history while the new coefficients remain prepared.
The engine must additionally match stable track/device/type identity before
calling inheritance. Rate changes refuse inheritance and reset. Tiny recurrence
outputs below 1e-30 are zeroed to prevent indefinitely decaying histories.
Disabled modes still advance their private histories: enabling does not resurrect
old input, and fully disabled output is bit-identical to the original input.
Audition has no effect while bass mono is disabled.

Zero declared algorithmic latency means no intentional delay buffer. IIR phase,
interface latency and future graph compensation remain separate. Seek/stop should
reset state, pause should freeze it and ordinary loop wraps should preserve it;
those transport rules require runtime integration tests before admission.

## source acceptance

Eight meaningful tests cover invalid/non-finite configuration, exact neutral
samples, DC rejection/channel isolation, independently calculated frequency
response, mono preservation/stereo DC removal, arbitrary block boundaries/reset,
compatible history transfer and rate mismatch, plus bypass history and large
finite signals. Existing device tests stay passing. Allocation-free operation is
established by source inspection of this standalone processor; callback processing,
publication and retirement need allocation probes when integrated.

The release example `profile_utility_filters` measures standalone processing at
48 kHz in 128-frame blocks, 1 or 100 tracks and 1/4/16 filter devices per track.
Construction and fixture generation are outside timing; copying one track input
and a fixed checksum are inside. Twenty warmups precede 1,000 timing samples.
A scalar/block oracle runs before timing. It does not measure graph mixing,
publication, plugins, UI feedback, monitoring or a physical audio deadline.


Measured 9 October 2026 on Apple M5 Pro arm64, Darwin 25.6.0, release build:

| tracks | filters/track | p50 us | p95 us | maximum us |
| --- | --- | --- | --- | --- |
| 1 | 1 | 0.92 | 1.00 | 5.21 |
| 1 | 4 | 6.75 | 6.96 | 42.21 |
| 1 | 16 | 15.83 | 27.67 | 106.12 |
| 100 | 1 | 87.08 | 118.75 | 195.50 |
| 100 | 4 | 322.04 | 366.46 | 671.08 |
| 100 | 16 | 1294.00 | 1369.42 | 1502.83 |

The input/oracle checks pass; retained log is
`artifacts/utility-filter-profile-20261009.log`. Other development work was active,
so these single-run timings are provisional and not a comparative regression
claim. Stateful graph mixing/tails and device deadlines require separate evidence.

## runtime admission

The filters now enter shared playback through registered Utility version 2 state
and the [stateful lattice contract](stateful-lattice.md). Their prepared native
processors declare zero fixed buffering/lookahead latency; IIR phase/group delay
is separate. Native listening and
measured reference response remain open; original standalone DSP checks do not
establish Ableton parity.
