# Device latency and responsive playback

## Required behavior and physical limits

Hex must keep device chains, tracks, parallel racks, sends/returns and sidechains
sample-aligned, while avoiding unnecessary host-added latency and transport waits.
Native devices must expose known algorithmic latency in frames or identify it as
unknown. External devices report latency to the host; the host propagates
cumulative latency through the compiled graph and presents correctly timed
audio/events to subsequent devices.

Compensation cannot remove causal processing delay, lookahead or audio-interface
latency. Shorter paths are delayed to align at a merge, or known timeline material
is processed ahead. Live already provides delay compensation; Hex's improvement
target is measured responsiveness, correct downstream timing and clear policies,
not claiming that compensation itself is a new feature. See
[Ableton's compensation description](https://help.ableton.com/hc/en-us/articles/209072409-Delay-Compensation-FAQ).

A prepared zero-latency chain should begin without an artificial compensation
pause. Arbitrary lookahead plugins processing unpredictable live input cannot
provide both their exact normal result and zero audible delay. Transport
acknowledgment, first audible sample, hardware latency and intrinsic DSP latency
must be measured and reported separately.

The native seam exposes known algorithmic frame counts or explicit unknown
through prepared effect/instrument contracts. Enabled unknown latency refuses
playback; bypass and memoryless matrices expose zero. Production graph
compensation now propagates checked cumulative arrivals and owns bounded
transactional rings, exact origin clocks, history transfer and export trim/drain.
See [runtime latency compensation](latency-compensation.md) for current limits,
stop-required transitions, test evidence and physical/native gaps.

## Host graph contract — planned

- Distinguish algorithmic latency from intended musical delay/reverb tails.
  Compensate only declared processing latency; preserve intended effects.
- Sum latency along serial paths. At each reconvergence, align incoming paths to
  the latest arrival and add the receiving device's own latency. Include groups,
  racks, sidechains, sends/returns and explicitly measured external hardware.
  Feedback routing requires a separate causal cycle contract before support.
- Native device APIs expose latency and latency changes. VST3 integration reads
  `IAudioProcessor::getLatencySamples()` and handles `kLatencyChanged` according
  to the plugin lifecycle. This is a host contract, not a new plugin-to-plugin
  messaging requirement. Source:
  [Steinberg processing FAQ](https://steinbergmedia.github.io/vst3_dev_portal/pages/FAQ/Processing.html).
- Propagate sample timestamps for MIDI, automation and transport-relative device
  timing with audio; do not fix audio alignment while leaving downstream LFOs,
  envelopes or automation on an incompatible timeline. Validate device-specific
  transport semantics against fixtures rather than assuming every plugin agrees.
- Prepare delay buffers and replacement schedules on the control side. Publish
  at a defined block boundary without callback allocation or final destruction.
  Specify transition behavior for latency changes, bypass, device removal and
  sample-rate changes; test discontinuities rather than silently shifting time.
- Avoid adding a whole buffer at each native device boundary. Any plugin isolation
  or asynchronous processing boundary must explicitly account for its extra delay
  and scheduling cost; choose isolation policy before external hosting work.

## Transport and monitoring policy — planned

Keep playback graphs, device instances and resident/streaming media prepared
before Play. Never instantiate devices, decode media or compile the graph in the
Play handler or callback. Pre-roll known timeline material off the critical Play
path where device state and source availability permit it. Invalidate prepared
state after relevant edits and seeks; do not reuse stale audio or assume arbitrary
stateful plugins can be rendered ahead safely.

When preparation is complete, start on the next eligible callback without a
fixed global compensation countdown. If exact startup needs unavailable history
or lookahead, show readiness and define the supported first-sample policy before
shipping. Do not silently drop the first transient, bypass a device or claim an
instant exact result. Start, seek, loop boundaries and rapid edits all need this
contract. Preparation is real work and must remain visible when unavoidable.

Provide an explicit low-latency monitoring mode so unrelated high-latency tracks
do not unnecessarily delay monitored input. Local device and interface delays
remain; explain any alignment tradeoff at affected merges. Never silently change
a plugin's quality/lookahead or bypass processing to satisfy a latency target.
Full compensated playback and low-latency live monitoring need distinct tested
contracts, including recording placement and hardware round-trip calibration.

## Acceptance before claiming alignment or immediate playback

Use impulses and timestamped MIDI/automation through zero-, fixed- and changing-
latency devices, serial chains, parallel branches and sidechains. Compare exact
sample positions over varied block partitions and sample rates. Test Play at
project start, arbitrary seek, loop wrap, bypass, insertion/removal, latency
changes during playback, and monitoring with a high-latency unrelated track.
Measure first audible output with a physical loopback as well as transport/UI
acknowledgment. Record unavoidable device latency separately from host overhead.
These contracts and tests are unimplemented; no current plugin-compensation or
zero-latency claim is supported by the fixed demo.
