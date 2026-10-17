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
`ControlStyle::with_device_selection` returns `DeviceFrameStyle`, which keeps the
neutral shell palette separate from header paint. `device_frame` and
`device_header` accept either presentation; a plain `ControlStyle` gives an idle
header matching its surround. Products may inject contrast-checked selected
header colors with `with_header_colors`. Selection never recolors the outer
border, body or control palette, and widgets do not acquire selection/focus,
bypass or drag/drop ownership.

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
device-specific layout or parameter semantics.

`ResponseGraph` blank-press creation is owner-opted-in. Owners may separately
enable creation dragging: the widget retains pointer capture and emits its
normal begin/change/commit/release/cancel sequence with node ID zero throughout.
The owner maps that stream to its own provisional identity and transaction;
asynchronous model publication cannot attach a released press to a new node.
Click-only creation owners retain their existing behavior.

`ResponseGraphModel` accepts separate normalized input and output analyzer
curves. Both are bounded, validated and painted behind response curves/nodes;
the owner controls visibility and injects semantic colors through
`ResponseGraphAnalyzerStyle`. The widget owns no audio capture or FFT state.

### shared device composition

Every device uses one `device_frame`/`device_header` geometry: 14px header,
3px horizontal header inset, 2px header gaps, 4px outer radius and 1px border.
The header and surround both paint the injected `surface`, with no bottom rule.
`with_device_selection` returns independent header paint without transforming
the neutral frame palette; body/control styles remain unchanged.
There is no compact/standard frame switch or device-selected header density.

Build the body with `device_body` (3px horizontal inset, 4px layout gutters),
then compose independently sized children:

- `device_layout(DeviceLayout::{Row, Column, Wrapped, Grid(columns)})` provides
  ordinary, wrapping or equal-column grid arrangements; zero grid columns means
  one. `device_split(first, second)` gives two equal flexible panes. A Row allows
  fixed/unequal widths and caller-selected flexible children.
- `device_section(style, DeviceSectionSurface::{Surround, Rail, Display})` gives
  a rounded semantic surface without fixing its height or children.
- `device_rail` groups rows with 2px gutters. `device_grouped_row(style, active)`
  supplies the shared rail/surround focus state, without owning focus or bindings.
- `device_display` supplies a recessed column for graphs or tabbed inspectors.
  Owners retain graph data, tab slots, selection/focus state and gesture entities.

Sections use 3px radii and 4px horizontal insets. Insets are same-color structural
side borders: GPUI clips descendants' paint and hit masks inside visible borders,
whereas padding alone cannot contain oversized children at rounded corners.
This leaves the full vertical content height available for 40px controls. Keep
those borders/overflow masks when composing; do not replace them with padding,
transparent borders or `overflow_visible`. Add content padding inside when needed.
The body uses the same mechanism with 3px side borders and rounded lower corners.
Header side borders protect the inner rounded top corners and clip oversized
caller title/actions. The surface and border paint retain their actual radii.
Numeric readouts retain the separate `CONTROL_CORNER_RADIUS` token.

`device_compact_section` uses the same 3px rounded surface with 4px vertical
structural insets instead of horizontal insets. Use it for narrow full-width
controls; it preserves their width while containing paint/hits above and below.
`device_dense_section` and `device_compact_row` use 1px corners and vertical
insets when a tightly packed column or short row needs its full control height.
Pick one boundary for a region, rather than nesting section insets. These are
reusable density/containment policies, never device identities.

`device_action` supplies the shared 12px compact action geometry, and
`device_segment` adds the common readout rounding for direct selection.
`DeviceActionStyle` injects foreground/background for both idle and hover; the
product is responsible for contrast. `device_badge`, `device_identity_marker`,
`device_focus_target`, `device_caption`, `device_inline_caption` and
`device_divider` keep stateless device chrome inside this library. Captions
separate full-column headings from intrinsic-width inline labels. Owners attach
IDs, focus, handlers/tooltips and semantic colors; widgets own no device state.

Extend this library first when a device needs a new visual capability. Add a
small generic semantic primitive/layout option and rendered boundary coverage,
then consume it in the panel. Do not add device-named variants, type-ID switches,
parameter semantics, a monolithic device template or another GUI library.
Colors stay injected and typography inherited; device width, parameter order,
canonical bindings and musical meaning remain caller-owned.

Rendered tests cover normal/constrained header/body bounds, actual filled-quad
radii/backgrounds, oversized content paint/hit masks, equal split/grid placement,
wrapping and unchanged control heights. They inspect the GPUI scene and input
routing, not GPU framebuffer pixels or physical native visual acceptance.

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

Untouched numeric drafts retain the original exact value when an owner finalizes
them or the field loses focus. Display rounding is not a parameter edit. Actual
typed/composed input is tracked separately, so deliberately entering the displayed
rounded value still commits through the existing edit events and validation.

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

The `DeviceMeters` numeric presentation renders two adjacent sample-peak wells.
The input well is read-only; output uses the existing numeric scrub/reset/focus
contract. Callers supply levels, held markers, height, colors and already-themed
hover text through `DeviceMeterReadout`; widgets never fetch audio telemetry.

`DeviceMeters` presents one compact adjustable meter well. The host supplies
input/output index, live and held sample peaks, gain, colors and tooltip text,
and composes the two controls. Gain and unity lines accompany the audio fill.
Readout updates do not interrupt pointer gestures; both wells use the shared
numeric gesture and double-click reset lifecycle.

Windowed generic bodies retain the shared surface and rounded corners while
placing the standard inset inside their scroll content. The editor owns row
visibility, offsets and gesture pins; the widget owns chrome and that inset.
