#!/usr/bin/env python3
"""Source guard for app text routing, font selection, and bundle naming."""
from pathlib import Path
import re
import sys
ROOT = Path(__file__).resolve().parent.parent
errors = []
# Text children must be normalized centrally; other children are view elements.
widgets = (
    'div(', 'Self::button(', 'Self::with_tooltip(', 'Self::ruler(', 'Self::empty_grid(',
    'self.track(', 'self.mixer_strip(', 'self.mixer(', 'self.transport(',
    'self.browser(', 'self.inspector(', 'self.status_bar(', 'theme::text(', 'crate::theme::text(',
    'self.icon(', 'self.control_icon(', 'self.arrangement(', 'self.ruler(', 'self.clip_view(',
    'button(', 'waveform_view(', 'clip_area)', 'header)', 'bars)', 'cells)',
    'content)', 'rows);', 'track_name_view)', 'track_controls)',
    'main)', 'mixer)', 'lane)', 'row)', 'rows)', 'clips)', 'strips)', 'track_labels)',
    'self.session_grid(', 'workspace)', 'self.session_toolbar(', 'self.session_icon(',
    'self.browser_view(', 'workspace_content.', 'self.browser_icon(',
    'self.browser_sections(', 'self.browser_places(', 'self.browser_instrument_row(',
    'folder_dialog)', 'self.browser.search.clone())', 'rows.flex_1()',
    'resize)', 'panel)', 'div().w(px(4.0))', 'canvas(', 'BrowserSearchElement',
    'scene_rows)', 'cell)', 'session_scenes_header()', 'scene_column)',
    'source_adjust_button(',
)
for path in (ROOT / 'crates/hex-ui/src').rglob('*.rs'):
    source = path.read_text()
    for match in re.finditer(r'\.child\(\s*', source):
        suffix = source[match.end():]
        if not suffix.startswith(widgets):
            line = source.count('\n', 0, match.start()) + 1
            errors.append(f'{path.relative_to(ROOT)}:{line}: route text through theme::text or add an explicit view component')
    for match in re.finditer(r'\.font_family\(([^)]*)\)', source):
        if match.group(1).strip() != 'theme::FONT_FAMILY':
            errors.append(f'{path.relative_to(ROOT)}: use the required theme font')
for root_view in ('app.rs', 'editor/render.rs'):
    app = (ROOT / 'crates/hex-ui/src' / root_view).read_text()
    if '.font_family(theme::FONT_FAMILY)' not in app:
        errors.append(f'{root_view}: root UI must inherit Ioskeley Mono')
# Every transparent-titlebar entry point shares the native window-control inset.
for top_row in ('editor/bootstrap.rs', 'editor/transport.rs', 'layout/transport.rs'):
    source = (ROOT / 'crates/hex-ui/src' / top_row).read_text()
    if '.pl(px(theme::TITLEBAR_LEADING_INSET))' not in source:
        errors.append(f'{top_row}: reserve the shared native titlebar leading inset')
for key in ('CFBundleName', 'CFBundleDisplayName'):
    source = (ROOT / 'scripts/bundle-macos.sh').read_text()
    if f'<key>{key}</key><string>hex</string>' not in source:
        errors.append(f'{key} must be lowercase hex')
if errors:
    print('\n'.join(errors), file=sys.stderr)
    sys.exit(1)
print('UI style passed: centralized lowercase text, Ioskeley Mono, lowercase bundle name')
