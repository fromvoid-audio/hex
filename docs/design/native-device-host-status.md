# Native device-host support and remaining contracts

Source audit at `2818bf08fc8ac5d87a1f1027b12465983540ad74`. This compares the [host brief](device-host-brief.md) with production ownership. It is a support inventory, not evidence of native listening/interaction acceptance or a claim that all backend work is planned or implemented.

## Current support

| Brief capability | Current native path | Boundary and limitation |
| --- | --- | --- |
| 200 px hosts and left sidebar | `hex-ui::device_composition`, `hex-widgets::frame` and `device_housing` | Shared presentation exists. Caller-provided actions still include separate preset buttons; the requested context menu/default workflow is not complete. |
| Device power | `hex-project::Device::bypassed` and the existing prepared device-edit path | One device has an authored bypass switch. There is no generic per-device child unit whose effective power can inherit from a parent. |
| External stereo meter and insert-after action | `editor::device_panel::device_add` plus the device-panel meter composition | Real stable owner/device targets and canonical flat-chain insertion. |
| Searchable insertion modal | `editor::device_panel::device_picker` | Real native catalog/search/keyboard insertion. Reuse this entry point; an internal child-chain destination does not exist yet. |
| Browser insertion | Existing browser insertion/drop owners and ordinary device commands | Real supported flat destinations. Do not relabel them as nested destinations without a canonical child owner. |
| Macro/modulator toggles | `hex-widgets::device_housing` | Explicit view-only placeholders. Their panels currently stack inside the body; they are not the brief's independent horizontal, uniformly sized working slot panels. |
| Canonical device content | `hex-project::Device`, `DeviceSnapshot` and device clipboard | Device identity, native state, bypass, comparison and automation have existing owners. There are no generic host macro/modulator/child fields in `Device`. UI must not create a competing editable model. |
| Retained project instruments | `hex-project::native_sources` and existing shared-source playback owners | Real retained identities and singleton sharing exist. A source definition can retain an ordered chain, but `NativeSource::binding` explicitly refuses multi-device chains. A registry definition is not proof of multi-device playback admission. |
| Presets on disk | `hex-storage::preset`, `hex-session::service::preset`, `editor::device_preset` | Current preset schema 2 stores class, native state and optional A/B comparison. Schemas 1/2 have strict readers. This payload does not contain host children, macro/modulator mappings, authored bypass or song automation. Load/save uses current owner validation and the control worker. |
| Save default device state | No default-store request or per-type default namespace in these paths | Existing save-new presets are not user defaults. Fresh catalog insertion must be distinguished from insertion of explicitly supplied clipboard/preset state. |

## Reuse and missing work

- Shared parameter composition and the first real mapping workflow remain owned by [#3337](https://pulse.iocore.dev/#/issues/3337) and [#3452](https://pulse.iocore.dev/#/issues/3452). The generic host panel must consume those contracts; Vector's fixed internal modulation is not a replacement.
- Parallel/nested Rack work already has [#3460](https://pulse.iocore.dev/#/issues/3460), [#3482](https://pulse.iocore.dev/#/issues/3482) and [#3483](https://pulse.iocore.dev/#/issues/3483). A generic serial child slot after every device is a different ownership requirement. Reusing a Rack foundation must not silently require every parent to become a parallel Rack or duplicate its graph model.
- [#6569](https://pulse.iocore.dev/#/issues/6569) resolves that generic host-unit contract: stable identity/order and caps, retained-source versus track-local scope, parent effective bypass while preserving child switches, complete unit snapshots, provider eligibility, resource/history/migration laws and bounded implementation leaves. The detailed generic-unit implementation sequence is **not yet fully decomposed**; that issue is the explicit unblocking work, not an implementation claim.
- [#6570](https://pulse.iocore.dev/#/issues/6570) adds validated per-type disk defaults on the control worker. [#6571](https://pulse.iocore.dev/#/issues/6571) consumes it in the requested sidebar context menu and fresh catalog insertion. These can proceed independently of the generic nested unit. They initially use the existing state/comparison preset payload and must say so.
- [#3893](https://pulse.iocore.dev/#/issues/3893) applies the shared 200 px host's horizontal macro/modulator panel layout after real source/mapping/ownership contracts exist. The panel geometry follows the current host brief: independent panels and uniform two/three/four-high square slots. It is blocked by real mapping and host-contract work, rather than by Vector-specific FM migration.
- Retained-source manager and editing-scope UI already belongs to [#3250](https://pulse.iocore.dev/#/issues/3250) and [#3281](https://pulse.iocore.dev/#/issues/3281). Reuse the canonical inventory for the concept's Project browser. Preserve assignment-versus-browsing, the one explicit receiver, source lifetime, and honest multi-device refusal. Reconcile browser/leading-picker affordance placement with the current Frame design without inventing a second manager or restoring a discarded decorative source card.

## Keep the current Vector iteration independent

The [Vector brief](vector-brief.md) fixes the initial routing algorithms and keeps general modulation at host level. [#3889](https://pulse.iocore.dev/#/issues/3889) therefore uses the existing native algorithm and voicing parameters; it no longer waits for future FM-as-host-mapping work. Loaded legacy/custom routing and native voice counts remain intact until explicitly changed. This does not cancel future modulation work or authorize a silent state migration.

## Preset and insertion invariants for the next contracts

A future full-unit format must explicitly preserve child settings and authored power, macros/modulators and mappings, stable references and resources. Its load must be one validated prepare-before-commit operation with Undo/Redo; a partially installed chain is not success. Define how old state-only presets preserve the existing target's host state, and keep song-time automation distinct from reusable device-unit contents unless that product contract changes. Newer/unknown/malformed payloads must not be silently downgraded.

Saved-default selection applies only to a typed fresh-creation intent. Paste, duplicate, imported presets and retained-source assignment already carry authored state and must not be overwritten by a default. Default file I/O belongs on the control worker, and a refused write must preserve the previous app-owned file and project. A successful disk save is not a project edit or native audio acceptance.

## Evidence paths

- [Canonical device](../../crates/hex-project/src/device.rs) and [snapshots](../../crates/hex-project/src/snapshot.rs).
- [Retained sources and binding refusal](../../crates/hex-project/src/native_sources.rs).
- [Shared housing placeholders](../../crates/hex-widgets/src/device_housing.rs) and [device dimensions](../../crates/hex-ui/src/device_composition.rs).
- [Insert-after target](../../crates/hex-ui/src/editor/device_panel/device_add.rs) and [catalog modal](../../crates/hex-ui/src/editor/device_panel/device_picker.rs).
- [Preset storage](../../crates/hex-storage/src/preset.rs), [worker owner validation](../../crates/hex-session/src/service/preset.rs) and [native preset actions](../../crates/hex-ui/src/editor/device_preset.rs).

No runtime source or saved format changed in this audit. Existing implementation tests were not rerun for prose. Source paths, current field/envelope shapes and linked issue states/dependencies were inspected directly; native UI/audio qualification remains with each implementation issue.
