//! Pre-open missing-media panel: manual locate with relative-folder resolve,
//! bounded, cancellable folder search, and candidates from the browser's sample
//! catalog offered as one "relink all found" action (never applied unasked).
//!
//! `hex_session::service::missing_media` owns reporting, matching and search;
//! this view owns only dialog state. The project opens through the existing
//! `OpenWith`/`OpenWithAudioMode` relink admission once every file is resolved,
//! or on request with the remaining files offline (their clips play silence),
//! so relinks stay in memory until the user saves. File system work runs on the
//! background executor; the panel only reads media and never changes it.
use super::EditorView;
use crate::theme::{self, PaletteRole};
use gpui::{Context, Div, PathPromptOptions, Stateful, div, prelude::*, px, rgb};
use hex_library::catalog::MissingSample;
use hex_session::{
    DocumentAudioMode, SourcePlanReservation,
    service::{
        ProjectSource, Request,
        missing_media::{self, CandidateFit, MissingMedia, SearchOutcome, SearchStop},
    },
};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

/// How one missing file is currently resolved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) enum Resolution {
    Missing,
    /// Matching (or header-unverified) file chosen for the open path to verify.
    Found(PathBuf, CandidateFit),
    /// Several files match; the user picks one.
    Choices(Vec<PathBuf>),
    /// The nearest candidate the open path would refuse, with the reason.
    Mismatch(PathBuf, String),
}

pub(super) struct Row {
    pub(super) media: MissingMedia,
    pub(super) resolution: Resolution,
}

struct Search {
    cancel: Arc<AtomicBool>,
    progress: Arc<AtomicUsize>,
}

/// What the panel resolves files for.
enum Target {
    /// Before opening a saved project.
    Open {
        source: ProjectSource,
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
    },
    /// Offline media of the open project, relinked in place as one edit.
    Relink,
}

/// Candidates the sample catalog proposed, checked by the session matcher.
pub(super) struct CatalogOffer {
    pub(super) outcome: SearchOutcome,
    /// The catalog was still scanning, so the offer may be incomplete.
    pub(super) partial: bool,
}

impl CatalogOffer {
    /// Unresolved rows the offer would resolve outright, and rows with choices.
    pub(super) fn counts(&self, rows: &[Row]) -> (usize, usize) {
        let mut unique = 0;
        let mut several = 0;
        for row in rows
            .iter()
            .filter(|row| !matches!(row.resolution, Resolution::Found(..)))
        {
            if self.outcome.unique_match(row.media.asset).is_some() {
                unique += 1;
            } else if self
                .outcome
                .candidates
                .get(&row.media.asset)
                .is_some_and(|found| {
                    found
                        .iter()
                        .filter(|(_, fit)| *fit == CandidateFit::Matches)
                        .count()
                        > 1
                })
                && !matches!(row.resolution, Resolution::Choices(_))
            {
                several += 1;
            }
        }
        (unique, several)
    }
}

pub(super) struct Panel {
    target: Target,
    generation: u64,
    revision: u64,
    pub(super) rows: Vec<Row>,
    search: Option<Search>,
    pub(super) catalog: Option<CatalogOffer>,
    catalog_cancel: Arc<AtomicBool>,
    note: Option<String>,
}

impl Panel {
    fn resolved(&self) -> BTreeMap<hex_project::AssetId, PathBuf> {
        self.rows
            .iter()
            .filter_map(|row| match &row.resolution {
                Resolution::Found(path, _) => Some((row.media.asset, path.clone())),
                _ => None,
            })
            .collect()
    }

    fn complete(&self) -> bool {
        self.rows
            .iter()
            .all(|row| matches!(row.resolution, Resolution::Found(..)))
    }
}

/// Applies a manual pick plus its relative-folder mappings; returns the count
/// of other files resolved through the same folder mapping.
pub(super) fn apply_locate(
    rows: &mut [Row],
    index: usize,
    path: PathBuf,
    fit: CandidateFit,
    mapped: Vec<(hex_project::AssetId, PathBuf)>,
) -> usize {
    if let Some(row) = rows.get_mut(index) {
        row.resolution = match fit {
            CandidateFit::Mismatch(reason) | CandidateFit::Unusable(reason) => {
                Resolution::Mismatch(path, reason)
            }
            fit => Resolution::Found(path, fit),
        };
    }
    let mut count = 0;
    for (asset, path) in mapped {
        if let Some(row) = rows.iter_mut().find(|row| {
            row.media.asset == asset && !matches!(row.resolution, Resolution::Found(..))
        }) {
            row.resolution = Resolution::Found(path, CandidateFit::Matches);
            count += 1;
        }
    }
    count
}

/// Applies a folder search to unresolved rows; returns how many were found.
pub(super) fn apply_search(rows: &mut [Row], outcome: &SearchOutcome) -> usize {
    let mut found = 0;
    for row in rows
        .iter_mut()
        .filter(|row| !matches!(row.resolution, Resolution::Found(..)))
    {
        if let Some(path) = outcome.unique_match(row.media.asset) {
            row.resolution = Resolution::Found(path.to_path_buf(), CandidateFit::Matches);
            found += 1;
            continue;
        }
        let Some(candidates) = outcome.candidates.get(&row.media.asset) else {
            continue;
        };
        let matching: Vec<_> = candidates
            .iter()
            .filter(|(_, fit)| *fit == CandidateFit::Matches)
            .map(|(path, _)| path.clone())
            .collect();
        if matching.len() > 1 {
            row.resolution = Resolution::Choices(matching);
        } else if let Some((path, CandidateFit::Mismatch(reason))) = candidates
            .iter()
            .find(|(_, fit)| matches!(fit, CandidateFit::Mismatch(_)))
            && row.resolution == Resolution::Missing
        {
            row.resolution = Resolution::Mismatch(path.clone(), reason.clone());
        }
    }
    found
}

fn search_summary(outcome: &SearchOutcome, found: usize) -> String {
    let mut text = format!("searched {} entries · found {found}", outcome.scanned);
    match outcome.stopped {
        Some(SearchStop::Cancelled) => text.push_str(" · cancelled"),
        Some(SearchStop::EntryLimit) => text.push_str(" · stopped at entry limit"),
        None => {}
    }
    if outcome.unreadable > 0 {
        text.push_str(&format!(" · {} unreadable skipped", outcome.unreadable));
    }
    if outcome.depth_skipped > 0 {
        text.push_str(&format!(" · {} too deep skipped", outcome.depth_skipped));
    }
    text
}

impl EditorView {
    /// Checks for missing media off the UI thread, then opens directly or shows
    /// the panel. `opening_project` stays set while the panel is open so edits
    /// and other project operations wait for it.
    pub(super) fn open_checking_missing_media(
        &mut self,
        source: ProjectSource,
        mode: DocumentAudioMode,
        reservation: Option<SourcePlanReservation>,
        direct: Request,
        cx: &mut Context<Self>,
    ) {
        self.opening_project = true;
        let inspect = source.clone();
        let report = cx
            .background_executor()
            .spawn(async move { missing_media::missing_media_report(&inspect) });
        cx.spawn(async move |this, cx| {
            let report = report.await;
            let _ = this.update(cx, |view, cx| {
                match report {
                    Ok(missing) if missing.is_empty() => {
                        view.opening_project = false;
                        view.begin_open_request(direct);
                    }
                    Ok(missing) => {
                        view.missing_media = Some(Panel {
                            target: Target::Open {
                                source,
                                mode,
                                reservation,
                            },
                            generation: view.project_generation,
                            revision: view.state.as_ref().map_or(0, |state| state.revision),
                            rows: missing
                                .into_iter()
                                .map(|media| Row {
                                    media,
                                    resolution: Resolution::Missing,
                                })
                                .collect(),
                            search: None,
                            catalog: None,
                            catalog_cancel: Arc::new(AtomicBool::new(false)),
                            note: None,
                        });
                        view.status = "locate missing files to open the project".into();
                        view.missing_media_catalog_lookup(cx);
                    }
                    Err(reason) => {
                        view.opening_project = false;
                        view.error = Some(reason.to_lowercase().into());
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Ask the browser's sample catalog for candidates on a background worker and
    /// check them with the session matcher. The result is only offered.
    fn missing_media_catalog_lookup(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.missing_media.as_ref() else {
            return;
        };
        let missing: Vec<_> = panel
            .rows
            .iter()
            .filter(|row| !matches!(row.resolution, Resolution::Found(..)))
            .map(|row| row.media.clone())
            .collect();
        let cancel = Arc::clone(&panel.catalog_cancel);
        let (catalog, partial) = self.sample_catalog_snapshot();
        let lookup = cx.background_executor().spawn(async move {
            let lists = missing
                .iter()
                .filter_map(|media| {
                    let query = MissingSample::named(&media.expected_path)?;
                    let paths: Vec<_> = catalog
                        .find_candidates(&query)
                        .into_iter()
                        .map(|found| found.path)
                        .collect();
                    (!paths.is_empty()).then_some((media.asset, paths))
                })
                .collect();
            missing_media::check_candidate_lists(
                &missing,
                &lists,
                missing_media::SearchLimits::default().max_candidates,
                &cancel,
            )
        });
        cx.spawn(async move |this, cx| {
            let outcome = lookup.await;
            let _ = this.update(cx, |view, cx| {
                if let Some(panel) = view.missing_media.as_mut()
                    && outcome.stopped.is_none()
                {
                    panel.catalog = Some(CatalogOffer { outcome, partial });
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// Apply the catalog offer: unique matches resolve, several matches become
    /// choices. Rows already resolved are kept.
    pub(super) fn missing_media_relink_catalog(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.missing_media.as_mut() else {
            return;
        };
        let Some(offer) = panel.catalog.as_ref() else {
            return;
        };
        let (_, several) = offer.counts(&panel.rows);
        let found = apply_search(&mut panel.rows, &offer.outcome);
        let mut note = format!("relinked {found} from the sample catalog");
        if several > 0 {
            note.push_str(&format!(" · {several} with several matches"));
        }
        panel.note = Some(note);
        cx.notify();
    }

    fn missing_media_locate(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(row) = self.missing_media.as_ref().and_then(|p| p.rows.get(index)) else {
            return;
        };
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some(theme::text(format!("locate {}", row.media.file_name()))),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(mut paths))) = prompt.await else {
                return;
            };
            let Some(path) = paths.pop() else {
                return;
            };
            let Ok(Some((media, all, resolved))) = this.update(cx, |view, _| {
                let panel = view.missing_media.as_ref()?;
                let media = panel.rows.get(index)?.media.clone();
                let all: Vec<_> = panel.rows.iter().map(|row| row.media.clone()).collect();
                Some((media, all, panel.resolved()))
            }) else {
                return;
            };
            let (path, fit, mapped) = cx
                .background_executor()
                .spawn(async move {
                    let fit = missing_media::check_candidate(&media, &path);
                    let mapped = if fit.usable() {
                        missing_media::resolve_relative(&all, &resolved, &media, &path)
                    } else {
                        Vec::new()
                    };
                    (path, fit, mapped)
                })
                .await;
            let _ = this.update(cx, |view, cx| {
                let Some(panel) = view.missing_media.as_mut() else {
                    return;
                };
                let mapped = apply_locate(&mut panel.rows, index, path, fit, mapped);
                panel.note =
                    (mapped > 0).then(|| format!("also found {mapped} from the same folder"));
                cx.notify();
            });
        })
        .detach();
    }

    fn missing_media_search(&mut self, cx: &mut Context<Self>) {
        if self
            .missing_media
            .as_ref()
            .is_none_or(|panel| panel.search.is_some())
        {
            return;
        }
        let prompt = cx.prompt_for_paths(PathPromptOptions {
            files: false,
            directories: true,
            multiple: true,
            prompt: Some(theme::text("search folders")),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(roots))) = prompt.await else {
                return;
            };
            let cancel = Arc::new(AtomicBool::new(false));
            let progress = Arc::new(AtomicUsize::new(0));
            let Ok(Some(missing)) = this.update(cx, |view, cx| {
                let panel = view.missing_media.as_mut()?;
                if panel.search.is_some() {
                    return None;
                }
                panel.search = Some(Search {
                    cancel: Arc::clone(&cancel),
                    progress: Arc::clone(&progress),
                });
                panel.note = None;
                cx.notify();
                Some(
                    panel
                        .rows
                        .iter()
                        .filter(|row| !matches!(row.resolution, Resolution::Found(..)))
                        .map(|row| row.media.clone())
                        .collect::<Vec<_>>(),
                )
            }) else {
                return;
            };
            let worker_cancel = Arc::clone(&cancel);
            let worker_progress = Arc::clone(&progress);
            let search = cx.background_executor().spawn(async move {
                missing_media::search_folders(
                    &missing,
                    &roots,
                    missing_media::SearchLimits::default(),
                    &worker_cancel,
                    &worker_progress,
                )
            });
            // Repaint progress until the search finishes.
            let timer = cx.background_executor().clone();
            let ticker = this.clone();
            let mut ticker_cx = cx.clone();
            let done = Arc::new(AtomicBool::new(false));
            let ticker_done = Arc::clone(&done);
            cx.foreground_executor()
                .spawn(async move {
                    while !ticker_done.load(Ordering::Relaxed) {
                        timer.timer(Duration::from_millis(100)).await;
                        if ticker.update(&mut ticker_cx, |_, cx| cx.notify()).is_err() {
                            break;
                        }
                    }
                })
                .detach();
            let outcome = search.await;
            done.store(true, Ordering::Relaxed);
            let _ = this.update(cx, |view, cx| {
                let Some(panel) = view.missing_media.as_mut() else {
                    return;
                };
                panel.search = None;
                let found = apply_search(&mut panel.rows, &outcome);
                panel.note = Some(search_summary(&outcome, found));
                cx.notify();
            });
        })
        .detach();
    }

    fn missing_media_choose(&mut self, index: usize, choice: usize, cx: &mut Context<Self>) {
        if let Some(row) = self
            .missing_media
            .as_mut()
            .and_then(|panel| panel.rows.get_mut(index))
            && let Resolution::Choices(paths) = &row.resolution
            && let Some(path) = paths.get(choice)
        {
            row.resolution = Resolution::Found(path.clone(), CandidateFit::Matches);
            cx.notify();
        }
    }

    pub(super) fn missing_media_dismiss(&mut self, cx: &mut Context<Self>) {
        if let Some(panel) = self.missing_media.take() {
            panel.catalog_cancel.store(true, Ordering::Relaxed);
            if let Some(search) = panel.search {
                search.cancel.store(true, Ordering::Relaxed);
            }
            if matches!(panel.target, Target::Open { .. }) {
                self.opening_project = false;
                self.status = "open cancelled".into();
            }
            cx.notify();
        }
    }

    /// Shows the panel for the open project's offline media (from the status
    /// bar indicator). The browser's sample catalog is checked in the background
    /// and only offered; nothing else is read until the user locates or searches.
    pub(super) fn missing_media_show_offline(&mut self, cx: &mut Context<Self>) {
        if self.missing_media.is_some() || self.opening_project {
            return;
        }
        let Some(state) = self.state.as_ref() else {
            return;
        };
        let base = state
            .document_path
            .as_deref()
            .and_then(std::path::Path::parent);
        let rows: Vec<_> = missing_media::offline_media_report(&state.project, base)
            .into_iter()
            .map(|media| Row {
                media,
                resolution: Resolution::Missing,
            })
            .collect();
        if rows.is_empty() {
            return;
        }
        self.missing_media = Some(Panel {
            target: Target::Relink,
            generation: self.project_generation,
            revision: state.revision,
            rows,
            search: None,
            catalog: None,
            catalog_cancel: Arc::new(AtomicBool::new(false)),
            note: None,
        });
        self.missing_media_catalog_lookup(cx);
        cx.notify();
    }

    /// Relinks the files found so far as one undoable edit; the rest stay offline.
    pub(super) fn missing_media_relink(&mut self, cx: &mut Context<Self>) {
        let Some(panel) = self.missing_media.as_ref() else {
            return;
        };
        if !matches!(panel.target, Target::Relink)
            || panel.search.is_some()
            || panel.resolved().is_empty()
        {
            return;
        }
        let Some(panel) = self.missing_media.take() else {
            return;
        };
        if panel.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != panel.revision)
        {
            self.error = Some("project changed · relink cancelled".into());
        } else if !self.enqueue(Request::RelinkMedia(panel.resolved())) {
            self.missing_media = Some(panel);
        }
        cx.notify();
    }

    fn missing_media_open(&mut self, cx: &mut Context<Self>) {
        self.missing_media_open_with(false, cx);
    }

    /// Opens with the files found so far; with `offline`, files still missing
    /// open offline (their clips are silent) instead of blocking the open.
    pub(super) fn missing_media_open_with(&mut self, offline: bool, cx: &mut Context<Self>) {
        let Some(panel) = self.missing_media.as_ref() else {
            return;
        };
        if (!offline && !panel.complete())
            || panel.search.is_some()
            || !matches!(panel.target, Target::Open { .. })
        {
            return;
        }
        let Some(panel) = self.missing_media.take() else {
            return;
        };
        self.opening_project = false;
        if panel.generation != self.project_generation
            || self
                .state
                .as_ref()
                .is_none_or(|state| state.revision != panel.revision)
        {
            self.error = Some("project changed · open cancelled".into());
        } else {
            let relinks = panel.resolved();
            if let Target::Open {
                source,
                mode,
                reservation,
            } = panel.target
            {
                self.begin_open_request(open_request(source, relinks, mode, reservation));
            }
        }
        cx.notify();
    }

    pub(super) fn missing_media_overlay(&self, cx: &mut Context<Self>) -> Option<Stateful<Div>> {
        let panel = self.missing_media.as_ref()?;
        let muted = rgb(theme::color(PaletteRole::Muted));
        let mut list = div()
            .id("missing-media-list")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .flex()
            .flex_col();
        for (index, row) in panel.rows.iter().enumerate() {
            let media = &row.media;
            let mut uses = media
                .usages
                .iter()
                .map(|usage| usage.describe())
                .collect::<Vec<_>>()
                .join(", ");
            if media.usage_count > media.usages.len() {
                uses.push_str(&format!(" +{}", media.usage_count - media.usages.len()));
            }
            if media.usage_count == 0 {
                uses = "unused".into();
            }
            if media.device_original {
                uses.push_str(" · device original");
            }
            let (status, detail) = match &row.resolution {
                Resolution::Missing => ("missing", None),
                Resolution::Found(path, CandidateFit::Unverified(_)) => (
                    "found · unverified",
                    Some(hex_session::service::text::verbatim(path.display())),
                ),
                Resolution::Found(path, _) => (
                    "found",
                    Some(hex_session::service::text::verbatim(path.display())),
                ),
                Resolution::Choices(paths) => {
                    ("several matches", Some(format!("{} files", paths.len())))
                }
                Resolution::Mismatch(path, reason) => (
                    "mismatch",
                    Some(format!(
                        "{} · {reason}",
                        hex_session::service::text::verbatim(path.display())
                    )),
                ),
            };
            let mut entry = div()
                .id(("missing-media-row", index))
                .px_2()
                .py_1()
                .flex()
                .flex_col()
                .border_b_1()
                .border_color(rgb(theme::color(PaletteRole::Border)))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(div().flex_1().min_w_0().truncate().child(theme::text(
                            hex_session::service::text::verbatim(media.file_name()),
                        )))
                        .child(div().text_color(muted).child(theme::text(status)))
                        .child(button(("missing-media-locate", index), "locate…").on_click(
                            cx.listener(move |view, _, _, cx| view.missing_media_locate(index, cx)),
                        )),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(theme::text(hex_session::service::text::verbatim(
                            media.expected_path.display(),
                        ))),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(muted)
                        .truncate()
                        .child(theme::text(format!("used in {uses}"))),
                );
            if let Some(detail) = detail {
                entry = entry.child(
                    div()
                        .text_xs()
                        .truncate()
                        .child(theme::text(format!("→ {detail}"))),
                );
            }
            if let Resolution::Choices(paths) = &row.resolution {
                for (choice, path) in paths.iter().enumerate() {
                    entry = entry.child(
                        div()
                            .id(("missing-media-choice", index * 64 + choice))
                            .text_xs()
                            .truncate()
                            .cursor_pointer()
                            .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
                            .child(theme::text(format!(
                                "use {}",
                                hex_session::service::text::verbatim(path.display())
                            )))
                            .on_click(cx.listener(move |view, _, _, cx| {
                                view.missing_media_choose(index, choice, cx)
                            })),
                    );
                }
            }
            list = list.children([entry]);
        }
        let found = panel
            .rows
            .iter()
            .filter(|row| matches!(row.resolution, Resolution::Found(..)))
            .count();
        let footer_note = match &panel.search {
            Some(search) => Some(format!(
                "searching · {} entries",
                search.progress.load(Ordering::Relaxed)
            )),
            None => panel.note.clone(),
        };
        let search_button = if panel.search.is_some() {
            button("missing-media-cancel-search", "cancel search").on_click(cx.listener(
                |view, _, _, cx| {
                    if let Some(search) =
                        view.missing_media.as_ref().and_then(|p| p.search.as_ref())
                    {
                        search.cancel.store(true, Ordering::Relaxed);
                    }
                    cx.notify();
                },
            ))
        } else {
            button("missing-media-search", "search folders…")
                .on_click(cx.listener(|view, _, _, cx| view.missing_media_search(cx)))
        };
        let relinking = matches!(panel.target, Target::Relink);
        let catalog_button = panel.catalog.as_ref().and_then(|offer| {
            let (unique, several) = offer.counts(&panel.rows);
            (unique + several > 0).then(|| {
                let mut label = format!("relink all found ({unique})");
                if several > 0 {
                    label = format!("relink all found ({unique}) · {several} to choose");
                }
                if offer.partial {
                    label.push_str(" · catalog still scanning");
                }
                button("missing-media-relink-catalog", label)
                    .on_click(cx.listener(|view, _, _, cx| view.missing_media_relink_catalog(cx)))
            })
        });
        let can_open = panel.complete() && panel.search.is_none();
        let offline_button =
            (!relinking && !panel.complete() && panel.search.is_none()).then(|| {
                button("missing-media-open-offline", "open with missing offline")
                    .on_click(cx.listener(|view, _, _, cx| view.missing_media_open_with(true, cx)))
            });
        let open_button = if relinking {
            let can_relink = found > 0 && panel.search.is_none();
            button("missing-media-relink", "relink")
                .when(!can_relink, |button| {
                    button.text_color(muted).cursor_default()
                })
                .on_click(cx.listener(|view, _, _, cx| view.missing_media_relink(cx)))
        } else {
            button("missing-media-open", "open")
                .when(!can_open, |button| {
                    button.text_color(muted).cursor_default()
                })
                .on_click(cx.listener(|view, _, _, cx| view.missing_media_open(cx)))
        };
        let hint = if relinking {
            "relink found files as one undoable edit; the rest stay offline"
        } else {
            "open once every file is found, or open with missing files offline"
        };
        Some(
            div()
                .id("missing-media-overlay")
                .debug_selector(|| "missing-media-overlay".into())
                .absolute()
                .inset_0()
                .pt(px(super::header::WINDOW_CONTROL_HEIGHT))
                .flex()
                .justify_center()
                .bg(rgb(theme::color(PaletteRole::Background)))
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_scroll_wheel(|_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .w_full()
                        .max_w(px(720.0))
                        .min_w_0()
                        .h_full()
                        .px_3()
                        .py_2()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .text_sm()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(div().font_weight(gpui::FontWeight::BOLD).child(
                                    theme::text(format!(
                                        "missing files: {}",
                                        panel.rows.len() - found
                                    )),
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .text_xs()
                                        .text_color(muted)
                                        .child(theme::text(hint)),
                                ),
                        )
                        .children([list])
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .children(catalog_button)
                                .children([search_button])
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .truncate()
                                        .text_xs()
                                        .text_color(muted)
                                        .children(footer_note.map(theme::text)),
                                )
                                .child(
                                    button(
                                        "missing-media-dismiss",
                                        if relinking { "close" } else { "cancel" },
                                    )
                                    .on_click(
                                        cx.listener(|view, _, _, cx| {
                                            view.missing_media_dismiss(cx)
                                        }),
                                    ),
                                )
                                .children(offline_button)
                                .children([open_button]),
                        ),
                ),
        )
    }
}

fn open_request(
    source: ProjectSource,
    relinks: BTreeMap<hex_project::AssetId, PathBuf>,
    mode: DocumentAudioMode,
    reservation: Option<SourcePlanReservation>,
) -> Request {
    match mode {
        DocumentAudioMode::Resident => Request::OpenWith { source, relinks },
        DocumentAudioMode::VerifiedStreaming => Request::OpenWithAudioMode {
            source,
            relinks,
            mode,
            reservation,
        },
    }
}

fn button(id: impl Into<gpui::ElementId>, label: impl AsRef<str>) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .px_2()
        .cursor_pointer()
        .hover(|s| s.bg(rgb(theme::color(PaletteRole::Hover))))
        .child(theme::text(label))
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "ui-test-support"))]
mod ui_tests;
