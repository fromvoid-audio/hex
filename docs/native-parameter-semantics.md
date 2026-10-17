# native parameter semantics

Pulse #2674 / PR290 adds optional state-aware declarations to the existing
metadata contract. `ParameterDescriptor` retains canonical units, domains,
defaults and presentation; existing literals, IDs, codecs and parameter values
do not change. `parameter_semantics_for_state` defaults to `None`, meaning
undeclared. It never guesses linear normalization, editability or runtime support.

Control consumers use `validated_state_parameters` or
`validated_parameter_semantics` with a migrated, codec-validated current state.
They validate dynamic membership and every declared semantic/domain combination.
No helper constructs defaults, recurses through a registry or runs on the callback.
Unknown and deleted IDs refuse rather than resolving by vector position.

The declaration separates read/write access, normalization, stepping, automation,
modulation, smoothing and change impact. Read-only differs from a temporarily
ineffective control: inactive authored values remain stored/editable according to
the provider's existing law. Effect/instrument registries reject read-only or
invalid declarations before single or atomic private patching. The default atomic
metadata patch also checks access; declaring providers must enforce it in their
own direct patch overrides. The conformance harness tests that refusal. No current
production provider in this slice introduces a read-only parameter.

Normalization is explicit linear, positive logarithmic, discrete or unspecified.
Canonical units are not duplicated in semantics. In particular amplitude displayed
in dB does not imply a logarithmic normalized law at zero. Public conversion
refuses nonfinite/out-of-domain values and unspecified/incompatible mappings.
Singleton domains normalize to zero and all normalized inputs decode to their sole
value. Boolean midpoint is true; choices round to the nearest index with midpoint
upward. Conversion uses finite f64 intermediates for broad canonical float ranges.
It does not replace device math, clamp external inputs or quantize an increment.
Stepping describes the input affordance; device codecs remain the authority for
valid canonical values. A finite positive increment is expressed in canonical
units, separately from normalization and display precision.

Automation distinguishes undeclared, unsupported and an existing device-owned
Arrangement envelope. Modulation currently distinguishes undeclared/unsupported;
no generic processing capability is advertised. Smoothing can be unspecified,
none or device-managed; that last value preserves the documented device-specific
law rather than claiming a shared ramp. Value-only edits still require normal
control preparation/revision publication. Structural edits require re-preparation;
metadata alone permits no direct callback mutation or restart bypass.

## first production providers and consumers

Auto Pan declares linear amount/phase and positive logarithmic rate, continuous
controls and its device-managed five-millisecond transition. Phase follows the
existing shortest circular path. The generic automation/modulation paths remain
unsupported; ordinary parameter gestures still use their existing protocol.

EQ declares frequency/Q logarithmic, gain linear dB and discrete enabled/shape/
slope/channel fields using its existing stable `NodeId`-derived IDs. Its channel
mode is structural and not envelope-automatable; global gain is also not a node
lane. Node membership is checked in the actual state. Imported legacy shelves
still refuse gain/Q/shape lanes until explicit conversion, while retaining their
authored values and existing availability rules. Discrete node lanes remain
supported by the codec/runtime even where the current envelope editor only
authors frequency/gain/Q; semantic support is not a new UI control.

EQ's existing precedence remains: a node lane supplies its field while an
appropriate running Arrangement clock is available; otherwise the authored base
field is used. Continuous lanes interpolate normalized values; discrete lanes
hold; endpoints hold outside the authored interval. The runtime refreshes targets
on discontinuity/refresh or its 64-frame cadence and then smooths coefficients.
This is not generic sample-accurate delivery. Mode changes and filter shape/history
transitions retain their previous runtime compatibility/reset laws.

`NativeDeviceParameterView` caches optional declarations alongside validated
values/descriptors. It combines read-only access with existing state-dependent
enabled status and never enables an ineffective control. Generic controls consume
that existing enabled field. Host input/output gain controls retain explicitly
undeclared semantics. The standalone device conformance report is schema 2 with
stable lowercase semantic labels and explicit `null` for undeclared providers. A
separate populated-EQ fixture demonstrates dynamic fields that the empty EQ
default cannot expose. No desktop or project is required to generate the report.

## evidence and remaining work

Focused tests cover numeric boundaries/round trips/singletons/discrete midpoint,
invalid declarations, malformed state and unknown IDs, read-only single/atomic
registry refusal before private mutation, Auto Pan declarations, agreement with
EQ's existing normalized math, stable reorder/delete/no-reuse and legacy shelf
lane restrictions. Cached engine-view tests preserve legacy undeclared controls
and disabled EQ gain. Existing registry authoring checks remain in use.

Commands and exact candidate/full local-CI evidence are recorded with PR290 and
Pulse. The generated report is parsed as JSON and checked for the populated EQ
and explicit capability fields. Metadata changes do not establish native UI,
listening, reference laws or physical deadlines.

Shared sample-offset scheduling, cross-source precedence, generic automation and
modulation runtime, dynamic rack/sample-zone addressing, and downstream W20
production/oracle qualification remain W05 work. Existing EQ automation and
one-Undo gesture/publication acknowledgments are preserved, not generalized by
this metadata bootstrap.
