//! Native, control-side view of bounded diagnostic session snapshots.
use crate::theme;
use gpui::{
    App, AppContext, Bounds, ClipboardItem, Context, Entity, IntoElement, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, UniformListScrollHandle, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, rgb, size, uniform_list,
};
use hex_diagnostics::reader::{LogFilter, LogSnapshot, read_session};
use hex_widgets::{TextInput, TextInputChanged, TextInputConfig, TextInputKind};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
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
        };
        view.reload(cx);
        view
    }
    fn reload(&mut self, cx: &mut Context<Self>) {
        self.generation = self.generation.wrapping_add(1);
        let generation = self.generation;
        let path = self.path.clone();
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
            let snapshot = read_session(&path).map_err(|e| e.to_string());
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
                    Ok(mut snapshot) => {
                        match sessions {
                            Ok(sessions) => view.sessions = sessions,
                            Err(error) => {
                                view.sessions = vec![view.path.clone()];
                                snapshot
                                    .warnings
                                    .push(format!("could not list sessions: {error}"));
                            }
                        }
                        view.snapshot = Some(Arc::new(snapshot));
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
        self.filtering = true;
        let category = CATEGORIES[self.category].to_owned();
        let seconds = WINDOWS[self.window].1;
        let query = self.query.read(cx).text().to_owned();
        let task = cx.background_executor().spawn(async move {
            let latest = snapshot
                .lines
                .iter()
                .filter_map(|line| line.event.as_ref().map(|e| e.unix_nanos))
                .max();
            let since = seconds.zip(latest).map(|(seconds, latest)| {
                latest.saturating_sub(Duration::from_secs(seconds).as_nanos())
            });
            let filter = LogFilter {
                category: (category != "all").then_some(category.as_str()),
                text: (!query.is_empty()).then_some(query.as_str()),
                since_unix_nanos: since,
                until_unix_nanos: None,
            };
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
                    format!("{} lines · click then shift-click to select", rows.len())
                } else {
                    format!("{} lines · {}", rows.len(), snapshot.warnings.join(" · "))
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
        self.reload(cx);
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
        let time = WINDOWS[self.window].0.to_owned();
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
                anchor..=end,
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

fn make_excerpt(
    snapshot: &LogSnapshot,
    rows: &[usize],
    path: &Path,
    category: &str,
    time: &str,
    query: &str,
    selection: std::ops::RangeInclusive<usize>,
) -> String {
    let mut result = format!(
        "hex log excerpt\nsession: {}\ncategory: {category}; time: {time}; search: {query}\n",
        path.display()
    );
    if let Some(header) = snapshot
        .lines
        .iter()
        .find(|line| line.text.starts_with("session_started "))
    {
        let context: String = header.text.chars().take(4096).collect();
        result.push_str(&context);
        if context.len() < header.text.len() {
            result.push_str(" [header clipped]");
        }
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
    let start = *selection.start();
    let end = *selection.end();
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
                        Some(
                            div()
                                .id(("log-line", index))
                                .h(px(22.0))
                                .w_full()
                                .flex()
                                .items_center()
                                .gap(px(8.0))
                                .px(px(8.0))
                                .overflow_hidden()
                                .bg(rgb(theme::color(if selected {
                                    theme::PaletteRole::Hover
                                } else {
                                    theme::PaletteRole::Background
                                })))
                                .child(
                                    div()
                                        .w(px(92.0))
                                        .flex_shrink_0()
                                        .text_color(rgb(theme::color(theme::PaletteRole::Muted)))
                                        .child(theme::text(time)),
                                )
                                .child(div().overflow_hidden().child(display))
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
                    .child(control("latest").on_click(cx.listener(|view, _, _, _cx| {
                        if !view.rows.is_empty() {
                            view.scroll
                                .scroll_to_item(view.rows.len() - 1, gpui::ScrollStrategy::Bottom);
                        }
                    }))),
            )
            .child(div().flex_1().min_h_0().child(list))
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
        };
        let result = make_excerpt(
            &snapshot,
            &[1, 3],
            Path::new("/tmp/session.log"),
            "audio",
            "last 1 min",
            "error",
            1..=0,
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
        };
        let result = make_excerpt(
            &snapshot,
            &[0],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            0..=0,
        );
        assert!(result.contains("excerpt clipped"));
        assert!(result.len() < COPY_LIMIT);
    }

    #[test]
    fn oversized_multibyte_warning_still_obeys_clipboard_limit() {
        let snapshot = LogSnapshot {
            lines: Vec::new(),
            warnings: vec!["é".repeat(COPY_LIMIT)],
        };
        let result = make_excerpt(
            &snapshot,
            &[],
            Path::new("session.log"),
            "all",
            "entire session",
            "",
            0..=0,
        );
        assert!(result.len() <= COPY_LIMIT);
        assert!(result.ends_with(COPY_CLIPPED));
    }

    #[test]
    fn timestamp_is_utc_time_of_day() {
        assert_eq!(format_time(3_723_004_000_000), "01:02:03.004");
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
}
