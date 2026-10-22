# Operator enable badge reference

On 11 October 2026 the user supplied four crops, retained privately under
`research/private/operator-enable-20261011/` in the PR156 worktree. They show
focused-enabled, focused-disabled, unfocused-enabled and unfocused-disabled
badges. Their source reference context is Live12.4.6; no fresh executable
identity, scale, project fixture, audio export or numerical measurement is
established by these crops. Do not infer it from earlier inventory metadata.
No Live UI automation or production-set changes were performed for this task.

The [official Live12 Operator manual](https://www.ableton.com/en/live-manual/12/live-instrument-reference/#operator)
was consulted11October2026. Osc On is an oscillator on/off control. Oscillator
signals can act as output or modulate another oscillator. The manual also says
turning oscillators off does not save CPU power. It does not define sample-level
disable transitions, recurrence retention, phase/envelope progression during
disable, re-enable during a release, or exact smoothing coefficients.

## Observations and confidence

High confidence from the direct user requirement and crops: enabled fill is
oscillator-colored, disabled fill dark; focused border white independently.
Focused-disabled includes a matching oscillator row. High confidence from the
manual: the badge represents a real musical enable control, not selection alone.
There is no measured audio parity evidence in this observation.
