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
    'self.track(', 'self.mixer_strip(', 'self.mixer(', 'self.transport(', 'self.clip_detail_view(',
    'self.browser(', 'self.inspector(', 'self.status_bar(', 'theme::text(', 'crate::theme::text(',
    'self.icon(', 'self.control_icon(', 'self.arrangement(', 'self.ruler(', 'self.clip_view(',
    'button(', 'waveform_view(', 'clip_area)', 'header)', 'bars)', 'cells)',
    'content)', 'rows);', 'track_name_view)', 'track_controls)',
    # Compact Operator composition components keep labels routed through
    # `theme::text` at their component boundary.
    'operator_row(', 'operator_label(', 'page_tab(', 'shell)', 'page_tabs)', 'detail)',
    'level)', 'primary)', 'multiplier)', 'fixed_control)', 'section_title(', 'page_tabs(', 'control)',
    'preview::render(', 'fine)', 'toggle(', 'heading)', 'left)', 'select_row)',
    'routing_link(', 'selector(',
    'dials)', 'icon)', 'toggle.entity.clone())', 'selector.entity.clone())',
    'waveform)', 'phase)', 'velocity)', 'feedback)', 'cycle.unwrap_or_else(', 'graph)',
    'fields)', 'labeled_field(', 'carrier)', 'preset)', 'compact_control(', 'compact_control_with_codec(',
    'routing_header_cell(', 'routing_cell(',
    'hex_widgets::curve_plot(',
    # Generic device layout components accept already-themed child elements.
    'hex_widgets::device_display(', 'hex_widgets::device_body(',
    'hex_widgets::device_section(', 'hex_widgets::device_layout(',
    'hex_widgets::device_rail(', 'hex_widgets::device_grouped_row(',
    'hex_widgets::device_split(',
    'main)', 'mixer)', 'lane)', 'row)', 'rows)', 'clips)', 'strips)', 'track_labels)',
    'self.session_grid(', 'workspace)', 'self.session_toolbar(', 'self.session_icon(',
    'self.browser_view(', 'workspace_content.', 'self.browser_icon(', 'channel_controls)',
    'self.browser_sections(', 'self.browser_places(', 'self.browser_instrument_row(',
    'folder_dialog)', 'self.browser.search.clone())', 'rows.flex_1()',
    'resize)', 'panel)', 'div().w(px(4.0))', 'canvas(', 'BrowserSearchElement',
    'scene_rows)', 'cell)', 'session_scenes_header()', 'scene_column)',
    'source_adjust_button(', 'gain_adjust_button(',
    'field_row(', 'loop_row(', 'time_ruler(', 'source_marker(', 'source_column)',
    'source_row(', 'details)', 'waveform_panel)', 'canvas_region)', 'roll_area)',
    'status_slot(', 'super::waveform::waveform_view_cached(',
    'super::waveform::waveform_source_segment_view_cached(',
    'super::waveform::waveform_source_segment_gain_view_cached(',
    'toolbar)', 'control)', 'element)', 'controls)', 'clip_view)', 'step_button(', 'control_button(',
    'parameter_control(', 'rendered)', 'add_button)', 'chain)', 'render_add_menu(',
    'mode_control)', 'lane_button(', 'left)', 'compact_selector(', 'graph_entity',
    'section_heading(', 'unavailable(',
    'menu_message(', 'action_button(', 'action_button_sized(', 'entity.clone())', 'control.entity.clone())',
    'super::waveform::waveform_source_segment_view(', 'self.mixer_strip(', 'detail)', 'ruler_label)',
    'self.title_bar()', 'self.title_bar(cx)', 'self.theme_button(', 'card)',
    'header::title_bar(', 'header::loading_toolbar(',
    'lights)', 'crate::build_info::label()', 'version_label()', 'debug_button()',
    'super::loop_rail::view(', 'render_device_card_target(', 'render_device_end_target(',
    'device_drag_title(', 'render_return_drop_target(', 'uniform_list(',
    'loop_grid_layer(',
    'self.return_send_menu(', 'send_adjust_button(', 'super::device_panel::return_send_numeric_control(', 'upper.flex_1().min_h_0()',
    'returns)', 'add)', 'return_button(', 'render_return_effect(',
    'render_return_add_menu(',
    'control(', 'spectrum_toggle(', 'audition_toggle(', 'selected_value)', 'input_heading)', 'output_heading)', 'polarity)',
    'channel)', 'dial)', 'mono)', 'bass)', 'cutoff_row)', 'audition)', 'input)',
    'output)', 'gain)', 'balance)', 'cutoff)', 'mute_dc)', 'mute)', 'dc)',
    # Saturator composition uses typed shared controls and a normalized plot;
    # its axis/error text is routed through theme::text at the panel boundary.
    'plot)', 'chooser)', 'drive)', 'wet)',
    # Compressor composition uses shared typed numeric controls and normalized plot.
    'ratio)', 'attack)', 'release)', 'field(',
    # Gate composition contains already-themed compact dial child controls.
    'gate_column(',
    # Device-chain navigator is a typed view component; labels use theme::text.
    'super::device_panel::bottom_bar(', 'overview)',
    # Shared host meter strip supplies lowercase tooltip text through theme::text.
    'gain_meters::render(', 'super::gain_meters::render(',
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
# The shared compact title row reserves balanced space for native controls;
# editor startup and the loaded editor both use that component. The generated
# demo still owns its separate toolbar and retains the same leading inset.
header = (ROOT / 'crates/hex-ui/src/editor/header.rs').read_text()
if 'crate::build_info::HEADER_INSET.max(theme::TITLEBAR_LEADING_INSET)' not in header or '.px(px(' not in header:
    errors.append('editor/header.rs: center title with balanced native-control insets')
for top_row in ('editor/bootstrap.rs', 'editor/render.rs'):
    source = (ROOT / 'crates/hex-ui/src' / top_row).read_text()
    if 'header::title_bar' not in source and 'self.title_bar(cx)' not in source and 'self.title_bar()' not in source:
        errors.append(f'{top_row}: use the shared title row')
if 'header::loading_toolbar' not in (ROOT / 'crates/hex-ui/src/editor/bootstrap.rs').read_text():
    errors.append('editor/bootstrap.rs: align startup toolbar with editor toolbar geometry')
demo_toolbar = (ROOT / 'crates/hex-ui/src/layout/transport.rs').read_text()
if '.pl(px(theme::TITLEBAR_LEADING_INSET))' not in demo_toolbar:
    errors.append('layout/transport.rs: reserve the shared native titlebar leading inset')
for key in ('CFBundleName', 'CFBundleDisplayName'):
    source = (ROOT / 'scripts/hex-test-build.py').read_text()
    if f'"{key}": "hex"' not in source:
        errors.append(f'{key} must be lowercase hex')
if errors:
    print('\n'.join(errors), file=sys.stderr)
    sys.exit(1)
print('UI style passed: centralized lowercase text, Ioskeley Mono, lowercase bundle name')
