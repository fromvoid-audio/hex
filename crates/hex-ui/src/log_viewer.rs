//! Native, control-side view of bounded diagnostic session snapshots.
use crate::theme;
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size, uniform_list,
};
use hex_diagnostics::reader::{LogFilter, LogLine, LogSnapshot, read_session_page};
use hex_widgets::{TextInput, TextInputChanged, TextInputConfig, TextInputKind};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const CATEGORIES: &[&str] = &[
    "all",
    "uncategorised",
    "startup",
    "ui",
    "session",
    "audio",
    "transport",
    "persistence",
    "plugin",
    "media",
];
const WINDOWS: &[(&str, Option<u64>)] = &[
    ("entire session", None),
    ("last 10 s", Some(10)),
    ("last 30 s", Some(30)),
    ("last 1 min", Some(60)),
    ("last 5 min", Some(300)),
];
const COPY_LIMIT: usize = 1024 * 1024;
const COPY_CLIPPED: &str = "[excerpt clipped at 1 mib]\n";

mod scrollbar;

/// One action's owned filters, with a fixed inclusive relative-time window.
/// Moving between pages never reanchors a range to that page's last event.
struct ViewerFilter {
    category: &'static str,
    query: String,
    window: &'static str,
    since: Option<u128>,
    until: Option<u128>,
}
impl ViewerFilter {
    fn new(
        category: &'static str,
        query: String,
        window: (&'static str, Option<u64>),
        now: u128,
    ) -> Self {
        Self {
            category,
            query,
            window: window.0,
            since: window
                .1
                .map(|seconds| now.saturating_sub(Duration::from_secs(seconds).as_nanos())),
            until: window.1.map(|_| now),
        }
    }
    fn borrowed(&self) -> LogFilter<'_> {
        LogFilter {
            category: (self.category != "all").then_some(self.category),
            text: (!self.query.is_empty()).then_some(self.query.as_str()),
            since_unix_nanos: self.since,
            until_unix_nanos: self.until,
        }
    }
}

struct FilteredCopy {
    text: String,
    matching_events: usize,
    warnings: bool,
    clipped: bool,
}

struct LogInputBindings;
impl gpui::Global for LogInputBindings {}

pub(crate) fn open(path: PathBuf, cx: &mut App) -> Result<(), String> {
    if cx.try_global::<LogInputBindings>().is_none() {
        hex_widgets::bind_text_input_keys(cx);
        cx.set_global(LogInputBindings);
    }
    let bounds = Bounds::centered(None, size(px(1000.0), px(650.0)), cx);
    cx.open_window(
        WindowOptions {
            titlebar: Some(gpui::TitlebarOptions {
                title: Some("hex · log".into()),
                appears_transparent: true,
                ..Default::default()
            }),
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            window_min_size: Some(size(px(850.0), px(350.0))),
            ..Default::default()
        },
        move |_, cx| cx.new(|cx| Viewer::new(path, cx)),
    )
    .map(|_| ())
    .map_err(|error| format!("could not open log viewer: {error}"))
}

struct Viewer {
    path: PathBuf,
    sessions: Vec<PathBuf>,
    snapshot: Option<Arc<LogSnapshot>>,
    page: usize,
    has_older: bool,
    has_newer: bool,
    rows: Arc<Vec<usize>>,
    query: Entity<TextInput>,
    _query_subscription: Subscription,
    category: usize,
    window: usize,
    generation: u64,
    loading: bool,
    reload_pending: bool,
    filtering: bool,
    filter_pending: bool,
    copying: bool,
    anchor: Option<usize>,
    selection: Option<usize>,
    status: String,
    scroll: UniformListScrollHandle,
    scrollbar: scrollbar::State,
}
impl Viewer {
    fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        let query = cx.new(|cx| {
            TextInput::new(
                cx,
                TextInputConfig {
                    id: "log-search".into(),
                    kind: TextInputKind::Search,
                    style: theme::control_style(),
                    action_style: theme::action_style(),
                    font_family: theme::FONT_FAMILY.into(),
                    placeholder: theme::text("search logs"),
                    clear_name: theme::text("clear search"),
                    clear_glyph: theme::text("×"),
                    max_bytes: 4096,
                },
            )
        });
        let subscription = cx.subscribe(&query, |view: &mut Self, _, _: &TextInputChanged, cx| {
            view.filter(cx)
        });
        let mut view = Self {
            path,
            sessions: Vec::new(),
            snapshot: None,
            page: 0,
            has_older: false,
            has_newer: false,
            rows: Arc::new(Vec::new()),
            query,
            _query_subscription: subscription,
            category: 0,
            window: 0,
            generation: 0,
            loading: false,
            reload_pending: false,
            filtering: false,
            filter_pending: false,
            copying: false,
            anchor: None,
            selection: None,
            status: "loading…".into(),
            scroll: UniformListScrollHandle::new(),
            scrollbar: scrollbar::State::default(),
        };
        view.reload(cx);
        view
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = self.path.clone();
        let requested_page = self.page;
        self.snapshot = None;
        self.rows = Arc::new(Vec::new());
        self.anchor = None;
        self.selection = None;
        self.status = "loading…".into();
        if self.loading {
            self.reload_pending = true;
            cx.notify();
            return;
        }
        self.loading = true;
        let task = cx.background_executor().spawn(async move {
            let sessions = list_sessions(&path);
            let snapshot = read_session_page(&path, requested_page).map_err(|e| e.to_string());
            (sessions, snapshot)
        });
        cx.spawn(async move |this, cx| {
            let (sessions, snapshot) = task.await;
            let _ = this.update(cx, |view, cx| {
                view.loading = false;
                if view.reload_pending {
                    view.reload_pending = false;
                    view.reload(cx);
                    return;
                }
                if view.generation != generation {
                    return;
                }
                match snapshot {
                    Ok(mut page) => {
                        match sessions {
                            Ok(sessions) => view.sessions = sessions,
                            Err(error) => {
                                view.sessions = vec![view.path.clone()];
                                page.snapshot
                                    .warnings
                                    .push(format!("could not list sessions: {error}"));
                            }
                        }
                        view.page = page.page;
                        view.has_older = page.has_older;
                        view.has_newer = page.has_newer;
                        view.snapshot = Some(Arc::new(page.snapshot));
                        view.filter(cx);
                    }
                    Err(error) => {
                        view.snapshot = None;
                        view.rows = Arc::new(Vec::new());
                        view.status = format!("could not read log: {error}");
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        cx.notify();
    }
    fn current_filter(&self, cx: &App) -> Result<ViewerFilter, String> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| {
                "could not resolve log time range: system clock is before unix epoch".to_owned()
            })?
            .as_nanos();
        Ok(ViewerFilter::new(
            CATEGORIES[self.category],
            self.query.read(cx).text().to_owned(),
            WINDOWS[self.window],
            now,
        ))
    }
    fn filter(&mut self, cx: &mut Context<Self>) {
        let Some(snapshot) = self.snapshot.clone() else {
            return;
        };
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        self.rows = Arc::new(Vec::new());
        self.anchor = None;
        self.selection = None;
        self.status = "filtering…".into();
        if self.filtering {
            self.filter_pending = true;
            cx.notify();
            return;
        }
        let request = match self.current_filter(cx) {
            Ok(request) => request,
            Err(error) => {
                self.status = error;
                cx.notify();
                return;
            }
        };
        self.filtering = true;
        let task = cx.background_executor().spawn(async move {
            let filter = request.borrowed();
            let rows = snapshot
                .lines
                .iter()
                .enumerate()
                .filter_map(|(index, line)| filter.matches(line).then_some(index))
                .collect::<Vec<_>>();
            (snapshot, rows)
        });
        cx.spawn(async move |this, cx| {
            let (snapshot, rows) = task.await;
            let _ = this.update(cx, |view, cx| {
                view.filtering = false;
                if view.filter_pending {
                    view.filter_pending = false;
                    view.filter(cx);
                    return;
                }
                if view.generation != generation {
                    return;
                }
                view.status = if snapshot.warnings.is_empty() {
                    format!("page {} · {} lines · filters apply to this page · history may shift between pages", view.page.saturating_add(1), rows.len())
                } else {
                    format!("page {} · {} lines · filters apply to this page · {}", view.page.saturating_add(1), rows.len(), snapshot.warnings.join(" · "))
                };
                view.rows = Arc::new(rows);
                view.anchor = None;
                view.selection = None;
                if !view.rows.is_empty() {
                    view.scroll
                        .scroll_to_item(view.rows.len() - 1, gpui::ScrollStrategy::Bottom);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn shift_session(&mut self, direction: isize, cx: &mut Context<Self>) {
        let Some(current) = self.sessions.iter().position(|path| path == &self.path) else {
            return;
        };
        let Some(next) = current
            .checked_add_signed(direction)
            .and_then(|index| self.sessions.get(index))
        else {
            return;
        };
        self.path = next.clone();
        self.page = 0;
        self.reload(cx);
    }
    fn shift_page(&mut self, older: bool, cx: &mut Context<Self>) {
        if older && self.has_older {
            if let Some(page) = self.page.checked_add(1) {
                self.page = page;
                self.reload(cx);
            }
        } else if !older && self.has_newer {
            self.page = self.page.saturating_sub(1);
            self.reload(cx);
        }
    }
    fn copy_all(&mut self, cx: &mut Context<Self>) {
        if self.copying {
            return;
        }
        let request = match self.current_filter(cx) {
            Ok(request) => request,
            Err(error) => {
                self.status = error;
                cx.notify();
                return;
            }
        };
        let path = self.path.clone();
        self.copying = true;
        self.status = "reading all retained matching logs…".into();
        let task = cx
            .background_executor()
            .spawn(async move { make_filtered_copy(&path, &request) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |view, cx| {
                view.copying = false;
                match result {
                    Ok(copy) => {
                        cx.write_to_clipboard(ClipboardItem::new_string(copy.text.clone()));
                        // GPUI's write API has no error return. Confirm the
                        // bounded payload through its read API before success.
                        let actual = cx.read_from_clipboard().and_then(|item| item.text());
                        view.status = filtered_copy_status(&copy, actual.as_deref()).into();
                    }
                    Err(error) => view.status = format!("could not copy filtered logs: {error}"),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }
    fn copy_selection(&mut self, cx: &mut Context<Self>) {
        if self.copying {
            return;
        }
        let (Some(snapshot), Some(anchor), Some(end)) =
            (self.snapshot.clone(), self.anchor, self.selection)
        else {
            self.status = "select log lines first".into();
            cx.notify();
            return;
        };
        let rows = self.rows.clone();
        let path = self.path.clone();
        let category = CATEGORIES[self.category].to_owned();
        let time = format!(
            "{} (page {})",
            WINDOWS[self.window].0,
            self.page.saturating_add(1)
        );
        let query = self.query.read(cx).text().to_owned();
        self.copying = true;
        let task = cx.background_executor().spawn(async move {
            make_excerpt(
                &snapshot,
                &rows,
                &path,
                &category,
                &time,
                &query,
                (anchor, end),
            )
        });
        cx.spawn(async move |this, cx| {
            let text = task.await;
            let clipped = text.ends_with(COPY_CLIPPED);
            let _ = this.update(cx, |view, cx| {
                view.copying = false;
                cx.write_to_clipboard(ClipboardItem::new_string(text));
                view.status = if clipped {
                    "selection copied · excerpt clipped at 1 mib"
                } else {
                    "selection copied · review before sharing"
                }
                .into();
                cx.notify();
            });
        })
        .detach();
    }
}

fn make_filtered_copy(path: &Path, request: &ViewerFilter) -> Result<FilteredCopy, String> {
    let result = hex_diagnostics::reader::read_session_filtered(path, &request.borrowed())
        .map_err(|error| error.to_string())?;
    let since = request
        .since
        .map_or_else(|| "unbounded".into(), |value| value.to_string());
    let until = request
        .until
        .map_or_else(|| "unbounded".into(), |value| value.to_string());
    let time = format!(
        "{}; inclusive unix_nanos {since}..{until}\nmatching event records: {}; omitted export entries: {}\nall retained pages; context/loss lines are kept regardless of event filters",
        request.window, result.matching_events, result.omitted_lines
    );
    let rows: Vec<_> = (0..result.snapshot.lines.len()).collect();
    let text = make_excerpt(
        &result.snapshot,
        &rows,
        path,
        request.category,
        &time,
        &request.query,
        (0, rows.len().saturating_sub(1)),
    );
    let clipped = result.omitted_lines > 0 || text.ends_with(COPY_CLIPPED);
    Ok(FilteredCopy {
        text,
        matching_events: result.matching_events,
        warnings: result.snapshot.header.is_none() || !result.snapshot.warnings.is_empty(),
        clipped,
    })
}

fn filtered_copy_status(copy: &FilteredCopy, actual: Option<&str>) -> &'static str {
    if actual != Some(copy.text.as_str()) {
        "could not verify clipboard contents · try copying again"
    } else if copy.clipped {
        "filtered logs copied · incomplete at export limit · review warnings"
    } else if copy.warnings {
        "filtered logs copied with warnings · review before sharing"
    } else if copy.matching_events == 0 {
        "no matching events · available context copied"
    } else {
        "all retained matching logs copied · review before sharing"
    }
}

fn make_excerpt(
    snapshot: &LogSnapshot,
    rows: &[usize],
    path: &Path,
    category: &str,
    time: &str,
    query: &str,
    selection: (usize, usize),
) -> String {
    let mut result = format!(
        "hex log excerpt\nsession: {}\ncategory: {category}; time: {time}; search: {query}\n",
        path.display()
    );
    if let Some(header) = &snapshot.header {
        result.push_str(header);
        result.push('\n');
    } else {
        result.push_str("warning: session header unavailable\n");
    }
    for warning in &snapshot.warnings {
        result.push_str(&format!("warning: {warning}\n"));
    }
    let content_limit = COPY_LIMIT - COPY_CLIPPED.len();
    if result.len() > content_limit {
        let mut end = content_limit;
        while !result.is_char_boundary(end) {
            end -= 1;
        }
        result.truncate(end);
        result.push_str(COPY_CLIPPED);
        return result;
    }
    let (start, end) = selection;
    for index in start.min(end)..=start.max(end) {
        let Some(line) = rows.get(index).and_then(|i| snapshot.lines.get(*i)) else {
            continue;
        };
        if result
            .len()
            .saturating_add(line.text.len())
            .saturating_add(1)
            > content_limit
        {
            result.push_str(COPY_CLIPPED);
            break;
        }
        result.push_str(&line.text);
        result.push('\n');
    }
    result
}

fn list_sessions(active: &Path) -> std::io::Result<Vec<PathBuf>> {
    let Some(directory) = active.parent() else {
        return Ok(vec![active.to_owned()]);
    };
    let mut sessions = Vec::new();
    for entry in std::fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some((stamp, pid)) = name
            .to_str()
            .and_then(|name| name.strip_prefix("session-"))
            .and_then(|name| name.strip_suffix(".log"))
            .and_then(|name| name.split_once('-'))
        else {
            continue;
        };
        if !pid.is_empty()
            && stamp.bytes().all(|b| b.is_ascii_digit())
            && pid.bytes().all(|b| b.is_ascii_digit())
        {
            if let Ok(stamp) = stamp.parse::<u128>() {
                sessions.push((stamp, entry.path()));
            }
            if sessions.len() > 1_000 {
                return Err(std::io::Error::other("too many sessions"));
            }
        }
    }
    sessions.sort_by_key(|(stamp, _)| *stamp);
    let mut paths: Vec<_> = sessions.into_iter().map(|(_, path)| path).collect();
    if !paths.iter().any(|path| path == active) {
        paths.push(active.to_owned());
    }
    Ok(paths)
}

/// Recorded classification only: message words never imply a severity. The
/// reader assigns legacy events its documented info default. Untyped lines
/// remain context; a writer loss record has its own explicit warning label.
fn level_badge(line: &LogLine) -> (&'static str, hex_project::ElementColor) {
    use hex_project::ElementColor;
    match line.event.as_ref().map(|event| event.level.as_str()) {
        Some("error") => ("error", ElementColor::Red),
        Some("warn") => ("warn", ElementColor::Amber),
        Some("info") => ("info", ElementColor::Blue),
        Some("debug") => ("debug", ElementColor::Gray),
        Some("trace") => ("trace", ElementColor::Gray),
        Some(_) => ("other", ElementColor::Gray),
        None if line.text.starts_with("diagnostics_loss ") => ("loss", ElementColor::Amber),
        None => ("context", ElementColor::Gray),
    }
}

impl Render for Viewer {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let snapshot = self.snapshot.clone();
        let rows = self.rows.clone();
        let selection = self
            .anchor
            .zip(self.selection)
            .map(|(a, b)| a.min(b)..=a.max(b));
        let list = uniform_list(
            "log-rows",
            rows.len(),
            cx.processor(move |_view, range: std::ops::Range<usize>, _, cx| {
                range
                    .filter_map(|index| {
                        let line = rows
                            .get(index)
                            .and_then(|i| snapshot.as_ref()?.lines.get(*i))?;
                        let time = line
                            .event
                            .as_ref()
                            .map(|event| format_time(event.unix_nanos))
                            .unwrap_or_default();
                        let display: String = line.text.chars().take(1800).collect();
                        let selected = selection
                            .as_ref()
                            .is_some_and(|range| range.contains(&index));
                        let background = theme::color(if selected {
                            theme::PaletteRole::Hover
                        } else {
                            theme::PaletteRole::Background
                        });
                        let foreground = theme::readable_text(
                            background,
                            theme::color(theme::PaletteRole::Text),
                        );
                        let secondary = theme::readable_text(
                            background,
                            theme::color(theme::PaletteRole::Muted),
                        );
                        let (level, color) = level_badge(line);
                        let badge = theme::element_colors(color);
                        let category: String = line
                            .event
                            .as_ref()
                            .map_or("—", |event| event.category.as_str())
                            .chars()
                            .take(24)
                            .collect();
                        Some(
                            div()
                                .id(("log-line", index))
                                .debug_selector(move || format!("log-row-{index}"))
                                .h(px(22.0))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(8.0))
                                .overflow_hidden()
                                .bg(rgb(background))
                                .text_color(rgb(foreground))
                                .child(
                                    div()
                                        .w(px(92.0))
                                        .flex_shrink_0()
                                        .text_color(rgb(secondary))
                                        .child(theme::text(time)),
                                )
                                .child(
                                    div()
                                        .debug_selector(move || format!("log-level-{index}"))
                                        .w(px(60.0))
                                        .h(px(18.0))
                                        .flex_shrink_0()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .rounded(px(2.0))
                                        .bg(rgb(badge.accent))
                                        .text_color(rgb(badge.accent_text))
                                        .child(theme::text(level)),
                                )
                                .child(
                                    div()
                                        .debug_selector(move || format!("log-category-{index}"))
                                        .w(px(80.0))
                                        .flex_shrink_0()
                                        .truncate()
                                        .text_color(rgb(secondary))
                                        .child(theme::text(hex_session::service::text::verbatim(
                                            category,
                                        ))),
                                )
                                .child(
                                    div()
                                        .debug_selector(move || format!("log-message-{index}"))
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .child(display),
                                )
                                .on_mouse_down(
                                    gpui::MouseButton::Left,
                                    cx.listener(
                                        move |view: &mut Viewer,
                                              event: &gpui::MouseDownEvent,
                                              _,
                                              cx| {
                                            if !event.modifiers.shift || view.anchor.is_none() {
                                                view.anchor = Some(index);
                                            }
                                            view.selection = Some(index);
                                            cx.notify();
                                        },
                                    ),
                                ),
                        )
                    })
                    .collect::<Vec<_>>()
            }),
        )
        .h_full()
        .min_h_0()
        .w_full()
        .track_scroll(self.scroll.clone());
        let session = self
            .path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let status = self.status.clone();
        let detail = self
            .selection
            .and_then(|selection| self.rows.get(selection))
            .and_then(|index| self.snapshot.as_ref()?.lines.get(*index))
            .map(|line| {
                let mut text: String = line.text.chars().take(16_384).collect();
                if text.len() < line.text.len() {
                    text.push_str("\n[display clipped]");
                }
                gpui::SharedString::from(text)
            })
            .unwrap_or_else(|| theme::text("select a line to inspect it"));
        div()
            .size_full()
            .flex()
            .flex_col()
            .font_family(theme::FONT_FAMILY)
            .text_size(px(11.0))
            .text_color(rgb(theme::color(theme::PaletteRole::Text)))
            .bg(rgb(theme::color(theme::PaletteRole::Background)))
            .child(
                div()
                    .h(px(34.0))
                    .pl(px(theme::TITLEBAR_LEADING_INSET))
                    .pr(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(theme::text("diagnostic log · utc"))
                    .child(
                        control("previous session")
                            .on_click(cx.listener(|view, _, _, cx| view.shift_session(-1, cx))),
                    )
                    .child(
                        control("next session")
                            .on_click(cx.listener(|view, _, _, cx| view.shift_session(1, cx))),
                    )
                    .child(div().flex_1().overflow_hidden().child(session))
                    .child(
                        control("refresh").on_click(cx.listener(|view, _, _, cx| view.reload(cx))),
                    ),
            )
            .child(
                div()
                    .h(px(34.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        control(&format!("category: {}", CATEGORIES[self.category])).on_click(
                            cx.listener(|view, _, _, cx| {
                                view.category = (view.category + 1) % CATEGORIES.len();
                                view.filter(cx);
                            }),
                        ),
                    )
                    .child(control(WINDOWS[self.window].0).on_click(cx.listener(
                        |view, _, _, cx| {
                            view.window = (view.window + 1) % WINDOWS.len();
                            view.filter(cx);
                        },
                    )))
                    .child(div().w(px(250.0)).child(self.query.clone()))
                    .child(
                        control("copy selection")
                            .on_click(cx.listener(|view, _, _, cx| view.copy_selection(cx))),
                    )
                    .child(
                        control("copy all")
                            .debug_selector(|| "log-copy-all".into())
                            .on_click(cx.listener(|view, _, _, cx| view.copy_all(cx))),
                    )
                    .child(
                        control("bottom")
                            .debug_selector(|| "log-bottom".into())
                            .on_click(cx.listener(|view, _, _, cx| {
                                if !view.rows.is_empty() {
                                    view.scroll.scroll_to_item(
                                        view.rows.len() - 1,
                                        gpui::ScrollStrategy::Bottom,
                                    );
                                    cx.notify();
                                }
                            })),
                    ),
            )
            .child(
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(
                        control("older lines")
                            .on_click(cx.listener(|view, _, _, cx| view.shift_page(true, cx))),
                    )
                    .child(
                        control("newer lines")
                            .on_click(cx.listener(|view, _, _, cx| view.shift_page(false, cx))),
                    )
                    .child(theme::text(format!(
                        "page {} · filters apply to this page",
                        self.page.saturating_add(1)
                    ))),
            )
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_1().min_w_0().h_full().child(list))
                    .child(self.log_scrollbar(cx)),
            )
            .child(
                div()
                    .id("log-detail")
                    .h(px(96.0))
                    .px(px(8.0))
                    .overflow_y_scroll()
                    .border_t_1()
                    .border_color(rgb(theme::color(theme::PaletteRole::Border)))
                    .child(detail),
            )
            .child(
                div()
                    .h(px(28.0))
                    .px(px(8.0))
                    .overflow_hidden()
                    .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                    .child(status),
            )
    }
}

fn control(label: &str) -> gpui::Stateful<gpui::Div> {
    hex_widgets::action(theme::text(label), theme::action_style())
        .id(gpui::SharedString::from(label.to_owned()))
        .h(px(22.0))
        .px(px(7.0))
}

fn format_time(unix_nanos: u128) -> String {
    let millis = unix_nanos / 1_000_000;
    let day_millis = millis % 86_400_000;
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        day_millis / 3_600_000,
        day_millis / 60_000 % 60,
        day_millis / 1000 % 60,
        day_millis % 1000
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use hex_diagnostics::reader::LogLine;

    #[test]
    fn excerpt_copies_selected_visible_range_with_context_and_warning() {
        let snapshot = LogSnapshot {
            lines: vec![
                LogLine {
                    text: "session_started hex_version=1".into(),
                    event: None,
                },
                LogLine {
                    text: "first".into(),
                    event: None,
                },
                LogLine {
                    text: "hidden".into(),
                    event: None,
                },
                LogLine {
                    text: "last".into(),
                    event: None,
                },
            ],
            warnings: vec!["records were lost".into()],
            header: Some("session_started hex_version=1".into()),
        };
        let result = make_excerpt(
            &snapshot,
            &[1, 3],
            Path::new("/tmp/session.log"),
            "audio",
            "last 1 min",
            "error",
            (1, 0),
        );
        assert!(result.contains("session_started hex_version=1"));
        assert!(result.contains("warning: records were lost"));
        assert!(result.contains("first\nlast\n"));
        assert!(!result.contains("hidden"));
    }

    #[test]
    fn excerpt_clips_oversized_lines_and_reports_the_limit() {
        let snapshot = LogSnapshot {
            lines: vec![LogLine {
                text: "x".repeat(COPY_LIMIT + 1),
                event: None,
            }],
            warnings: Vec::new(),
            header: None,
        };
        let result = make_excerpt(
            &snapshot,
            &[0],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            (0, 0),
        );
        assert!(result.contains("excerpt clipped"));
        assert!(result.len() < COPY_LIMIT);
    }

    #[test]
    fn oversized_multibyte_warning_still_obeys_clipboard_limit() {
        let snapshot = LogSnapshot {
            lines: Vec::new(),
            warnings: vec!["é".repeat(COPY_LIMIT)],
            header: None,
        };
        let result = make_excerpt(
            &snapshot,
            &[],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            (0, 0),
        );
        assert!(result.len() <= COPY_LIMIT);
        assert!(result.ends_with(COPY_CLIPPED));
    }

    #[test]
    fn timestamp_is_utc_time_of_day() {
        assert_eq!(format_time(3_723_004_000_000), "01:02:03.004");
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn long_log_rows_stay_within_their_visual_bounds(cx: &mut gpui::TestAppContext) {
        let dir =
            std::env::temp_dir().join(format!("hex-viewer-row-geometry-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-300-1.log");
        let long = format!(
            "event level=info category=ui unix_nanos=1 elapsed_nanos=1 sequence=1 truncated=false message=\"{}\"",
            "long diagnostic message ".repeat(80)
        );
        std::fs::write(&path, format!("session_started log_schema=3 segment=0\n{long}\nevent level=info category=ui unix_nanos=2 elapsed_nanos=2 sequence=2 truncated=false message=\"next\"\n")).unwrap();
        let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
        cx.run_until_parked();
        let viewer = handle.root(cx).unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(*handle, cx);
        for width in [850.0, 1200.0] {
            visual.simulate_resize(gpui::size(px(width), px(600.0)));
            visual.run_until_parked();
            let row = visual
                .debug_bounds("log-row-1")
                .expect("long row is visible");
            let next = visual
                .debug_bounds("log-row-2")
                .expect("next row is visible");
            let message = visual
                .debug_bounds("log-message-1")
                .expect("message is visible");
            assert!(
                message.top() >= row.top() && message.bottom() <= row.bottom(),
                "long message {message:?} must fit in row {row:?} at width {width}"
            );
            assert!(row.bottom() <= next.top(), "adjacent rows must not overlap");
        }
        let long_row = visual.debug_bounds("log-row-1").unwrap();
        visual.simulate_click(long_row.center(), gpui::Modifiers::default());
        visual.run_until_parked();
        viewer.read_with(visual, |view, _| {
            assert_eq!(view.selection, Some(1));
            let snapshot = view.snapshot.as_ref().unwrap();
            assert_eq!(snapshot.lines[1].text, long);
            let excerpt = make_excerpt(
                snapshot,
                &view.rows,
                &path,
                "all",
                "entire session",
                "",
                (1, 1),
            );
            assert!(
                excerpt.contains(&long),
                "copy preserves the complete record"
            );
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn sessions_are_ordered_and_archives_are_not_listed_as_sessions() {
        let dir = std::env::temp_dir().join(format!("hex-viewer-sessions-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let older = dir.join("session-100-7.log");
        let active = dir.join("session-200-8.log");
        std::fs::write(&older, "").unwrap();
        std::fs::write(&active, "").unwrap();
        std::fs::write(dir.join("session-100-7.1.log"), "").unwrap();
        assert_eq!(list_sessions(&active).unwrap(), vec![older, active]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn rapid_filter_changes_install_only_the_latest_result(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("hex-viewer-filter-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-100-1.log");
        std::fs::write(&path, concat!(
            "session_started log_schema=3 segment=0\n",
            "event level=info category=audio unix_nanos=1 elapsed_nanos=1 sequence=1 truncated=false message=\"audio\"\n",
            "event level=info category=ui unix_nanos=2 elapsed_nanos=2 sequence=2 truncated=false message=\"ui\"\n",
        )).unwrap();
        let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
        cx.run_until_parked();
        let viewer = handle.root(cx).unwrap();
        viewer.update(cx, |view, cx| {
            view.category = CATEGORIES
                .iter()
                .position(|category| *category == "audio")
                .unwrap();
            view.filter(cx);
            view.category = CATEGORIES
                .iter()
                .position(|category| *category == "ui")
                .unwrap();
            view.filter(cx);
        });
        cx.run_until_parked();
        viewer.read_with(cx, |view, _| {
            let snapshot = view.snapshot.as_ref().unwrap();
            let selected: Vec<_> = view
                .rows
                .iter()
                .filter_map(|index| snapshot.lines[*index].event.as_ref())
                .map(|event| event.category.as_str())
                .collect();
            assert_eq!(selected, ["ui"]);
            assert!(!view.filtering);
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "ui-test-support")]
    pub(super) fn paged_fixture(name: &str) -> (PathBuf, PathBuf) {
        let dir = std::env::temp_dir().join(format!("hex-viewer-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-200-1.log");
        let mut content =
            "session_started log_schema=3 segment=0 build=fixture\noldest\n".to_owned();
        content.push_str(&"newer\n".repeat(100_000));
        std::fs::write(&path, content).unwrap();
        (dir, path)
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn viewer_navigates_older_and_newer_pages(cx: &mut gpui::TestAppContext) {
        let (dir, path) = paged_fixture("pages");
        let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
        cx.run_until_parked();
        let viewer = handle.root(cx).unwrap();
        viewer.update(cx, |view, cx| {
            assert!(view.has_older);
            view.shift_page(true, cx);
        });
        cx.run_until_parked();
        viewer.update(cx, |view, cx| {
            assert_eq!(view.page, 1);
            assert!(
                view.snapshot
                    .as_ref()
                    .unwrap()
                    .lines
                    .iter()
                    .any(|line| line.text == "oldest")
            );
            assert!(view.has_newer);
            view.shift_page(false, cx);
        });
        cx.run_until_parked();
        viewer.read_with(cx, |view, _| {
            assert_eq!(view.page, 0);
            assert!(view.has_older);
        });
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(feature = "ui-test-support")]
    #[gpui::test]
    fn severity_badges_keep_rows_readable_and_copy_original_records(cx: &mut gpui::TestAppContext) {
        let dir = std::env::temp_dir().join(format!("hex-viewer-levels-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-400-1.log");
        let mut content = "session_started log_schema=3 segment=0 build=fixture\n".to_owned();
        for (index, level) in ["error", "warn", "info", "debug", "trace", "future"]
            .iter()
            .enumerate()
        {
            let sequence = index + 1;
            content.push_str(&format!("event level={level} category=audio unix_nanos={sequence} elapsed_nanos={sequence} sequence={sequence} truncated=false message=\"original {level} Message\"\n"));
        }
        content.push_str("older untyped context\ndiagnostics_loss queue_full=1\n");
        std::fs::write(&path, &content).unwrap();
        let handle = cx.add_window(|_, cx| Viewer::new(path.clone(), cx));
        cx.run_until_parked();
        let viewer = handle.root(cx).unwrap();
        let visual = &mut gpui::VisualTestContext::from_window(*handle, cx);
        for width in [850.0, 1200.0] {
            visual.simulate_resize(gpui::size(px(width), px(600.0)));
            visual.run_until_parked();
            for index in 0..9 {
                let row = visual
                    .debug_bounds(Box::leak(format!("log-row-{index}").into_boxed_str()))
                    .unwrap();
                let badge = visual
                    .debug_bounds(Box::leak(format!("log-level-{index}").into_boxed_str()))
                    .expect("every record has an explicit level or context label");
                let category = visual
                    .debug_bounds(Box::leak(format!("log-category-{index}").into_boxed_str()))
                    .unwrap();
                let message = visual
                    .debug_bounds(Box::leak(format!("log-message-{index}").into_boxed_str()))
                    .unwrap();
                assert!(badge.left() >= row.left() && badge.bottom() <= row.bottom());
                assert!(badge.right() <= category.left() && category.right() <= message.left());
                assert!(message.right() <= row.right());
                visual.simulate_click(row.center(), gpui::Modifiers::default());
                visual.run_until_parked();
                viewer.update(visual, |view, cx| view.copy_selection(cx));
                visual.run_until_parked();
                viewer.read_with(visual, |view, cx| {
                    assert_eq!(view.selection, Some(index));
                    let original = &view.snapshot.as_ref().unwrap().lines[index].text;
                    assert_eq!(original, content.lines().nth(index).unwrap());
                    let copied = cx.read_from_clipboard().unwrap().text().unwrap();
                    assert!(
                        copied.ends_with(&format!("{original}\n")),
                        "copy keeps original case and structured fields"
                    );
                });
            }
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
#[path = "log_viewer/copy_tests.rs"]
mod copy_tests;
