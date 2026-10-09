# hex-widgets

## Purpose

Reusable, compact native GPUI controls for Hex, including numeric knobs and
sliders, toggles, selectors, and device frames. Numeric controls expose typed
gesture events; callers own canonical state, audio commands, persistence, and
undo policy.

## Dependencies

Depends only on GPUI. Control colors, labels, values, units, and callbacks are
provided by callers. Text inherits the active GPUI font so the application can
apply its bundled Ioskeley Mono theme.

## Use

Create one persistent GPUI entity per control, pass a validated `NumericSpec`
and `ControlStyle`, render that entity in the device layout, and subscribe to
`NumericControlEvent`. Call `bind_numeric_control_keys` once during app setup
to enable focused text editing and arrow-key adjustments. Specs support linear,
positive logarithmic and zero-centered bipolar scales, optional units, and a
caller-provided formatter/parser pair. Toggle and selector entities emit their
own typed change events. `device_frame` accepts a title, style, header actions
and body element; it does not impose device-specific layout or parameter
semantics.

Numeric event phases are `begin`, `change`, `commit` and `cancel`. The widget
keeps a responsive local draft but does not queue audio work, merge gestures,
or create undo entries; subscribe at the UI/controller boundary and forward
events through the project's bounded command protocol.

## Does not own

Does not depend on or construct Hex projects, audio engines, DSP devices, session
workers, plug-in hosts, or application-specific theme state. It does not claim
that provisional control laws reproduce Ableton Live without reference tests.

Caller-owned styles can update existing numeric/toggle/selector controls without
replacing active entities or numeric drafts. Numeric text selections derive their
tint from the injected accent and surface. Application theme persistence remains
outside this crate.
