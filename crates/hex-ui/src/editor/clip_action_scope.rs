//! One dispatch-time membership snapshot for ordinary Arrangement clip actions.
//! Property intents reach the canonical group command; geometry must use its
//! existing group gesture owner. Source/note coordinates remain explicitly local.
use super::EditorView;
use hex_project::{ClipCommandScope, Command, Invalid};
use hex_session::service::Request;

impl EditorView {
    pub(super) fn resolve_clip_action_scope(
        &self,
        request: Request,
        focused_source: bool,
    ) -> Result<Request, Invalid> {
        let Request::Edit(command) = request else {
            return Ok(request);
        };
        let selected = self.selected_clip_ids();
        if selected.len() <= 1 {
            return Ok(Request::Edit(command));
        }
        let command = match command.clip_scope() {
            ClipCommandScope::Property { clip, edit } if selected.contains(&clip) => {
                Command::EditClips {
                    clips: selected,
                    edit,
                }
            }
            ClipCommandScope::Geometry { clip } if selected.contains(&clip) => {
                return Err(Invalid(
                    "this gesture requires a selection-wide clip operation",
                ));
            }
            ClipCommandScope::FocusedSnapshot { clip }
                if selected.contains(&clip) && !focused_source =>
            {
                return Err(Invalid(
                    "this source edit requires one clip or an explicit focused-source action",
                ));
            }
            ClipCommandScope::Explicit => match command {
                // A property control names its focused member; freeze the full
                // selection here, once, before the request can be queued.
                Command::EditClips { clips, edit }
                    if clips.len() == 1 && selected.contains(&clips[0]) =>
                {
                    Command::EditClips {
                        clips: selected,
                        edit,
                    }
                }
                other => other,
            },
            // Source points/notes name identities and coordinates in one source.
            // They intentionally keep that source, unlike whole-clip properties.
            ClipCommandScope::FocusedSnapshot { .. }
            | ClipCommandScope::FocusedSource { .. }
            | ClipCommandScope::FocusedNote { .. }
            | ClipCommandScope::Property { .. }
            | ClipCommandScope::Geometry { .. }
            | ClipCommandScope::Unrelated => command,
        };
        Ok(Request::Edit(command))
    }
}
