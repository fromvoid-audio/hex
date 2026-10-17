# hex-lattice-contract

## Purpose

Bounded, toolkit-independent metadata and opaque state contracts shared by
authored native devices, projects, storage, and the DSP lattice. Parameter display
text is nonempty lowercase text without control characters, limited to 128 UTF-8
bytes per label/unit/choice; selectors expose at most 256 choices. Both native
registries enforce these descriptors before admission. Static catalog descriptors
remain available; per-instance descriptors are owned values enumerated from the
current codec-validated state. `validated_state_descriptors` checks overrides
independently: at most 512 unique valid device parameter IDs, valid lowercase
presentation and typed defaults. Current-state validation, single/atomic patches,
availability and engine views use this boundary. Device codecs validate the
complete authored state; parameter values are additionally checked when read,
patched or presented. Enumeration must not recurse through registry migration/
default construction. Parameter IDs remain bounded to
1–4,294,967,293, below the host input/output gain namespace. Dynamic enumeration remains
control-side and does not add callback allocation or device automation. Existing
IDs stay unchanged; devices deriving IDs from stable object identities must use
checked arithmetic and explicitly refuse namespace exhaustion.

## Dependencies

Rust standard library only.

## Does not own

DSP, device implementations, project editing, serialization formats, UI, plugin
hosting, or runtime device registries.
