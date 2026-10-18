# project file actions

The native File menu offers resident/verified-streaming New and Open actions.
Focused editor handlers retain precedence. Four application fallbacks resolve
only the exact active window's actual editor root after dispatch releases its
window borrow, so losing element focus no longer silently drops these actions.
Registration is idempotent; a closed or non-editor window does not redirect the
action to another document. Rendering never forces keyboard focus.

Existing document replacement semantics stay in the canonical controller:
content/unsaved state asks for a second matching New/Open invocation through the
visible status message. Opening pauses edits; recording admission and device
control finalization retain their existing guards. A successful New publishes an
empty document and fresh history; a successful Open publishes the loaded project.
A failed Open leaves the current document and history intact and reports an error.
Resident and verified-streaming choices are explicit; unavailable verified
streaming refuses rather than falling back to another mode.

Eight bounded rendered regressions use disposable browser/settings paths installed
before workers start. They cover real window/application action dispatch with
focused, unfocused, multiple, non-editor and closed windows, exactly-once dirty
confirmation after re-rendering, actual worker New/saved Open, missing/malformed
Open preservation, pending refusal and explicit mode refusal. The original two
focus-loss failures are retained in Pulse evidence. GPUI's headless platform does
not implement the native Open picker: confirmation-path tests are not proof of
picker presentation or cancellation on macOS.

Final integrated full local CI belongs to the dedicated merger. Native menu and
picker acceptance is separate: in a coordinator-approved disposable document,
verify New confirmation preserves content until confirmed, Open displays a picker,
Cancel preserves current project/history and returns usable focus, a valid
current-format file opens, and failure remains visible without replacement.
Never run these destructive-document interactions on an unsaved user project.
Remembering a project directory is separate Pulse2818/PR424 work.
