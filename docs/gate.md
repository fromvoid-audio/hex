# Gate

`hex.gate` is an original zero-lookahead stereo-linked hysteresis gate in
Lattice. It uses a sample peak detector shared by both channels, a threshold to
open, a lower return level to begin hold, a floor attenuation, and linear gain
ramps. The defaults are threshold `-40 dbfs`, return `-45 dbfs`, floor `-60 db`,
attack `1 ms`, hold `50 ms`, and release `100 ms`.

The version-1 opaque state contains six little-endian `f32` controls. Validation
requires `return <= threshold`, bounded finite values, and sample rates from
8 kHz through 192 kHz. Preparation converts the dB decision boundaries to
linear gains and computes checked integer frame durations off the audio thread.
Per-sample detection is a linked-peak comparison with no logarithm or power
calculation. The runtime owns only fixed scalar history. Opening is inclusive at
the threshold. While open, samples at or below return advance a consecutive
hold counter; samples above return reset it. A nonzero hold keeps the gate open
for that many complete qualifying samples, then starts closing on the next
qualifying sample. The threshold check wins when threshold and return are equal.
Hold zero closes on the first qualifying sample. Attack and release reach their exact target on
the final configured frame; reopening during release continues from the current
gain. Bypass advances this state while returning input samples bit for bit.

This design deliberately uses original threshold, hysteresis, hold and linear
ramp semantics. It is not a measured recreation of Ableton Live's Gate detector,
timing, response, or visual controls. There is no sidechain, detector filter,
RMS mode, lookahead, or meter. The compact native device panel presents
threshold, return, and floor in the left column, with attack, hold, and release
in the right column. It uses shared Lattice dial controls and the same rendering
path for Track and Return devices; it adds no Gate-specific widget or alternate
parameter path. Source-level layout and gesture tests cover both owners, but
native pointer and appearance acceptance remain open.

The processor declares zero algorithmic latency and zero output tail: it scales
only the current input frame, so silence cannot produce sound. Stored latch and
ramp history is still copied when a same-rate prepared revision is admitted.
Changed parameter publication preserves unrelated history; floor changes while
closed start a bounded release-time ramp, while active attack/release edits
rebase only their corresponding ramp. The source tests and callback probe cover
these rules, track/Return routing, persistence, and callback allocation. Native
UI and listening acceptance remain open.
