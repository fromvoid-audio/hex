# Hex device-host brief

The shared host makes instruments and processors consistent to navigate, combine and reuse. It inherits the [Hex overview](../../README.md) and the local Codex product design philosophy. [Vector](vector-brief.md) is one consumer.

The [live track and device editing brief](live-repatching-brief.md) defines the
agreed 0.2 target for preparation, routing, power, retirement and undo/redo.
Those target rules are not a claim of current native behavior.

Every host is 200 px high, with variable width. Its left sidebar contains power, a device context menu immediately below it, a bottom-up device name, then macro and modulator toggles at the bottom in that order. Selection highlights the outer border, not the header fill.

Macros and modulators open independently and can remain open together: sidebar → macros → modulators → device controls → nested-device widget → external stereo meter/add button. Opening panels expands horizontally.

Both panels contain color-coded square slots. All slots within a panel have one uniform size. Use two-high sizing for one/two assigned slots, three-high for three, and four-high for four or more; a fifth begins a second column at the four-high size. Do not mix slot sizes within a panel. Each size has a deliberate internal layout: macro values remain readable, source slots show useful previews, and knobs/previews must not overlap labels.

Each device has a nested-device widget at its right end. Nested devices remain full-sized individual devices, with normal meters/add buttons between them and colored membership markers. Parent power controls the complete nested unit while preserving each child’s individual power state.

The external meter’s + inserts after the device; the nested insertion target inserts inside its child chain. Both also accept browser drag-and-drop. Clicking opens a simple modal listing available devices with a search bar. Search receives focus; arrows navigate, Enter or a result click inserts one device and closes the modal, and Escape closes without inserting. Focus returns to the new device’s controls.

The context menu includes loading/saving presets and saving a default device state. A preset captures the full unit: parent settings, macros, modulators, mappings, nested chains, child settings and on/off states. Disk behavior and destructive replacement interactions must be defined and implemented separately from illustrative Frame menus.

The [native host support inventory](native-device-host-status.md) records existing contracts, explicit placeholders and the remaining implementation paths. It does not redefine this product brief or claim native parity.
