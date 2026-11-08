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
    'self.track(', 'self.mixer_strip(', 'self.mixer(', 'self.transport(', 'self.controller_input_row(', 'self.controller_input_settings(', 'self.clip_detail_view(',
    'self.browser(', 'self.inspector(', 'self.status_bar(', 'theme::text(', 'crate::theme::text(',
    'self.icon(', 'self.control_icon(', 'self.control_icon_description(',
    # Workspace navigation components route their glyphs/tooltips through theme::text.
    'self.browser_toggle_button(', 'self.workspace_view_toggle(',
    'self.arrangement(', 'self.ruler(', 'self.clip_view(',
    # Main's arrangement map paints cached geometry without text.
    'super::arrangement_map::view(', 'super::arrangement_map::section(',
    'self.section_launch_action(',
    # Meter insertion action normalizes its caption in the owning component.
    'device_add::button(',
    'device_picker::render(',
    'super::device_panel::picker_overlay(',
    # Query clear-slot composition contains a themed action and native input semantics.
    'self.clear_search_slot(',
    # Pinned browser collections compose themed heading, labels and icons.
    'self.browser_collections(',
    # Header drop wrapper preserves the existing themed row and only refines drag paint.
    'self.device_drop_header(',
    # Clip name components normalize labels or render a native text-input entity.
    'self.clip_name_label(', 'self.clip_name_header(', 'self.track_name_label(', 'edit.input.clone())',
    # Waveform body wrappers contain view elements; preview titles normalize text.
    'clip_waveform_body(', 'super::tracks::clip_waveform_body(', 'self.clip_name_preview_header(',
    # Fitted MIDI note columns paint one text-free canvas rectangle each.
    'super::midi_thumbnail::note_column(',
    # Timeline boundary shell contains a previously constructed view and paint edges.
    'clip_boundary_shell(',
    # Loop body wrappers contain native views; marker glyphs use theme::text.
    'loop_handles::body(', 'loop_handles::audio_markers(', 'loop_handles::midi_markers(',
    'clip_fade_layer(', 'self.clip_name_drag_header(', 'clip_name_text(',
    'button(', 'waveform_view(', 'clip_area)', 'header)', 'bars)', 'cells)',
    'content)', 'rows);', 'track_name_view)', 'track_controls)',
    # Compact Operator composition components keep labels routed through
    # `theme::text` at their component boundary.
    'detail_caption(', 'detail_field(', 'upper_display)', 'envelope_fields)',
    # Companion inspector children are typed controls or already-themed rows.
    'harmonics::tools(', 'restart)', 'oscillator_velocity)', 'quantize)',
    'row.child(', 'loop_cell)',
    'tools)', 'oscillator_fields)', 'link)', 'enabled)', 'hex_widgets::device_segment(',
    'hex_widgets::action(',
    # The shared splitter handle paints caller-resolved geometry/color only.
    'self.project_spectrum_splitter_input(cx)',
    # Recorder controls route captions through the shared themed button factory.
    'self.sample_arm_button(', 'self.sample_recorder_mode_button(',
    # The take waveform canvas paints geometry only.
    'wave_canvas(',
    'harmonic_range_control)', 'harmonic_repeat_control)', 'harmonic_tools)',
    'view.device_panel.operator_harmonics[&key].entity.clone())',
    'operator_row(', 'operator_power(', 'page_tab(', 'shell)', 'page_tabs)', 'detail)',
    'level)', 'primary)', 'multiplier)', 'fixed_control)', 'section_title(', 'page_tabs(', 'control)',
    'preview::render(', 'fine)', 'toggle(', 'heading)', 'left)', 'select_row)',
    'routing_link(', 'selector(',
    'dials)', 'icon)', 'toggle.entity.clone())', 'selector.entity.clone())',
    'waveform)', 'waveform_preview)', 'phase)', 'velocity)', 'feedback)', 'cycle.unwrap_or_else(', 'graph)',
    'fields)', 'labeled_field(', 'carrier)', 'preset)', 'compact_control(', 'compact_control_with_codec(',
    'routing_header_cell(', 'routing_cell(',
    # Algorithm diagrams have no text; global controls normalize captions.
    'behavior.flex_1()', 'algorithms::selector(', 'global::controls(', 'global::expression_routing(', 'diagram(', 'control.w_full())', 'field)', 'line)',
    'hex_widgets::curve_plot(',
    # Output meter labels are routed through theme::text in the component.
    'crate::output_meter::render(',
    # Generic device layout components accept already-themed child elements.
    'hex_widgets::device_display(', 'hex_widgets::device_body(',
    'hex_widgets::device_section(', 'hex_widgets::device_layout(',
    'hex_widgets::device_rail(', 'hex_widgets::device_grouped_row(',
    'hex_widgets::device_split(',
    # Shared production composition slots are already-bound native view elements.
    'compact_rail(', 'dense_rail(', 'device_compact_section(', 'plot(', 'range)', 'footer)', 'rail)',
    'first)', 'second)', 'preview)', 'tabs)', 'rails)',
    'hex_widgets::device_divider(', 'hex_widgets::device_identity_marker(',
    'main)', 'mixer)', 'lane)', 'row)', 'rows)', 'clips)', 'strips)', 'track_labels)',
    'self.session_grid(', 'workspace)', 'self.session_toolbar(', 'self.session_icon(',
    'self.browser_view(', 'workspace_content.', 'self.browser_icon(', 'channel_controls)',
    # Split browser components normalize every caption at their boundary.
    'self.browser_history_arrow(', 'self.browser_reveal_button(',
    'self.browser_tag_selector(', 'browser_caption(', 'browser_heading(',
    'picker_label(', 'picker_heading(', 'picker_group_heading(',
    # Audition row controls normalize labels; miniature waveform contains no text.
    'self.browser_audition_row(',
    # Shared sampler/browser waveform canvas contains no text.
    'super::super::overview_waveform::render(',
    'browser_pending(', 'browser_divider(', 'group)', 'groups)', 'tags.child(',
    'editor)', 'sidebar)', 'panel.w_full(', 'self.browser_sidebar_resize(',
    'super::super::waveform::waveform_summary_view(',
    'self.browser_sections(', 'self.browser_places(', 'self.browser_instrument_row(',
    # Sample source filters route labels through theme::text.
    'self.sample_filters(', 'filters)',

    'folder_dialog)', 'self.browser.search.clone())', 'rows.flex_1()',
    'resize)', 'panel)', 'div().w(px(4.0))', 'canvas(', 'BrowserSearchElement',
    'layout::resize_divider_line(', 'resize_divider_line(',
    'scene_rows)', 'cell)', 'session_scenes_header()', 'scene_column)',
    'musical_control_row(', 'musical_control_label(', 'super::musical_audio_controls::view(',
    # Beats controls compose themed buttons/labels, not direct text children.
    'beats_controls(', 'choices)',
    'super::musical_audio_controls::warp_header(', 'sample_bpm::field(',
    'super::waveform::prepared_waveform_view_cached(',
    'super::waveform::timeline_waveform_view_cached(',
    'boundary::cursor_action(', 'boundary::handle(', 'source_adjust_button(', 'gain_adjust_button(', 'session_audio_loop_button(',
    'field_row(', 'loop_row(', 'time_ruler(', 'source_marker(', 'source_column)',
    'source_row(', 'details)', 'waveform_panel)', 'canvas_region)', 'roll_area)',
    'status_slot(', 'disk_overload_slot(', 'super::waveform::waveform_view_cached(',
    'super::waveform::waveform_source_segment_view_cached(',
    'super::waveform::waveform_source_segment_gain_view_cached(',
    'toolbar)', 'control)', 'element)', 'controls)', 'clip_view)', 'step_button(', 'control_button(',
    'parameter_control(', 'rendered)', 'add_button)', 'chain)', 'render_add_menu(',
    'mode_control)', 'lane_button(', 'left)', 'compact_selector(', 'graph_entity',
    'section_heading(', 'unavailable(',
    'menu_message(', 'action_button(', 'action_button_sized(', 'text_action_button(', 'text_segment_button(', 'entity.clone())', 'control.entity.clone())',
    'super::waveform::waveform_source_segment_view(', 'self.mixer_strip(', 'self.track_volume(', 'detail)', 'ruler_label)',
    'self.window_controls(', 'self.header_diagnostics(', 'self.project_title_label(', 'self.theme_button(', 'card)',
    'header::window_controls(', 'header::loading_toolbar(',
    # Changelog body/group captions normalize text; the test paint probe has no label.
    'self.changelog_group(', 'self.changelog_paint_probe(', 'notes)',
    'crate::tooltips::HelpToggle)',
    'lights)', 'crate::build_info::label()', 'crate::build_info::editor_label()', 'version_label()', 'debug_button()',
    'super::loop_rail::view(', 'render_device_card_target(', 'render_device_end_target(',
    # Device preset header actions reuse themed compact action buttons.
    'super::device_preset::actions(', 'super::device_panel::action_button(', 'super::device_panel::segment_button(',
    'super::super::device_preset::actions(',
    # The VST3 editor toggle uses the same themed device action.
    'vst3_editor_button(',
    # Shared A/B actions compose themed compact buttons, never raw text.
    'super::device_comparison::actions(', 'super::super::device_comparison::actions(',
    'device_drag_title(', 'render_return_drop_target(', 'uniform_list(',
    'loop_grid_layer(',
    # Track sizing composes non-text pointer handles and capture listeners.
    'self.arrangement_playback_layer(', 'self.track_resize_events(', 'self.track_width_resize_handle(',
    'self.return_send_menu(', 'send_adjust_button(', 'super::device_panel::return_send_numeric_control(', 'upper.flex_1().min_h_0()',
    'returns)', 'add)', 'return_button(', 'render_return_effect(',
    'render_return_add_menu(',
    'control(', 'spectrum_toggle(', 'audition_toggle(', 'selected_value)', 'input_heading)', 'output_heading)', 'polarity)',
    'channel)', 'dial)', 'mono)', 'bass)', 'cutoff_row)', 'audition)', 'input)',
    'output)', 'gain)', 'balance)', 'cutoff)', 'mute_dc)', 'mute)', 'dc)',
    # Saturator composition uses typed shared controls and a normalized plot;
    # its axis/error text is routed through theme::text at the panel boundary.
    'plot)', 'plot.id(', 'self.project_spectrum_panel(', 'self.project_spectrum_pointer_events(', 'chooser)', 'drive)', 'wet)',
    # Delay composition children are typed Div/shared controls; captions are
    # normalized through theme::text by control()/channel()/segment_button().
    'row()', 'mode)', 'timing)', 'offset)', 'right)', 'freeze)', 'smoothing)',
    'ping)', 'mix)', 'filter)', 'frequency)', 'width)', 'filter_row)',
    'disclosure)', 'delay_mod)', 'filter_mod)', 'modulation)', 'rate)', 'wave)',
    'morph)', 'quality)', 'loudness)', 'timings)', 'center)',
    # Compressor composition uses shared typed numeric controls and normalized plot.
    'ratio)', 'attack)', 'release)', 'field(',
    # Gate composition contains already-themed compact dial child controls.
    'gate_column(',
    # Device-chain navigator is a typed view component; labels use theme::text.
    'super::device_panel::bottom_bar(', 'overview)',
    # The passive post-device stereo meter supplies lowercase tooltip text through theme::text.
    'device_meter::render(', 'super::device_meter::render(', 'hex_widgets::stereo_peak_meter(',
    # Typed routing/recording components normalize their labels at the boundary.
    'self.routing_controls(', 'gain_row(', 'sources)', 'super::waveform::button(',
    # Retained recording controls route all captions through theme::text internally.
    'super::gain_recording::retained_controls(',
    # Fit/back controls delegate labels to the existing themed waveform button.
    'self.zoom_fit_button(', 'view.zoom_fit_button(',
    # Audio-event editor components keep their text at the themed boundary.
    'super::audio_event_editor::view(', 'waveform::button(',
    'waveform::waveform_source_segment_gain_view_cached(', 'waveform::waveform_view_cached(',
    'toolbar)', 'body)',
    'binding.entity.clone())', 'super::super::gain_recording::controls(',
    # Fade pointer capture is a typed canvas listener with no rendered text.
    'self.fade_pointer_events(', 'self.clip_detail_pointer_events(',
    # Tuning components normalize labels at their theme::text boundary.
    'tuning_section(', 'self.tuning_control(', 'self.tuning_library_panel(', 'self.tuning_mts_controls(',
    'pitch)', 'palette)', 'plots::lattice(', 'tuning_name_input.clone())',
    'tuning_nodes)', 'tuning_slots)', 'tuning_reference)', 'settings)', 'right)',
)
for path in (ROOT / 'crates/hex-ui/src').rglob('*.rs'):
    source = path.read_text()
    for match in re.finditer(r'\.child\(\s*', source):
        suffix = source[match.end():]
        # These two typed error components share fragment-normalized text. Keep
        # the exception local: arbitrary self.display_text() is not a text bypass.
        typed_error = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/render.rs'
            and suffix.startswith('error.display_text())')
        ) or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/display_error.rs'
            and suffix.startswith('self.display_text())')
        )
        # This fixture canvas records paint bounds and emits no text. Keep the
        # qualified call local rather than exempting its file or other canvases.
        fixture_canvas = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/waveform_header_ui_test.rs'
            and suffix.startswith('gpui::canvas(')
        )
        # Header mixer composition uses typed controls; each caption is checked
        # separately at its theme::text boundary. Keep variable names file-local.
        header_mixer_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/tracks.rs'
            and suffix.startswith(('utilities)', 'activator)', 'solo)', 'arm)',
                                   'self.track_pan(', 'header_content)'))
        ) or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/return_tracks.rs'
            and suffix.startswith(('self.return_volume(', 'gain_control)'))
        ) or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/mixer.rs'
            and suffix.startswith(('super::track_meters::render(', 'toggle_button_with_style('))
        )
        # Track meters compose a text-free shared gauge and its rendered fixture.
        # Tooltip text still passes through MiniTooltip's theme::text boundary.
        header_mixer_component = header_mixer_component or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/track_meters.rs'
            and suffix.startswith(('meter)', 'render_meter('))
        )
        # This fixture calls the typed, text-free performance segment view.
        # Keep the registration local; tooltip prose is normalized by its owner.
        header_mixer_component = header_mixer_component or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/track_performance/tests.rs'
            and suffix.startswith('super::super::render(')
        )
        # Clip-detail bindings return shared widgets; their labels pass through
        # theme::text in the owning component, never raw .child text.
        clip_detail_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/arrangement_audio_panel.rs'
            and suffix.startswith(('controls::number(', 'controls::toggle('))
        )
        # Console column and reorder boundaries return text-free surfaces or canvases.
        console_column_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/session_grid.rs'
            and suffix.startswith(('self.console_column_edge(',
                                   'self.console_column_surface(', 'self.console_reorder_layer(',
                                   'self.track_reorder_events(', 'tail)'))
        )
        # Reverb's grouped panel passes bound controls and provider plot data;
        # titles and numeric/toggle labels normalize through theme::text.
        reverb_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/device_panel/reverb_panel.rs'
            and suffix.startswith(('row(', 'title(', 'graph(', 'graph_column)',
                                   'contour)', 'early)', 'diffusion)', 'chorus)',
                                   'top)', 'bottom)', 'smooth)', 'density)', 'freeze)', 'high)', 'low)'))
        )
        # Browser place icons are text-free SVG components; their accessible
        # labels are checked separately at the existing theme::text boundary.
        browser_vector_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/browser/layout.rs'
            and suffix.startswith(('gpui::svg()', 'browser_vector_icon('))
        )
        # Browser result wrappers contain the existing UniformList and a
        # text-free scrollbar. Keep these exceptions at their typed owners.
        browser_scrollbar_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/browser.rs'
            and suffix.startswith('self.browser_results(')
        ) or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/browser/scrollbar.rs'
            and suffix.startswith(('list)', 'self.browser_scrollbar('))
        )
        # Playback vector controls and their SVG child contain no text.
        transport_vector_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/transport.rs'
            and suffix.startswith(('self.vector_icon(', 'gpui::svg()'))
        )
        # Limiter slots are bound controls or a shared text-free meter widget.
        # Every caption remains independently checked at theme::text.
        limiter_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/device_panel/limiter_panel.rs'
            and suffix.startswith(('crate::device_composition::dense_rail(', 'hex_widgets::level_meter(', 'meters)', 'gain)', 'link)',
                                   'maximize_control)', 'threshold)', 'display)', 'mode)',
                                   'lookahead)', 'routing)', 'release)', 'auto)'))
        )
        # Device chain insertion gaps are text-free hit targets beside each device.
        device_insertion_component = (
            path.relative_to(ROOT).as_posix() in (
                'crates/hex-ui/src/editor/device_panel.rs',
                'crates/hex-ui/src/editor/device_panel/return_drag.rs',
            )
            and suffix.startswith('insertion_gap(')
        )
        # Catalog list rows normalize their labels inside the shared picker.
        device_insertion_component = device_insertion_component or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/device_panel/device_picker.rs'
            and suffix.startswith(('list)', 'card)'))
        )
        # The rotated title paints the already themed label in the inherited font.
        device_insertion_component = device_insertion_component or (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/device_panel.rs'
            and suffix.startswith('hex_widgets::vertical_device_name(title)')
        )
        # The log viewer displays source bytes and native filenames verbatim;
        # query/list are typed GPUI components; log_scrollbar is text-free.
        log_viewer_source = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/log_viewer.rs'
            and suffix.startswith(('display)', 'session)', 'detail)', 'status)', 'self.query.clone())', 'list)', 'self.log_scrollbar('))
        )
        # Passive track channel wells and their wrapper contain no text; the
        # tooltip still passes through theme::text at its own child boundary.
        track_meter_component = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/track_meters.rs'
            and suffix.startswith(('bar)', 'bars.id('))
        )
        plugin_parameter_search = (
            path.relative_to(ROOT).as_posix() == 'crates/hex-ui/src/editor/device_panel/vst3_parameters.rs'
            and suffix.startswith('panel.search.clone())')
        )
        if not suffix.startswith(widgets) and not typed_error and not fixture_canvas and not header_mixer_component and not clip_detail_component and not console_column_component and not reverb_component and not browser_vector_component and not browser_scrollbar_component and not transport_vector_component and not limiter_component and not device_insertion_component and not log_viewer_source and not plugin_parameter_search and not track_meter_component:
            line = source.count('\n', 0, match.start()) + 1
            errors.append(f'{path.relative_to(ROOT)}:{line}: route text through theme::text or add an explicit view component')
    for match in re.finditer(r'\.font_family\(([^)]*)\)', source):
        if match.group(1).strip() != 'theme::FONT_FAMILY':
            errors.append(f'{path.relative_to(ROOT)}: use the required theme font')
# A generic lowercase pass over either error surface would corrupt source paths.
error_status = (ROOT / 'crates/hex-ui/src/editor/render.rs').read_text()
error_tooltip = (ROOT / 'crates/hex-ui/src/editor/display_error.rs').read_text()
if '.child(error.display_text())' not in error_status or 'ErrorTooltip(message.clone())' not in error_status:
    errors.append('editor/render.rs: status and tooltip must use typed source-preserving error text')
if '.child(self.display_text())' not in error_tooltip or 'self.0.display_text()' not in error_tooltip:
    errors.append('editor/display_error.rs: error tooltip must share source-preserving status text')
for root_view in ('app.rs', 'editor/render.rs'):
    app = (ROOT / 'crates/hex-ui/src' / root_view).read_text()
    if '.font_family(theme::FONT_FAMILY)' not in app:
        errors.append(f'{root_view}: root UI must inherit Ioskeley Mono')
# The shared top toolbar reserves space for native window controls;
# one EditorView renders both pending-backend startup and the loaded editor.
# The generated demo retains its separate toolbar with the same leading inset.
header = (ROOT / 'crates/hex-ui/src/editor/header.rs').read_text()
if '.pl(px(theme::TITLEBAR_LEADING_INSET))' not in header:
    errors.append('editor/header.rs: reserve the native-control leading inset')
for top_row in ('editor/render.rs',):
    source = (ROOT / 'crates/hex-ui/src' / top_row).read_text()
    if 'self.transport(window, cx)' not in source:
        errors.append(f'{top_row}: use the shared top toolbar')
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
