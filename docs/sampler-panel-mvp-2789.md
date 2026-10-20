# sampler scalar panel component

The staged MVP panel consumes the agreed fixed array of 43 copied parameter
facts after the caller has ended its retained snapshot read. It retains no
project/device state, sample, waveform, resource capability or read stamp.
Existing parameter controls remain responsible for guarded edits and gestures.

The first panel shows source start/end and reverse, Classic loop enable/bounds,
root/transpose/detune, voices, gain/pan/velocity and amplitude ADSR. Units and
mode-specific enabled flags come from the existing metadata. Inactive modes do
not show Classic loop controls. It exposes no loading action, fake waveform,
registry admission, extra mode, filter/modulation or warp claim.

Four shared rails use existing 136 px standard numeric controls, a two-by-two
source-flag group of 64 px compact dials, and 40 px compact buttons inside a
594 px body. Source flags use the existing Number codec under the explicit
`source · frames` heading; their canonical frame values and numeric entry stay exact. Leaf fixtures build real widgets from the actual
production section recipes and metadata descriptors, then check containment at
620/1200 px window widths, numeric display strings/units, typed input events, disabled
controls and inactive loop bounds. These fixtures own no Service or media worker;
they do not establish canonical command/ACK, host playback or native acceptance.

The exact four-file scalar-query prerequisite was published as
`1d5b89a2f658a22cb9c1f76c5bfc1631326e192d` on base
`b94b0e2f6fbec0932d422c00d4584739c192cb9a` and imported onto this component branch.
It adds Engine metadata decoding and the nominal `SnapshotRead::sampler_parameters`
method without opening the registry.

Production integration remains blocked on Native's actual retained UI capability
entry and existing binding-currentness migration. The current parent still uses
legacy owning state/cache access and holds no `SnapshotRead` or read stamp. This
component deliberately has a feature-only module mount and no Sampler parent
dispatch, frame-width arm, raw-state/current-Session shortcut or invented stamp.
The device-layout guard requires the real instrument dispatch and therefore
fails while this dependency is outstanding. Dead-code strict-check failures for
the unconnected leaf are preserved; no lint or guard is weakened to hide them.

After Native supplies that entry, copy the actual device's 43 facts inside its
retained read, end the read, and invoke `sampler_panel::render_body`. Integrate the
instrument-only dispatch/width arms and revalidate actual binding events,
currentness/refusal, canonical Undo/ACK and the final guards/strict configurations.
Loading, waveform, host publication and an optimized usable build remain Native
and coordinator work. This document is a source component report, not delivery or
full Sampler completion evidence.

## source readout measurement

The initial 46 px standard numeric value field left about 40 px after padding
and borders. A headless CoreText query of the bundled regular Ioskeley Mono at
9 pt measured `16777216` at 43.2 px, so removing units alone could not make the
maximum frame coordinate fit. The source flags now use existing compact dials:
actual rendered readout bounds are 64 px, leaving about 58 px for that exact
unit-free number. No binder/widget extension, smaller font, rounded numeric
codec or changed source-frame domain is used.

GPUI's test text platform did not register the bundled family through the normal
font loader; that failed probe is retained. Leaf fixtures request the product
family but establish component geometry and numeric display strings only. The
independent CoreText measurement above uses the actual bundled font, and native
paint/text interaction remains a separate acceptance gap.
