# hex-widgets

## Purpose

Reusable, compact native GPUI controls for Hex, including numeric knobs and
sliders, toggles, selectors, color pickers, and device frames. Numeric controls
and the color picker expose typed change events; callers own canonical state,
audio commands, persistence, and undo policy.

`MiniDial` and `CompactDial` share the same numeric entity and gesture path at
different reusable densities. `curve_plot` renders a bounded, read-only curve
from normalized points without assigning device-specific meaning to them.
`transfer_plot` uses the same bounded curve renderer with centered input/output
axes. Callers supply normalized samples and display their own axis ranges;
the widget has no DSP, device state or telemetry dependency.

Utility is the current visual testbed for the shared Lattice device interface.
Frames, knobs, sliders, value fields, switches, selectors and popup behavior are
library controls, not Utility-specific implementations. Device panels compose
these controls and bind their descriptors; changes to shared controls propagate
to every device using them. Presentation variants describe reusable density or
control purpose, never a device identity.

Device frames accept title elements as well as text, so callers can attach header
drag input while the frame retains shared title styling and fixed geometry.

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


Compact square switches, directly selectable compact segments and flat panel tabs
are reusable density variants with injected display/style. Shared compact dials
use open rails, progressive value arcs and needles; micro dials place labels above and
readouts beside their lower edge. Owners retain canonical bindings and gestures.

Rectangular numeric readouts retain an arrow pointer during hover and typing.
On macOS, an active value-field drag requests GPUI's window-scoped invisible
cursor after the movement threshold. Click-to-type and the insertion caret stay
available. Release, cancellation, focus/deactivation, disable and removal end
that override. Control-surface knob/slider cursors remain unchanged. See
`docs/device-controls.md` for the native acceptance gate and platform limits.

All numeric controls share noninteractive three-pixel corner marks for enabled
focus, active pointer presses and typed drafts. The caller injects neutral
`focus_marker` and unfilled `dial_rail` colors. Valid typed drafts retain their
caret/selection and inset background without a continuous accent outline;
invalid input keeps its existing red error outline. The one-pixel transparent
root border preserves the prior layout inset.

Numeric, toggle, selector and response-graph controls can opt into rendered-order
Tab traversal with `set_tab_navigation`. Participation defaults off and excludes
disabled controls (or noneditable graphs), including when availability changes
after opting in. Owners finalize pending drafts and choose traversal boundaries.
Editable graphs using this mode leave Tab/Shift-Tab
to their owner and use Ctrl-Tab/Ctrl-Shift-Tab to cycle nodes; their existing arrow
editing remains unchanged. Focused participating graphs use the shared corner
marks without altering graph geometry.

All knob densities use one normalized rail renderer with explicit `DialLayout`
semantics. `ReadoutGap` opens at lower right; `BottomGap` opens at the bottom;
`Pan` uses a bottom opening, a center-up neutral needle and a reset triangle.
Orientation is independent of density and parameter scale. Ordinary values fill
from normalized minimum; bipolar values fill from neutral center toward either
signed endpoint. Disabled controls keep muted artwork and no value fill.
The renderer consumes NumericSpec's mapping without changing parameter laws.
`MICRO_DIAL_WIDTH`, `MICRO_DIAL_HEIGHT` and `MICRO_DIAL_DIAMETER` define the shared
58x40 label-above tile used by both MicroDial and MiniDial. Compact and Standard
retain their footprints. The transient pointer-capture element is absolute so
pressing cannot insert a flex gap or move the dial/readout.
See research/observations/knob-focus-001.md for reference limits.
