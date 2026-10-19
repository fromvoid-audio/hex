# project file actions

The native File menu offers resident/verified-streaming New and Open actions.
Focused editor handlers retain precedence. Four application fallbacks resolve
only the exact active window's actual editor root after dispatch releases its
window borrow, so losing element focus no longer silently drops these actions.
Registration is idempotent; a closed or non-editor window does not redirect the
action to another document. Rendering never forces keyboard focus.

One Open action displays the picker and retains the selected path and explicit
audio mode. A clean document opens directly, even when it contains tracks. An
unsaved document asks **save / don't save / cancel**. Save uses the document's
existing location, or Save As for an untitled document, then opens the original
selection exactly once only after the canonical save succeeds. Save As Cancel,
save failure, confirmation Cancel and stale project results abandon that intent.
Don't Save loads the selection before replacing the current document. Picker
Cancel and load failure preserve the current project, history and original media.
An in-flight intent prevents a second replacement or ordinary edit. Existing
recording admission and device-control finalization retain their guards.

Track and clip name drafts follow their existing Enter/blur rule before the
picker starts: a current valid draft becomes one canonical, Undoable rename.
The selected Open waits for that worker result before classifying the document.
Save includes the name, Cancel keeps it, and Don't Save discards it only after a
successful replacement. Invalid, stale or temporarily blocked drafts refuse
Open without consuming the draft. Manual recovery/relink actions cannot replace
an active picker or selected Open intent; the already-owned audio-recovery
Restore token remains permitted to finish without consuming the selected path.

New retains its existing matching-second-invocation status confirmation; this
Open change does not redesign New. A successful replacement publishes fresh
history. Resident and verified-streaming choices remain explicit; unavailable
verified streaming refuses rather than falling back to another mode.

Bounded rendered regressions install disposable browser/settings paths
before workers start. They cover actual window/application dispatch, focused and
unfocused editors, multiple/non-editor/closed windows, rerendering, canonical
worker save/open success, both save paths, cancellation, failures, original media
preservation and stale results. Two legacy browser/recent tests retain selected
path identity coverage without the obsolete second-click contract. The original
focus-loss and one-Open failures and intermediate compile failure are retained.
GPUI's test platform queues injected picker results; it does not display a native
OS picker. These tests are not native picker or physical input acceptance.

Final integrated full local CI belongs to the dedicated merger. In a
coordinator-approved disposable document, native acceptance must check menu and
picker presentation, Save / Don't Save / Cancel, named and untitled Save paths,
visible failures and usable focus after cancellation. Never perform destructive
document interactions on an unsaved user project. Remembering a project directory
is separate Pulse2818/PR424 work.
