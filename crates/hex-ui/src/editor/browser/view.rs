//! Browser panel and row rendering, floating audio drag ghosts, and project opening.
use super::*;

impl EditorView {
    pub(in crate::editor) fn floating_browser_audio_drag_ghost(&self, scale: f32) -> Option<Div> {
        let payload = self
            .active_browser_audio_drag
            .as_ref()
            .filter(|drag| self.browser_audio_drag_is_current(drag))?;
        if let Some(drag::BrowserDropDestination::NewTrack {
            before_track: None,
            start,
        }) = payload.destination.get()
        {
            let state = self.state.as_ref()?;
            let body = self.arrangement_bounds?;
            let viewport = self.visible_viewport(state.timeline_frames);
            let timeline_width = body.width - self.track_header_width();
            if !timeline_width.is_finite() || timeline_width <= 0.0 || viewport.length == 0 {
                return None;
            }
            let Some(end) = payload
                .metadata
                .borrow()
                .as_ref()
                .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                .and_then(|metadata| start.checked_add(metadata.playback_frames))
            else {
                let left = body.left
                    + ((start.saturating_sub(viewport.start).min(viewport.length)) as f64
                        / viewport.length as f64
                        * f64::from(timeline_width)) as f32;
                let top = if state.project.tracks().is_empty() {
                    body.top
                } else {
                    payload.insertion_line_y.get().unwrap_or(body.top) + 1.0
                };
                let height = (body.top + body.height - top).clamp(0.0, 44.0);
                if height <= 0.0 {
                    return None;
                }
                return Some(browser_audio_drag_ghost(
                    payload,
                    BrowserGhostGeometry {
                        left,
                        top,
                        width: 3.0,
                        height,
                        scale,
                    },
                    BrowserGhostDecor {
                        selector: "browser-audio-append-preview",
                        notched: false,
                        grid: None,
                        waveform_visual: None,
                    },
                ));
            };
            let visible_start = start.max(viewport.start);
            let visible_end = end.min(viewport.end()?);
            if visible_start >= visible_end {
                return None;
            }
            let left = body.left
                + ((visible_start - viewport.start) as f64 / viewport.length as f64
                    * f64::from(timeline_width)) as f32;
            let width = (visible_end - visible_start) as f64 / viewport.length as f64
                * f64::from(timeline_width);
            let top = if state.project.tracks().is_empty() {
                body.top
            } else {
                payload.insertion_line_y.get().unwrap_or(body.top) + 1.0
            };
            let height = (body.top + body.height - top).clamp(0.0, 44.0);
            if height <= 0.0 {
                return None;
            }
            return Some(browser_audio_drag_ghost(
                payload,
                BrowserGhostGeometry {
                    left,
                    top,
                    width: width as f32,
                    height,
                    scale,
                },
                BrowserGhostDecor {
                    selector: "browser-audio-append-preview",
                    notched: visible_end == end,
                    grid: Some((
                        super::super::musical_time::grid_lines(
                            &state.project,
                            viewport,
                            timeline_width,
                            self.snap_grid,
                        ),
                        visible_start,
                        visible_end,
                    )),
                    waveform_visual: payload
                        .metadata
                        .borrow()
                        .as_ref()
                        .filter(|metadata| metadata.sample_rate == state.project.sample_rate())
                        .map(|metadata| {
                            super::super::waveform::waveform_source_segment_view_cached(
                                Arc::clone(&metadata.waveform),
                                start,
                                visible_start,
                                visible_end,
                                std::rc::Rc::clone(&self.waveform_geometry_cache),
                                crate::theme::color(crate::theme::PaletteRole::Waveform),
                            )
                        }),
                },
            ));
        }
        if payload.lane_hovered.get() {
            return Some(
                div()
                    .debug_selector(|| "browser-audio-floating-preview-hidden".to_owned())
                    .absolute()
                    .left_0()
                    .top_0()
                    .w(px(0.0))
                    .h(px(0.0)),
            );
        }
        let (pointer_x, pointer_y) = payload.last_pointer.get()?;
        let (left, top) = browser_audio_drag_ghost_position((pointer_x, pointer_y));
        Some(browser_audio_drag_ghost(
            payload,
            BrowserGhostGeometry {
                left,
                top,
                width: 184.0,
                height: 36.0,
                scale,
            },
            BrowserGhostDecor {
                selector: "browser-audio-floating-preview",
                notched: true,
                grid: None,
                waveform_visual: None,
            },
        ))
    }

    pub(super) fn start_browser_scan(&mut self, request: ScanRequest, cx: &mut Context<Self>) {
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let path_for_scan = request.directory.clone();
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    scan_directory(path_for_scan)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.browser_scan_finished(request, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn browser_scan_finished(
        &mut self,
        request: ScanRequest,
        result: Result<DirectoryListing, ScanError>,
        cx: &mut Context<Self>,
    ) {
        let (is_current, pending) = self.browser.scans.complete(request.generation);
        let mut next = pending;
        if is_current {
            let selection = if self.browser.loading
                && matches!(self.browser.section, BrowserSection::Place(_))
            {
                self.browser.pending_selection.take()
            } else {
                self.browser_selected_item(cx)
            };
            self.browser.samples.invalidate_listing();
            match result {
                Ok(listing) => {
                    self.browser.listing = Some(listing);
                    self.browser.error = None;
                    self.browser.loading = false;
                }
                Err(_)
                    if !matches!(self.browser.section, BrowserSection::Place(_))
                        && request.directory == self.browser.music
                        && self.browser.music != self.browser.home
                        && next.is_none() =>
                {
                    let fallback = self.browser.home.clone();
                    self.browser.directory = fallback.clone();
                    self.browser.loading = true;
                    next = self.browser.scans.request(fallback);
                }
                Err(error) => {
                    self.browser.listing = None;
                    self.browser.error = Some(
                        hex_session::service::text::lowercase_outside_verbatim(&error.to_string()),
                    );
                    self.browser.loading = false;
                }
            }
            self.restore_browser_selection(selection, cx);
        }
        if let Some(request) = next {
            self.start_browser_scan(request, cx);
        }
    }

    pub(in crate::editor) fn browser_view(
        &mut self,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let width = self.browser_content_width(window);
        let content = self.browser_content_view(window, cx);
        hex_widgets::panel_frame(theme::workspace_panel_style(), content)
            .w(px(width))
            .flex_shrink_0()
    }

    pub(super) fn browser_content_view(&mut self, window: &Window, cx: &mut Context<Self>) -> Div {
        self.browser.scrollbar.begin_render();
        if !self.browser.open {
            let rail = div()
                .w(px(COLLAPSED_WIDTH))
                .flex_shrink_0()
                .flex()
                .flex_col()
                .items_center()
                .pt_1()
                .gap_1()
                .bg(rgb(theme::color(theme::PaletteRole::Surface)))
                .child(self.browser_icon(
                    "▤",
                    "open browser · cmd+alt+b",
                    |view, _, cx| {
                        view.browser.open = true;
                        cx.notify();
                    },
                    cx,
                ));
            return rail;
        }

        self.reconcile_browser_source(cx);
        self.reconcile_browser_tree_requests(cx);
        self.restore_browser_history_position(cx);
        let path = self.browser.directory.clone();
        let folder_dialog = self.browser_folder_button(cx);
        let control = self.browser_random_button(cx);
        let folder_dialog = div()
            .flex()
            .items_center()
            .gap_1()
            .child(folder_dialog)
            .child(control);
        let section = self.browser.section.clone();
        if matches!(section, BrowserSection::All | BrowserSection::Instruments)
            && self.browser.device_catalog.is_none()
        {
            let selection = self.browser_selected_item(cx);
            let catalog = (|| {
                let mut entries = Vec::new();
                for class in [
                    hex_engine::NativeDeviceClass::Instrument,
                    hex_engine::NativeDeviceClass::Effect,
                ] {
                    entries.extend(
                        hex_engine::native_device_catalog_for_class(class)
                            .map_err(|error| {
                                hex_session::service::text::lowercase_outside_verbatim(
                                    &error.to_string(),
                                )
                            })?
                            .into_iter()
                            .map(|entry| (class, entry)),
                    );
                }
                Ok(entries)
            })();
            self.browser.device_catalog = Some(catalog);
            if selection.is_some() {
                self.restore_browser_selection(selection, cx);
            }
        }
        let entries = self.browser_visible_entries(cx);
        let devices = if section == BrowserSection::All {
            self.browser_visible_devices(cx)
        } else {
            Vec::new()
        };
        let project_samples =
            (section == BrowserSection::ProjectSamples).then(|| self.browser_project_samples(cx));
        let file_count = project_samples
            .as_ref()
            .map_or(entries.len(), |rows| rows.len());
        let device_count = devices.len();
        let plugins = self.browser_visible_plugins(cx);
        let project_context = self
            .state
            .as_ref()
            .map(|state| (self.project_generation, state.revision));
        let plugin_groups =
            (section == BrowserSection::Plugins).then(|| plugins::PluginGroups::new(&plugins));
        let row_count = file_count
            + device_count
            + plugins.len()
            + plugin_groups
                .as_ref()
                .map_or(0, plugins::PluginGroups::heading_count);
        let positions = BrowserRows::new(
            file_count,
            device_count,
            plugins.len(),
            self.browser_devices_first(cx),
        );
        let rows = uniform_list(
            "library-browser-entries",
            row_count,
            cx.processor(move |view, range: std::ops::Range<usize>, _window, cx| {
                range
                    .filter_map(|index| {
                        if let Some(groups) = &plugin_groups {
                            match groups.line(index)? {
                                plugins::PluginLine::Heading(group, _) => {
                                    Some(Self::browser_plugin_heading(group))
                                }
                                plugins::PluginLine::Plugin(logical) => plugins
                                    .get(logical)
                                    .cloned()
                                    .map(|row| view.browser_plugin_row(logical, row, cx)),
                            }
                        } else if positions.files.contains(&index) {
                            if let (Some(samples), Some((generation, revision))) =
                                (&project_samples, project_context)
                            {
                                samples
                                    .get(index - positions.files.start)
                                    .cloned()
                                    .map(|row| {
                                        view.browser_project_sample_row(
                                            index, generation, revision, row, cx,
                                        )
                                    })
                            } else {
                                entries
                                    .get(index - positions.files.start)
                                    .cloned()
                                    .map(|entry| view.browser_row(index, entry, cx))
                            }
                        } else if positions.devices.contains(&index) {
                            devices.get(index - positions.devices.start).cloned().map(
                                |(class, entry)| view.browser_device_row(index, class, entry, cx),
                            )
                        } else {
                            plugins
                                .get(index.checked_sub(positions.plugins.start)?)
                                .cloned()
                                .map(|row| view.browser_plugin_row(index, row, cx))
                        }
                    })
                    .collect()
            }),
        )
        .h_full()
        .min_h_0()
        .w_full()
        .track_scroll(self.browser.scroll.clone());

        let bounds = window.bounds();
        let pane_width =
            self.clamped_browser_width(self.browser.width, f32::from(bounds.size.width));
        let mut panel = div()
            .w(px(pane_width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .flex_col()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)));
        let toolbar = self.browser_toolbar(cx);
        if section == BrowserSection::Samples {
            panel = panel.child(self.sample_filters(cx));
        }
        if matches!(
            section,
            BrowserSection::All | BrowserSection::Projects | BrowserSection::Place(_)
        ) {
            panel =
                panel.child(
                    div()
                        .h(px(24.0))
                        .flex_shrink_0()
                        .px_1()
                        .flex()
                        .items_center()
                        .gap_1()
                        .child(div().flex_1().min_w_0().overflow_hidden().text_xs().child(
                            theme::text(if section == BrowserSection::All {
                                std::borrow::Cow::Borrowed("all folders")
                            } else if let Some(name) =
                                path.file_name().and_then(|name| name.to_str())
                            {
                                std::borrow::Cow::Owned(hex_session::service::text::verbatim(name))
                            } else {
                                std::borrow::Cow::Borrowed("folder")
                            }),
                        ))
                        .child(folder_dialog),
                );
        }
        panel = panel.child(self.browser_tag_selector(cx));
        panel = panel.child(
            div()
                .h(px(22.0))
                .flex_shrink_0()
                .px_2()
                .bg(rgb(theme::color(theme::PaletteRole::Hover)))
                .text_xs()
                .child(theme::text("name")),
        );
        if section == BrowserSection::All {
            let notice = self
                .browser
                .error
                .clone()
                .or_else(|| {
                    self.browser
                        .device_catalog
                        .as_ref()
                        .and_then(|catalog| catalog.as_ref().err())
                        .map(|error| format!("device catalog unavailable · {error}"))
                })
                .or_else(|| self.browser.loading.then(|| "scanning folder…".into()));
            if let Some(notice) = notice {
                panel = panel.child(
                    div()
                        .px_1()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(notice)),
                );
            }
        }
        match section {
            BrowserSection::Plugins => {
                let controls = self.plugin_controls(cx);
                panel = panel
                    .child(controls)
                    .child(self.browser_results(rows, row_count, cx));
            }
            BrowserSection::Collection(_) => {
                panel = panel.child(
                    div()
                        .flex_1()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .child(theme::text("collection assignments pending")),
                );
            }
            BrowserSection::Instruments => {
                match self
                    .browser
                    .device_catalog
                    .clone()
                    .unwrap_or_else(|| Ok(Vec::new()))
                {
                    Ok(_) => {
                        let entries = self.browser_visible_devices(cx);
                        if entries.is_empty() {
                            panel = panel.child(
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .px_2()
                                    .pt_2()
                                    .text_xs()
                                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                                    .child(theme::text("no matching devices")),
                            );
                        } else {
                            let count = entries.len();
                            let list = uniform_list(
                                "native-device-browser-entries",
                                count,
                                cx.processor(
                                    move |view, range: std::ops::Range<usize>, _window, cx| {
                                        range
                                            .filter_map(|index| {
                                                entries.get(index).cloned().map(|(class, entry)| {
                                                    view.browser_device_row(index, class, entry, cx)
                                                })
                                            })
                                            .collect()
                                    },
                                ),
                            )
                            .h_full()
                            .min_h_0()
                            .w_full()
                            .track_scroll(self.browser.scroll.clone())
                            .flex_1();
                            panel = panel.child(self.browser_results(list, count, cx));
                        }
                    }
                    Err(error) => {
                        panel = panel.child(
                            div()
                                .flex_1()
                                .min_h_0()
                                .px_2()
                                .pt_2()
                                .text_xs()
                                .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                                .child(theme::text(format!(
                                    "device catalog unavailable · {error}"
                                ))),
                        );
                    }
                }
            }
            BrowserSection::Samples => {
                if row_count == 0 {
                    panel = panel.child(
                        div()
                            .flex_1()
                            .px_2()
                            .pt_2()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text(
                                "no matching samples · add a folder or change filters",
                            )),
                    );
                } else {
                    panel = panel.child(self.browser_results(rows, row_count, cx));
                }
            }
            _ if row_count == 0
                && self.browser.loading
                && (matches!(self.browser.section, BrowserSection::Place(_))
                    || self
                        .browser
                        .listing
                        .as_ref()
                        .is_none_or(|listing| listing.directory != path)) =>
            {
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text("scanning folder…")),
                );
            }
            _ if row_count == 0 && self.browser.error.is_some() => {
                let error = self.browser.error.clone().unwrap_or_default();
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Primary)))
                        .child(theme::text(error)),
                );
            }
            _ if row_count == 0 => {
                let empty = if self
                    .browser
                    .listing
                    .as_ref()
                    .is_some_and(|listing| listing.truncated)
                {
                    "no matching items in first 2048 entries"
                } else {
                    "no matching items"
                };
                panel = panel.child(
                    div()
                        .flex_1()
                        .min_h_0()
                        .px_2()
                        .pt_2()
                        .text_xs()
                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                        .child(theme::text(empty)),
                );
            }
            _ => {
                panel = panel.child(self.browser_results(rows, row_count, cx));
                if self
                    .browser
                    .listing
                    .as_ref()
                    .is_some_and(|listing| listing.truncated)
                {
                    panel = panel.child(
                        div()
                            .h(px(18.0))
                            .flex_shrink_0()
                            .px_1()
                            .text_xs()
                            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                            .child(theme::text("listing capped · open a smaller folder")),
                    );
                }
            }
        }

        if let Some(status) = self.browser.tree.status.clone() {
            panel = panel.child(
                div()
                    .h(px(18.0))
                    .flex_shrink_0()
                    .px_1()
                    .overflow_hidden()
                    .text_xs()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(theme::text(status)),
            );
        }
        let preview = self.browser_selection_preview(cx);
        let sidebar = self.browser_sidebar(pane_width, cx);
        let editor = self.browser_tag_editor(pane_width, cx);
        let editor_width = self.browser.layout.editor_width(pane_width);
        panel = div()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .flex()
            .flex_col()
            .child(panel.w_full().flex_1().min_h_0().min_w_0())
            .child(preview);
        // The rail belongs to the fixed base column. The optional editor is a
        // sibling of that WHOLE column, so neither opening control can move.
        panel = div()
            .w(px(pane_width + editor_width))
            .flex_shrink_0()
            .min_h_0()
            .flex()
            .bg(rgb(theme::color(theme::PaletteRole::Surface)))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .child(
                div()
                    .w(px(pane_width))
                    .flex_shrink_0()
                    .min_h_0()
                    .flex()
                    .flex_col()
                    .child(toolbar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .child(sidebar)
                            .child(self.browser_sidebar_resize(pane_width, cx))
                            .child(panel),
                    ),
            )
            .children(editor);

        let entity = cx.entity();
        let mut resize = div()
            .id("browser-resize-handle")
            .debug_selector(|| "browser-resize-handle".to_owned())
            .h_full()
            .w(px(4.0))
            .flex_shrink_0()
            .cursor_col_resize()
            .relative()
            .group("browser-resize-handle")
            .child(layout::resize_divider_line(
                "browser-resize-handle",
                "browser-resize-line",
                self.browser.resizing,
            ));
        resize = resize
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event, window, cx| {
                    view.begin_browser_resize(
                        resize::Divider::Outer,
                        event,
                        pane_width,
                        window,
                        cx,
                    );
                }),
            )
            .child(
                canvas(
                    |_, _, _| (),
                    move |_, _, window, cx| {
                        entity.read(cx).renew_browser_resize(
                            resize::Divider::Outer,
                            window,
                            entity.entity_id(),
                        );
                    },
                )
                .size_full(),
            );

        div()
            .track_focus(&self.browser.results_focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, window, cx| {
                    // Only explicit text entry keeps editing focus. Descendant text
                    // inputs stop this local bubble; no global pointer interception.
                    if view.browser.open {
                        view.browser.results_focus.focus(window);
                    } else {
                        view.focus.focus(window);
                    }
                    window.prevent_default();
                    cx.notify();
                }),
            )
            .flex()
            .min_h_0()
            .flex_shrink_0()
            .child(panel)
            .child(
                div()
                    .w(px(4.0))
                    .h_full()
                    .flex_shrink_0()
                    .min_h_0()
                    .child(resize),
            )
    }

    pub(super) fn with_browser_audio_drag(
        row: gpui::Stateful<Div>,
        payload: drag::BrowserAudioDrag,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let drag_entity = cx.entity();
        let mut row = row.on_drag(payload.clone(), move |payload, _, window, cx| {
            let position = window.mouse_position();
            payload.snap_modifier.set(window.modifiers().platform);
            payload.copy_modifier.set(window.modifiers().alt);
            cx.set_active_drag_cursor_style(drag::copy_cursor(payload.copy_modifier.get()), window);
            payload
                .last_pointer
                .set(Some((f32::from(position.x), f32::from(position.y))));
            let (preview, lane_hovered) = drag_entity.update(cx, |view, cx| {
                view.begin_browser_audio_drag(payload);
                if payload.project_asset.is_none() {
                    view.request_browser_audio_preview_for_drag(payload, cx);
                }
                view.browser_audio_drop_position(
                    payload,
                    f32::from(position.x),
                    f32::from(position.y),
                )
            });
            payload.preview.set(preview);
            payload.lane_hovered.set(lane_hovered);
            drag_entity.update(cx, |_, cx| cx.notify());
            cx.new(|_| BrowserAudioDragGhost)
        });
        let move_entity = cx.entity();
        row = row.on_drag_move::<drag::BrowserAudioDrag>(move |event, window, cx| {
            let Some(payload) = event
                .dragged_item()
                .downcast_ref::<drag::BrowserAudioDrag>()
            else {
                return;
            };
            let pointer = event.event.position;
            let pointer_xy = (f32::from(pointer.x), f32::from(pointer.y));
            payload.last_pointer.set(Some(pointer_xy));
            payload.snap_modifier.set(event.event.modifiers.platform);
            payload.copy_modifier.set(event.event.modifiers.alt);
            cx.set_active_drag_cursor_style(drag::copy_cursor(payload.copy_modifier.get()), window);
            let (preview, lane_hovered) = move_entity.update(cx, |view, _| {
                view.browser_audio_drop_position(payload, pointer_xy.0, pointer_xy.1)
            });
            payload.preview.set(preview);
            payload.lane_hovered.set(lane_hovered);
            move_entity.update(cx, |_, cx| cx.notify());
        });
        let release_entity = cx.entity();
        let release_payload = payload.clone();
        row = row.on_mouse_up(MouseButton::Left, move |_, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
            release_entity.update(cx, |view, cx| {
                view.clear_browser_audio_drag(&release_payload);
                cx.notify();
            });
        });
        row
    }

    pub(super) fn browser_project_sample_row(
        &mut self,
        index: usize,
        generation: u64,
        revision: u64,
        row: ProjectSample,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let key = BrowserItemKey::ProjectSample(generation, revision, row.asset);
        let selected = self.project_sample_current(&key);
        let payload = self.state.as_ref().and_then(|state| {
            if generation != self.project_generation || revision != state.revision {
                return None;
            }
            let asset = state.project.asset(row.asset)?;
            let frames = state.project.existing_import_frames(row.asset).ok()?;
            let metadata = Rc::new(RefCell::new(state.waveforms.get(&row.asset).map(
                |waveform| {
                    Rc::new(AudioPreview {
                        playback_frames: frames,
                        sample_rate: state.project.sample_rate(),
                        waveform: waveform.clone(),
                    })
                },
            )));
            let mut payload = self.audio_file_drag_payload(
                asset.path().to_path_buf(),
                row.name.clone(),
                None,
                metadata,
                Default::default(),
            )?;
            payload.project_asset = Some(row.asset);
            Some(payload)
        });
        let row = result_row(selected)
            .id(("browser-project-sample", row.asset.value()))
            .debug_selector(move || format!("browser-project-sample-{index}"))
            .cursor_pointer()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    let Some(current) = view.browser_history_index(&key, cx) else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.select_project_sample(Some(key.clone()), cx);
                    cx.notify();
                }),
            )
            .child(result_label(row.name, selected, theme::PaletteRole::Text));
        if let Some(payload) = payload {
            Self::with_browser_audio_drag(row, payload, cx)
        } else {
            row
        }
    }

    pub(super) fn browser_row(
        &mut self,
        index: usize,
        entry: LibraryEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let (disclosure, icon) = match entry.kind {
            EntryKind::Directory => match self.browser.tree.disclosure(&entry.path) {
                tree::Disclosure::Loading => (Some("…"), None),
                tree::Disclosure::Error => (Some("!"), None),
                tree::Disclosure::Expanded => (None, Some("icons/browser-disclosure/expanded.svg")),
                tree::Disclosure::Collapsed => {
                    (None, Some("icons/browser-disclosure/collapsed.svg"))
                }
            },
            EntryKind::WaveAudio => (None, Some("icons/browser-place/samples.svg")),
            EntryKind::Project => (None, Some("icons/browser-place/projects.svg")),
        };
        let path = entry.path.clone();
        let name = entry.name.clone();
        let drag_payload = if entry.kind == EntryKind::WaveAudio {
            self.state.as_ref().and_then(|state| {
                // Rendering a row reads only the existing metadata cache.
                let slot = self
                    .browser
                    .preview_cache
                    .get(&PreviewKey {
                        path: path.clone(),
                        sample_rate: state.project.sample_rate(),
                    })
                    .cloned()
                    .unwrap_or_default();
                self.audio_file_drag_payload(
                    path.clone(),
                    name.clone(),
                    Some(self.browser.scan_generation()),
                    slot.metadata,
                    slot.error,
                )
            })
        } else {
            None
        };
        let selected = self.browser.results_focused && self.browser.selected_index == index;
        let click_path = path.clone();
        let click_kind = entry.kind;
        let mut row = result_row(selected)
            .id(gpui::SharedString::from(tree::row_id(&path)))
            .debug_selector(|| format!("browser-entry-{index}"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    // Even a stale painted row leaves query editing; availability
                    // still gates every selection/audition/import action below.
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    if view.browser_place_unavailable() {
                        cx.notify();
                        cx.stop_propagation();
                        return;
                    }
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::File(click_path.clone()), cx)
                    else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser_selection_audition(
                        (click_kind == EntryKind::WaveAudio).then(|| click_path.clone()),
                        cx,
                    );
                    cx.notify();
                    if event.click_count >= 2 {
                        match click_kind {
                            EntryKind::Directory => view.toggle_browser_folder(
                                click_path.clone(),
                                event.modifiers.platform,
                                cx,
                            ),
                            EntryKind::WaveAudio => {
                                // Sample double-click remains selection/audition.
                                // Import is an explicit Enter or drag/drop action.
                                cx.stop_propagation();
                            }
                            EntryKind::Project => view.open_browser_project(click_path.clone(), cx),
                        }
                    }
                }),
            );
        if entry.kind == EntryKind::WaveAudio {
            let sampler_path = path.clone();
            row = row.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    window.prevent_default();
                    cx.stop_propagation();
                    if view.browser_place_unavailable() {
                        return;
                    }
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::File(sampler_path.clone()), cx)
                    else {
                        return;
                    };
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.open_first_sampler_menu(sampler_path.clone(), event.position, window, cx);
                }),
            );
        }
        if let Some(payload) = drag_payload {
            let hover_path = path.clone();
            let sample_rate = self
                .state
                .as_ref()
                .map_or(48_000, |state| state.project.sample_rate());
            let hover_entity = cx.entity();
            row = row.on_hover(move |hovered, _, cx| {
                if *hovered {
                    hover_entity.update(cx, |view, cx| {
                        view.request_browser_audio_preview(hover_path.clone(), sample_rate, cx);
                    });
                }
            });
            row = Self::with_browser_audio_drag(row, payload, cx);
        }
        if entry.kind == EntryKind::Directory && !self.browser_place_unavailable() {
            let payload = self.browser_folder_drag(path.clone(), name);
            row = row.on_drag(payload, |payload, _, _, cx| {
                cx.new(|_| folder_places::FolderGhost(payload.label.clone()))
            });
        }
        let depth = self.browser.tree.depth(&path);
        let mut control = div()
            .w(px(16.0))
            .flex_shrink_0()
            .h(px(ROW_HEIGHT))
            .flex()
            .items_center()
            .text_size(px(10.0))
            .line_height(px(ROW_HEIGHT))
            .text_color(rgb(selection_ink(selected, theme::PaletteRole::Muted)))
            .children(disclosure.map(theme::text))
            .children(icon.map(|path| {
                gpui::svg()
                    .path(path)
                    .size(px(14.0))
                    .text_color(rgb(selection_ink(selected, theme::PaletteRole::Muted)))
            }));
        if icon.is_some() && entry.kind != EntryKind::Directory {
            control = control.debug_selector(move || format!("browser-file-icon-{index}"));
        }
        if entry.kind == EntryKind::Directory {
            let folder = path.clone();
            control = control
                .debug_selector(move || format!("browser-disclosure-{index}"))
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                        view.toggle_browser_folder(folder.clone(), event.modifiers.platform, cx);
                        view.browser.results_focus.focus(window);
                        window.prevent_default();
                        cx.stop_propagation();
                    }),
                );
        }
        let tooltip = self.browser.tree.error(&path).map_or_else(
            || path.to_string_lossy().into_owned(),
            |error| {
                format!(
                    "{} · {error}",
                    hex_session::service::text::verbatim(path.display())
                )
            },
        );
        row.tooltip(move |_, cx| layout::browser_label_tooltip(&tooltip, cx))
            .child(
                div()
                    .w(px((depth as f32 * 12.0).min(96.0)))
                    .max_w(relative(0.4))
                    .flex_shrink_0(),
            )
            .child(control)
            .child(
                result_label(entry.name, selected, theme::PaletteRole::Text)
                    .debug_selector(move || format!("browser-entry-label-{index}")),
            )
    }

    pub(super) fn browser_device_row(
        &mut self,
        index: usize,
        class: hex_engine::NativeDeviceClass,
        entry: hex_engine::NativeDeviceCatalogEntry,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let class_label = match class {
            hex_engine::NativeDeviceClass::Instrument => "instrument",
            hex_engine::NativeDeviceClass::Effect => "effect",
        };
        let type_id = entry.type_id;
        let debug_type = type_id.as_str().unwrap_or("invalid").to_owned();
        let name = entry.name.to_lowercase();
        let payload = self.state.as_ref().map(|state| drag::NativeDeviceDrag {
            project_generation: self.project_generation,
            revision: state.revision,
            source: drag::NativeDeviceDragSource::Browser { class, type_id },
            destination: Rc::new(Cell::new(None)),
            return_destination: Rc::new(Cell::new(None)),
        });
        let label_id = format!("browser-device-label-{class_label}-{debug_type}");
        let selected = self.browser.results_focused && self.browser.selected_index == index;
        let row = result_row(selected)
            .id(gpui::SharedString::from(format!(
                "browser-device-{class_label}-{debug_type}"
            )))
            .debug_selector(move || format!("browser-device-{class_label}-{debug_type}"))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, event: &MouseDownEvent, window, cx| {
                    let Some(current) =
                        view.browser_history_index(&BrowserItemKey::Device(class, type_id), cx)
                    else {
                        return;
                    };
                    view.checkpoint_browser_history(cx);
                    view.browser.selected_index = current;
                    view.browser.results_focused = true;
                    view.browser.results_focus.focus(window);
                    window.prevent_default();
                    if event.click_count == 2 {
                        let request = view
                            .selected_track
                            .ok_or("select a track before adding a device")
                            .and_then(|track| {
                                view.browser_device_append_request(track, class, type_id, cx)
                            });
                        match request {
                            Ok(request) => {
                                if view.enqueue_catalog_device(request) {
                                    view.error = None;
                                    view.status = "adding device to track".into();
                                }
                            }
                            Err(reason) => view.error = Some(reason.into()),
                        }
                        cx.stop_propagation();
                    }
                    cx.notify();
                }),
            )
            .child(
                div()
                    .w(px(16.0))
                    .h(px(ROW_HEIGHT))
                    .text_size(px(10.0))
                    .line_height(px(ROW_HEIGHT))
                    .text_color(rgb(selection_ink(
                        selected,
                        theme::PaletteRole::SecondaryText,
                    )))
                    .child(theme::text(
                        if class == hex_engine::NativeDeviceClass::Instrument {
                            "⌘"
                        } else {
                            "◈"
                        },
                    )),
            )
            .child(
                result_label(name, selected, theme::PaletteRole::Text)
                    .debug_selector(move || label_id.clone()),
            );
        Self::with_browser_device_drag(row, payload, cx)
    }

    pub(super) fn with_browser_device_drag(
        row: gpui::Stateful<Div>,
        payload: Option<drag::NativeDeviceDrag>,
        cx: &Context<Self>,
    ) -> gpui::Stateful<Div> {
        let Some(payload) = payload else {
            return row;
        };
        let drag_entity = cx.entity();
        let release_entity = drag_entity.clone();
        row.on_drag(payload.clone(), move |payload, _, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::ClosedHand, window);
            drag_entity.update(cx, |view, cx| {
                view.begin_native_device_drag(payload);
                cx.notify();
            });
            cx.new(|_| BrowserAudioDragGhost)
        })
        .on_mouse_up(MouseButton::Left, move |_, window, cx| {
            cx.set_active_drag_cursor_style(gpui::CursorStyle::Arrow, window);
            release_entity.update(cx, |view, cx| {
                view.clear_native_device_drag(&payload);
                cx.notify();
            });
        })
    }

    pub(super) fn browser_icon(
        &self,
        label: &'static str,
        tooltip: &'static str,
        action: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        let description = theme::text(tooltip);
        hex_widgets::action(theme::text(label), theme::action_style())
            .id(label)
            .debug_selector(move || format!("browser-control-{label}"))
            .w(px(22.0))
            .h(px(22.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(move |view, _, window, cx| {
                    view.browser.results_focused = false;
                    action(view, window, cx);
                }),
            )
    }

    pub(super) fn browser_folder_button(&self, cx: &mut Context<Self>) -> gpui::Stateful<Div> {
        let description = theme::text("choose a folder");
        hex_widgets::action(theme::text("⌂"), theme::action_style())
            .id("browser-folder-picker")
            .w(px(22.0))
            .h(px(22.0))
            .text_xs()
            .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
            .tooltip(move |_, cx| {
                cx.new(|_| super::super::transport::MiniTooltip {
                    kind: crate::tooltips::Kind::Identification,
                    description: description.clone(),
                })
                .into()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|view, _, _, cx| view.open_browser_folder(cx)),
            )
    }

    pub(super) fn open_browser_folder(&mut self, cx: &mut Context<Self>) {
        let response = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("add browser folder".into()),
        });
        cx.spawn(async move |this, cx| {
            let picked = response.await;
            let _ = this.update(cx, |view, cx| {
                match picked {
                    Ok(Ok(Some(mut paths))) if !paths.is_empty() => {
                        if let Some(path) = paths.pop() {
                            if !view.browser.places.contains(&path) {
                                if view.browser.places.len() < PLACE_CAPACITY {
                                    view.browser.places.push(path.clone());
                                    view.queue_browser_places_save(cx);
                                } else {
                                    view.browser.places_status =
                                        Some("browser folder limit reached".into());
                                }
                            }
                            if view.browser.places.contains(&path)
                                && !view.browser_place_visible(&path)
                            {
                                view.set_browser_source_visible(
                                    BrowserSection::Place(path.clone()),
                                    true,
                                    cx,
                                );
                            }
                            view.refresh_sample_catalog(cx);
                            if view.browser.section != BrowserSection::Samples
                                && view.browser.places.contains(&path)
                                && view.browser_place_visible(&path)
                            {
                                view.select_browser_source(BrowserSection::Place(path), cx);
                            }
                        }
                    }
                    Ok(Err(error)) => {
                        view.error = Some(
                            hex_session::service::text::lowercase_outside_verbatim(
                                &error.to_string(),
                            )
                            .into(),
                        )
                    }
                    _ => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    pub(super) fn open_browser_project(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if self.browser_place_unavailable() {
            return;
        }
        self.choose_project(path, hex_session::DocumentAudioMode::Resident);
        cx.notify();
    }
}
