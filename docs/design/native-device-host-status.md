# Native device-host support and remaining contracts

Source audit updated for the implemented native device-host unit contracts (`#13263`, `#13264`). This compares the [host brief](device-host-brief.md) with production ownership. It is a support inventory, not evidence of native macOS listening/interaction acceptance.

## Current support

| Brief capability | Current native path | Boundary and limitation |
| --- | --- | --- |
| 200 px hosts and left sidebar | `hex-ui::device_composition`, `hex-widgets::frame`, `device_housing`, `editor::device_preset` | Shared presentation and sidebar context menu (`load preset`, `save preset`, `save as default`) are wired through worker barriers and Shift-F10 keyboard access. |
| Device power & serial child units | `hex-project::Device::parent` (`MAX_SERIAL_DEVICE_DEPTH = 8`), `Command::InsertNestedDevice`, `Command::ReparentDevice`, `hex-engine::serial_effect_power::EffectPower` | Authored `bypassed` stays local to each node while `EffectPower` propagates ancestor effective bypass across Track, Return, Main, and retained-source suffixes without overwriting child switches. |
| External stereo meter and insert-after action | `editor::device_panel::device_add` plus the device-panel meter composition | Real stable owner/device targets and canonical chain insertion. |
| Searchable insertion modal & browser insertion | `editor::device_panel::device_picker`, browser drop owners, `hex-session::service::device_defaults` | Fresh catalog insertions resolve per-type saved defaults on the control worker; supplied state (paste, duplicate, preset load, retained-source assignment) is never overwritten. |
| Macro/modulator toggles & panels | `hex-widgets::device_housing`, `editor::device_panel::modulators`, `hex-lattice::NativeModulatorRegistry` | Independent horizontal macro/modulator panels with uniform slot geometry wired into canonical `Device::modulation`. |
| Canonical device content | `hex-project::Device`, `DeviceSnapshot`, `PortableDeviceUnit`, `DeviceInitialization` | Device identity, native state, `parent`, `bypassed`, comparison, modulation, and automation share canonical ownership and schema-125 persistence. |
| Retained project instruments | `hex-project::native_sources`, `hex-engine::source_effect_stage` | Multi-device `NativeSource` post-instrument chains of zero-latency native effects (including nested serial children) compile and play back once per merged source with receiver-only audio delivery and parent bypass inheritance. Pre-instrument source effects and foreign/nonzero-latency source effects remain explicitly refused. |
| Presets on disk | `hex-storage::preset` (schemas 1–2 state-only, schema 3 `preset/unit.rs`), `hex-session::service::preset`, `editor::device_preset` | Schema-3 full-unit presets save and load parent settings, A/B comparison, host trims, macros/modulators/mappings, serial children, and individual power states atomically in one prepare-before-commit Undo/Redo operation. Legacy schemas 1–2 preserve target host state. Asset-bearing unit preset transfer (`OriginalStore`) remains explicitly refused at the service boundary until media-import custody is wired. |
| Save default device state | `hex-storage::preset::defaults`, `Request::SaveDeviceUnitDefault`, `hex-session::service::device_defaults` | Control-worker atomic publication of per-class/provider defaults (state-only or schema-3 full unit), wired into the sidebar context menu (`save as default`) and applied only to fresh catalog insertions (including nested child catalog construction). |

## Remaining boundaries

- Parallel/nested Rack branches remain owned separately ([#3460](https://pulse.iocore.dev/#/issues/3460), [#3482](https://pulse.iocore.dev/#/issues/3482), [#3483](https://pulse.iocore.dev/#/issues/3483)) and do not force serial child units into parallel Racks.
- Asset-bearing unit preset/default media import (e.g. Sampler original audio transfer through `hex-session::service::preset::originals::OriginalStore`) remains explicitly refused rather than silently dropping asset references.
- Native macOS visual/listening qualification remains tracked separately from headless unit/playback proofs.

## Evidence paths

- [Canonical device](../../crates/hex-project/src/device.rs), [unit parenting](../../crates/hex-project/src/device_unit_parent.rs), [initialization](../../crates/hex-project/src/device_initialization.rs), and [snapshots](../../crates/hex-project/src/snapshot.rs).
- [Retained sources](../../crates/hex-project/src/native_sources.rs), [serial effect power](../../crates/hex-engine/src/serial_effect_power.rs), and [source effect stages](../../crates/hex-engine/src/source_effect_stage.rs).
- [Shared housing](../../crates/hex-widgets/src/device_housing.rs) and [device dimensions](../../crates/hex-ui/src/device_composition.rs).
- [Insert-after target](../../crates/hex-ui/src/editor/device_panel/device_add.rs) and [catalog modal](../../crates/hex-ui/src/editor/device_panel/device_picker.rs).
- [Preset storage](../../crates/hex-storage/src/preset.rs), [full-unit preset schema 3](../../crates/hex-storage/src/preset/unit.rs), [per-type defaults](../../crates/hex-storage/src/preset/defaults.rs), [worker preset/default owners](../../crates/hex-session/src/service/preset.rs), [fresh-insert defaults](../../crates/hex-session/src/service/device_defaults.rs), and [sidebar preset/default menu](../../crates/hex-ui/src/editor/device_preset.rs).
