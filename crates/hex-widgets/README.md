# hex-widgets

## Purpose

Reusable, compact native GPUI controls for Hex, including numeric knobs and
sliders, toggles, selectors, color pickers, and device frames. Numeric controls
and the color picker expose typed change events; callers own canonical state,
audio commands, persistence, and undo policy.

Utility is the current visual testbed for the shared Lattice device interface.
Frames, knobs, sliders, value fields, switches, selectors and popup behavior are
library controls, not Utility-specific implementations. Device panels compose
these controls and bind their descriptors; changes to shared controls propagate
to every device using them. Presentation variants describe reusable density or
control purpose, never a device identity.

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
own typed change events. `ColorPicker` accepts caller-owned opaque RGB and style,
draws a saturation/brightness field and hue strip, and emits synchronized RGB/HSV
values. The caller retains the RGB value as canonical state and may synchronize
it back while preserving a preferred hue for gray or black. `device_frame`
accepts a title, style, header actions and body element; it does not impose
device-specific layout or parameter semantics. All device frames use the shared
`DEVICE_CORNER_RADIUS` token; `compact_device_frame` changes header density only.
Header backgrounds carry matching inner rounding because GPUI's content masks
are rectangular. Numeric readouts use `CONTROL_CORNER_RADIUS`. Device modules
must reuse these library frames rather than selecting device-specific corners.

Numeric event phases are `begin`, `change`, `commit` and `cancel`. The widget
keeps a responsive local draft but does not queue audio work, merge gestures,
or create undo entries; subscribe at the UI/controller boundary and forward
events through the project's bounded command protocol.

## Does not own

Does not depend on or construct Hex projects, audio engines, DSP devices, session
workers, plug-in hosts, or application-specific theme state. It does not claim
that provisional control laws reproduce Ableton Live without reference tests.

Caller-owned styles can update existing numeric/toggle/selector/color-picker controls without
replacing active entities or numeric drafts. Numeric text selections derive their
tint from the injected accent and surface. Application theme persistence remains
outside this crate.

Selectors render choices in a deferred anchored foreground popup. Menus escape
ancestor clipping, retain the field width, flip at window boundaries and scroll
long lists within the viewport. Popup hitboxes occlude underlying controls;
option clicks select once and stop propagation. Escape, outside click, blur and
disabling close the popup. Opening it does not change the field footprint.
A cached content-box measurement refreshes only when bounds change. These are
shared Lattice widget behaviors, with Utility as the current testbed.
