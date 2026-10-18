
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
