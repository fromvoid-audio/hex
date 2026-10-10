//! Application-wide four-color theme preview and background persistence.
use super::{EditorView, ThemeSettingsEscape};
use crate::theme::{self, PaletteRole};
use gpui::{
    AnyWindowHandle, AppContext, Context, Div, Entity, FocusHandle, Stateful, Subscription, Window,
    div, prelude::*, px, rgb, rgba,
};
use hex_preferences::ThemePreferences;
use hex_widgets::{
    NumericControl, NumericControlConfig, NumericControlEvent, NumericControlKind,
    NumericEventPhase, NumericScale, NumericSpec,
};
use std::{collections::HashMap, path::PathBuf};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ThemeChannel {
    BackgroundRed,
    BackgroundGreen,
    BackgroundBlue,
    TextRed,
    TextGreen,
    TextBlue,
    PrimaryRed,
    PrimaryGreen,
    PrimaryBlue,
    SecondaryRed,
    SecondaryGreen,
    SecondaryBlue,
}

impl ThemeChannel {
    const ALL: [Self; 12] = [
        Self::BackgroundRed,
        Self::BackgroundGreen,
        Self::BackgroundBlue,
        Self::TextRed,
        Self::TextGreen,
        Self::TextBlue,
        Self::PrimaryRed,
        Self::PrimaryGreen,
        Self::PrimaryBlue,
        Self::SecondaryRed,
        Self::SecondaryGreen,
        Self::SecondaryBlue,
    ];

    fn color_name(self) -> &'static str {
        match self {
            Self::BackgroundRed | Self::BackgroundGreen | Self::BackgroundBlue => "background",
            Self::TextRed | Self::TextGreen | Self::TextBlue => "text",
            Self::PrimaryRed | Self::PrimaryGreen | Self::PrimaryBlue => "primary",
            Self::SecondaryRed | Self::SecondaryGreen | Self::SecondaryBlue => "secondary",
        }
    }

    fn channel_name(self) -> &'static str {
        match self {
            Self::BackgroundRed | Self::TextRed | Self::PrimaryRed | Self::SecondaryRed => "r",
            Self::BackgroundGreen | Self::TextGreen | Self::PrimaryGreen | Self::SecondaryGreen => {
                "g"
            }
            Self::BackgroundBlue | Self::TextBlue | Self::PrimaryBlue | Self::SecondaryBlue => "b",
        }
    }

    fn get(self, colors: ThemePreferences) -> u8 {
        let color = match self {
            Self::BackgroundRed | Self::BackgroundGreen | Self::BackgroundBlue => colors.background,
            Self::TextRed | Self::TextGreen | Self::TextBlue => colors.text,
            Self::PrimaryRed | Self::PrimaryGreen | Self::PrimaryBlue => colors.primary,
            Self::SecondaryRed | Self::SecondaryGreen | Self::SecondaryBlue => colors.secondary,
        };
        match self.channel_name() {
            "r" => color.red,
            "g" => color.green,
            _ => color.blue,
        }
    }

    fn set(self, colors: &mut ThemePreferences, value: u8) {
        let color = match self {
            Self::BackgroundRed | Self::BackgroundGreen | Self::BackgroundBlue => {
                &mut colors.background
            }
            Self::TextRed | Self::TextGreen | Self::TextBlue => &mut colors.text,
            Self::PrimaryRed | Self::PrimaryGreen | Self::PrimaryBlue => &mut colors.primary,
            Self::SecondaryRed | Self::SecondaryGreen | Self::SecondaryBlue => {
                &mut colors.secondary
            }
        };
        match self.channel_name() {
            "r" => color.red = value,
            "g" => color.green = value,
            _ => color.blue = value,
        }
    }
}

struct ColorControl {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}

pub(super) struct ThemeSettingsState {
    path: PathBuf,
    window: AnyWindowHandle,
    loading: bool,
    writable: bool,
    pub(super) open: bool,
    saving: bool,
    load_generation: u64,
    save_generation: u64,
    pending_save: Option<(u64, ThemePreferences)>,
    saving_colors: Option<(u64, ThemePreferences)>,
    before_open: ThemePreferences,
    draft: ThemePreferences,
    error: Option<String>,
    focus: FocusHandle,
    controls: HashMap<ThemeChannel, ColorControl>,
}

impl ThemeSettingsState {
    pub(super) fn new(path: PathBuf, window: &Window, cx: &mut Context<EditorView>) -> Self {
        let colors = theme::preferences();
        Self {
            path,
            window: window.window_handle(),
            loading: true,
            writable: true,
            open: false,
            saving: false,
            load_generation: 1,
            save_generation: 0,
            pending_save: None,
            saving_colors: None,
            before_open: colors,
            draft: colors,
            error: None,
            focus: cx.focus_handle(),
            controls: HashMap::new(),
        }
    }
}

pub(super) fn default_preferences_path() -> PathBuf {
    #[cfg(test)]
    {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT_TEST_PATH: AtomicU64 = AtomicU64::new(0);
        std::env::temp_dir().join(format!(
            "hex-ui-test-theme-{}-{}.json",
            std::process::id(),
            NEXT_TEST_PATH.fetch_add(1, Ordering::Relaxed)
        ))
    }
    #[cfg(not(test))]
    {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        hex_preferences::theme_preferences_path(&home)
    }
}

impl EditorView {
    pub(super) fn start_theme_preferences_load(&mut self, cx: &mut Context<Self>) {
        let path = self.theme_settings.path.clone();
        let generation = self.theme_settings.load_generation;
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hex_preferences::load_theme_preferences(&path) })
                .await;
            let _ = this.update(cx, |view, cx| {
                view.theme_preferences_loaded(generation, result, cx);
                cx.notify();
            });
        })
        .detach();
    }

    fn theme_preferences_loaded(
        &mut self,
        generation: u64,
        result: Result<ThemePreferences, hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.theme_settings.load_generation {
            return;
        }
        self.theme_settings.loading = false;
        match result {
            Ok(colors) => {
                self.theme_settings.draft = colors;
                if theme::colors_are_legible(colors) {
                    self.theme_settings.before_open = colors;
                    theme::apply_preferences(colors);
                } else {
                    self.theme_settings.before_open = theme::preferences();
                    self.theme_settings.error = Some(
                        "saved theme colors need more contrast; adjust the colors before saving"
                            .into(),
                    );
                }
            }
            Err(error) => {
                self.theme_settings.writable = false;
                self.theme_settings.error = Some(format!(
                    "could not load theme settings · {}",
                    error.to_string().to_lowercase()
                ));
            }
        }
        if self.theme_settings.open {
            self.create_theme_controls(cx);
        }
    }

    pub(super) fn open_theme_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_settings.open {
            self.theme_settings.focus.focus(window);
            return;
        }
        self.theme_settings.open = true;
        if self.theme_settings.loading {
            self.theme_settings.error = Some("loading theme settings…".into());
        } else if self.theme_settings.writable
            && theme::colors_are_legible(self.theme_settings.draft)
        {
            self.theme_settings.error = None;
        }
        self.theme_settings.before_open = theme::preferences();
        if theme::colors_are_legible(self.theme_settings.draft) {
            self.theme_settings.draft = self.theme_settings.before_open;
        }
        if !self.theme_settings.loading {
            self.create_theme_controls(cx);
        }
        self.theme_settings.focus.focus(window);
        cx.notify();
    }

    pub(super) fn theme_settings_escape(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_settings.open {
            self.cancel_theme_settings(window, cx);
        }
    }

    fn create_theme_controls(&mut self, cx: &mut Context<Self>) {
        if !self.theme_settings.controls.is_empty() {
            self.sync_theme_controls(cx);
            return;
        }
        let style = super::device_panel::control_style();
        for channel in ThemeChannel::ALL {
            let spec = match NumericSpec::new(
                0.0,
                255.0,
                0.0,
                NumericScale::Linear,
                0,
                None::<std::sync::Arc<str>>,
            ) {
                Ok(spec) => spec,
                Err(error) => {
                    self.theme_settings.error = Some(error.to_string());
                    return;
                }
            };
            let id = format!("theme-{}-{}", channel.color_name(), channel.channel_name());
            let config = match NumericControlConfig::new(
                id,
                theme::text(channel.channel_name()),
                NumericControlKind::HorizontalSlider,
                spec,
                f64::from(channel.get(self.theme_settings.draft)),
                style,
            ) {
                Ok(config) => config,
                Err(error) => {
                    self.theme_settings.error = Some(error.to_string());
                    return;
                }
            };
            let entity = cx.new(|control_cx| NumericControl::from_config(control_cx, config));
            let subscription =
                cx.subscribe(&entity, move |view, _, event: &NumericControlEvent, cx| {
                    if matches!(
                        event.phase,
                        NumericEventPhase::Change
                            | NumericEventPhase::Commit
                            | NumericEventPhase::Cancel
                    ) {
                        view.update_theme_channel(channel, event.value, cx);
                    }
                });
            self.theme_settings.controls.insert(
                channel,
                ColorControl {
                    entity,
                    _subscription: subscription,
                },
            );
        }
    }

    fn update_theme_channel(&mut self, channel: ThemeChannel, value: f64, cx: &mut Context<Self>) {
        if self.theme_settings.saving {
            return;
        }
        if !value.is_finite() || !(0.0..=255.0).contains(&value) {
            self.theme_settings.error = Some("color channel must be between 0 and 255".into());
            return;
        }
        channel.set(&mut self.theme_settings.draft, value.round() as u8);
        let valid = theme::colors_are_legible(self.theme_settings.draft);
        if valid {
            theme::apply_preferences(self.theme_settings.draft);
        }
        self.theme_settings.error =
            (!valid).then(|| "text needs 4.5:1 contrast; accent colors need 3:1 contrast".into());
        cx.notify();
    }

    fn sync_theme_controls(&mut self, cx: &mut Context<Self>) {
        let colors = self.theme_settings.draft;
        let style = super::device_panel::control_style();
        for (channel, control) in &self.theme_settings.controls {
            let value = f64::from(channel.get(colors));
            control.entity.update(cx, |control, cx| {
                let _ = control.set_external_style(style, cx);
                let _ = control.set_external_value(value, cx);
            });
        }
    }

    fn collect_theme_control_drafts(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Result<ThemePreferences, String> {
        let mut colors = self.theme_settings.draft;
        for (channel, control) in &self.theme_settings.controls {
            let result = control.entity.update(cx, |control, cx| {
                let prepared = control
                    .prepare_finish_edit_for_owner(cx)
                    .map_err(|error| error.to_string())?;
                if let Some(prepared) = prepared {
                    channel.set(&mut colors, prepared.value.round() as u8);
                    control.accept_finish_edit_for_owner(prepared, cx);
                }
                Ok::<(), String>(())
            });
            result.map_err(|error| error.to_string())?;
        }
        self.theme_settings.draft = colors;
        let valid = theme::colors_are_legible(colors);
        if valid {
            theme::apply_preferences(colors);
        }
        self.theme_settings.error =
            (!valid).then(|| "text needs 4.5:1 contrast; accent colors need 3:1 contrast".into());
        Ok(colors)
    }

    pub(super) fn reset_theme_settings(&mut self, cx: &mut Context<Self>) {
        if self.theme_settings.saving {
            return;
        }
        match self.collect_theme_control_drafts(cx) {
            Ok(_) => {
                let defaults = ThemePreferences::default();
                self.theme_settings.draft = defaults;
                theme::apply_preferences(defaults);
                self.theme_settings.error = None;
                self.sync_theme_controls(cx);
            }
            Err(error) => {
                self.theme_settings.error = Some(format!("invalid color value · {error}"))
            }
        }
        cx.notify();
    }

    pub(super) fn save_theme_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_settings.loading
            || self.theme_settings.saving
            || !self.theme_settings.writable
        {
            self.theme_settings.error = Some("theme settings cannot be saved".into());
            cx.notify();
            return;
        }
        let colors = match self.collect_theme_control_drafts(cx) {
            Ok(colors) => colors,
            Err(error) => {
                self.theme_settings.error = Some(format!("invalid color value · {error}"));
                cx.notify();
                return;
            }
        };
        if !theme::colors_are_legible(colors) {
            self.theme_settings.error =
                Some("text needs 4.5:1 contrast; accent colors need 3:1 contrast".into());
            cx.notify();
            return;
        }
        self.theme_settings.save_generation = self.theme_settings.save_generation.wrapping_add(1);
        self.theme_settings.pending_save = Some((self.theme_settings.save_generation, colors));
        self.start_theme_save(cx);
        let _ = window;
        cx.notify();
    }

    fn start_theme_save(&mut self, cx: &mut Context<Self>) {
        if self.theme_settings.saving || !self.theme_settings.writable {
            return;
        }
        let Some((generation, colors)) = self.theme_settings.pending_save.take() else {
            return;
        };
        self.theme_settings.saving = true;
        self.theme_settings.saving_colors = Some((generation, colors));
        self.theme_settings.error = Some("saving theme settings…".into());
        let path = self.theme_settings.path.clone();
        let window_handle = self.theme_settings.window;
        let editor_focus = self.focus.clone();
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { hex_preferences::save_theme_preferences(&path, &colors) })
                .await;
            let closed = this
                .update(cx, |view, cx| {
                    let closed = view.theme_settings_saved(generation, result, cx);
                    cx.notify();
                    closed
                })
                .unwrap_or(false);
            if closed {
                let _ = cx.update_window(window_handle, move |_, window, _| {
                    editor_focus.focus(window);
                });
            }
        })
        .detach();
    }

    fn theme_settings_saved(
        &mut self,
        generation: u64,
        result: Result<(), hex_preferences::PreferencesError>,
        cx: &mut Context<Self>,
    ) -> bool {
        if generation != self.theme_settings.save_generation {
            return false;
        }
        self.theme_settings.saving = false;
        let saved_colors = self
            .theme_settings
            .saving_colors
            .take()
            .filter(|(saved_generation, _)| *saved_generation == generation)
            .map(|(_, colors)| colors);
        if let Some((_, _)) = self.theme_settings.pending_save {
            self.start_theme_save(cx);
            return false;
        }
        match result {
            Ok(()) => {
                if let Some(colors) = saved_colors {
                    if self.theme_settings.draft == colors {
                        self.theme_settings.open = false;
                        self.theme_settings.error = None;
                        self.theme_settings.controls.clear();
                        return true;
                    } else {
                        self.theme_settings.save_generation =
                            self.theme_settings.save_generation.wrapping_add(1);
                        self.theme_settings.pending_save = Some((
                            self.theme_settings.save_generation,
                            self.theme_settings.draft,
                        ));
                        self.start_theme_save(cx);
                    }
                }
            }
            Err(error) => {
                self.theme_settings.error = Some(format!(
                    "could not save theme settings · {}",
                    error.to_string().to_lowercase()
                ));
            }
        }
        false
    }

    pub(super) fn cancel_theme_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.theme_settings.saving {
            self.theme_settings.error = Some("theme save in progress".into());
            cx.notify();
            return;
        }
        theme::apply_preferences(self.theme_settings.before_open);
        self.theme_settings.draft = self.theme_settings.before_open;
        self.theme_settings.open = false;
        self.theme_settings.error = None;
        self.theme_settings.controls.clear();
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn prepare_theme_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.theme_settings.saving {
            self.theme_settings.error = Some("theme save in progress".into());
            cx.notify();
            return false;
        }
        if self.theme_settings.open {
            theme::apply_preferences(self.theme_settings.before_open);
            self.theme_settings.draft = self.theme_settings.before_open;
            self.theme_settings.open = false;
            self.theme_settings.controls.clear();
            self.focus.focus(window);
            cx.notify();
        }
        true
    }

    pub(super) fn render_theme_settings(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !self.theme_settings.open {
            return None;
        }
        let colors = self.theme_settings.draft;
        let style = super::device_panel::control_style();
        let error = self.theme_settings.error.clone();
        let writable = self.theme_settings.writable;
        let saving = self.theme_settings.saving;
        let mut rows = div().flex().flex_col().gap(px(6.0));
        for (name, color, channels) in [
            (
                "background",
                colors.background,
                [
                    ThemeChannel::BackgroundRed,
                    ThemeChannel::BackgroundGreen,
                    ThemeChannel::BackgroundBlue,
                ],
            ),
            (
                "text",
                colors.text,
                [
                    ThemeChannel::TextRed,
                    ThemeChannel::TextGreen,
                    ThemeChannel::TextBlue,
                ],
            ),
            (
                "primary",
                colors.primary,
                [
                    ThemeChannel::PrimaryRed,
                    ThemeChannel::PrimaryGreen,
                    ThemeChannel::PrimaryBlue,
                ],
            ),
            (
                "secondary",
                colors.secondary,
                [
                    ThemeChannel::SecondaryRed,
                    ThemeChannel::SecondaryGreen,
                    ThemeChannel::SecondaryBlue,
                ],
            ),
        ] {
            let hex = format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue);
            let mut channel_controls = div().flex().flex_1().gap(px(4.0));
            for channel in channels {
                if let Some(control) = self.theme_settings.controls.get(&channel) {
                    control.entity.update(cx, |control, cx| {
                        let _ = control.set_external_style(style, cx);
                        control.set_enabled(!saving, cx);
                        let _ = control.set_external_value(f64::from(channel.get(colors)), cx);
                    });
                    channel_controls = channel_controls.child(
                        div()
                            .flex_1()
                            .min_w(px(72.0))
                            .h(px(26.0))
                            .child(control.entity.clone()),
                    );
                }
            }
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.0))
                    .child(div().size(px(12.0)).bg(rgb(color.packed())))
                    .child(
                        div()
                            .w(px(78.0))
                            .text_xs()
                            .text_color(rgb(style.text))
                            .child(theme::text(name)),
                    )
                    .child(
                        div()
                            .w(px(56.0))
                            .text_xs()
                            .text_color(rgb(style.muted))
                            .child(theme::text(hex)),
                    )
                    .child(channel_controls),
            );
        }
        let mut card = div()
            .id("theme settings panel")
            .debug_selector(|| "theme settings panel".into())
            .track_focus(&self.theme_settings.focus)
            .key_context("hex_theme_settings")
            .w(px(560.0))
            .max_w(px(600.0))
            .p(px(10.0))
            .gap(px(8.0))
            .flex()
            .flex_col()
            .border_1()
            .border_color(rgb(style.border))
            .bg(rgb(style.surface))
            .font_family(theme::FONT_FAMILY)
            .text_color(rgb(style.text))
            .shadow_lg()
            .child(div().text_sm().child(theme::text("theme colors")))
            .child(rows);
        if let Some(error) = error {
            card = card.child(
                div()
                    .id("theme settings status")
                    .debug_selector(|| "theme settings status".into())
                    .text_xs()
                    .text_color(rgb(theme::readable_text(
                        style.surface,
                        theme::color(PaletteRole::Primary),
                    )))
                    .child(theme::text(error)),
            );
        }
        let save_label = if saving { "saving…" } else { "save" };
        card = card.child(
            div()
                .flex()
                .justify_end()
                .gap(px(4.0))
                .child(self.theme_button(
                    "reset",
                    "reset theme colors",
                    |view, _, cx| view.reset_theme_settings(cx),
                    cx,
                ))
                .child(self.theme_button(
                    "cancel",
                    "cancel theme changes",
                    |view, window, cx| view.cancel_theme_settings(window, cx),
                    cx,
                ))
                .child(
                    self.theme_button(
                        save_label,
                        "save theme settings",
                        |view, window, cx| view.save_theme_settings(window, cx),
                        cx,
                    )
                    .opacity(if writable && !saving { 1.0 } else { 0.45 }),
                ),
        );
        Some(
            div()
                .id("theme settings backdrop")
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(rgba(0x00000088))
                .child(card),
        )
    }

    fn theme_button(
        &self,
        label: &'static str,
        id: &'static str,
        handler: impl Fn(&mut EditorView, &mut Window, &mut Context<EditorView>) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<Div> {
        div()
            .id(id)
            .debug_selector(move || id.into())
            .h(px(22.0))
            .px(px(7.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .border_1()
            .border_color(rgb(theme::color(PaletteRole::Border)))
            .bg(rgb(theme::color(PaletteRole::Raised)))
            .hover(|style| style.bg(rgb(theme::color(PaletteRole::Hover))))
            .text_xs()
            .on_click(cx.listener(move |view, _, window, cx| handler(view, window, cx)))
            .child(theme::text(label))
    }

    pub(super) fn on_theme_settings_escape_action(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.theme_settings_escape(window, cx);
    }
}

pub(super) fn overlay(
    view: &mut EditorView,
    cx: &mut Context<EditorView>,
) -> Option<Stateful<Div>> {
    view.render_theme_settings(cx)
}

pub(super) fn handle_escape_action(
    view: &mut EditorView,
    _action: &ThemeSettingsEscape,
    window: &mut Window,
    cx: &mut Context<EditorView>,
) {
    view.on_theme_settings_escape_action(window, cx);
}

#[cfg(all(test, feature = "ui-test-support"))]
mod tests {
    use super::*;
    use gpui::{
        AppContext, Empty, Modifiers, MouseButton, TestAppContext, VisualTestContext, point, px,
    };
    use hex_project::Project;
    use hex_session::service::{Service, ViewState};
    use std::sync::Arc;

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            use std::sync::atomic::{AtomicU64, Ordering};
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "hex-ui-theme-test-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path).expect("create isolated theme directory");
            Self(path)
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct RestoreTheme(ThemePreferences);

    impl Drop for RestoreTheme {
        fn drop(&mut self) {
            theme::apply_preferences(self.0);
        }
    }

    fn editor_fixture(
        cx: &mut TestAppContext,
        directory: &TestDirectory,
    ) -> (gpui::AnyWindowHandle, gpui::Entity<EditorView>) {
        cx.update(super::super::bind_editor_keys);
        let bootstrap = cx.add_window(|_, _| Empty);
        let handle = *bootstrap;
        let editor = cx
            .update_window(handle, |_, window, cx| {
                let service = Service::new(48_000, 32, super::super::MAX_RESIDENT_FRAMES)
                    .expect("test service");
                let editor = window.replace_root(cx, |window, cx| {
                    EditorView::new_for_test_with_theme_preferences_path(
                        window,
                        cx,
                        service,
                        Err("offline test".into()),
                        directory.0.join("browser.json"),
                        directory.0.join("recent.json"),
                        directory.0.join("theme.json"),
                    )
                });
                editor.update(cx, |view, _| {
                    view.state = Some(Arc::new(ViewState {
                        project: Arc::new(Project::new(48_000).expect("project")),
                        waveforms: Default::default(),
                        midi_preview: Arc::new(Default::default()),
                        timeline_frames: 384_000,
                        can_undo: false,
                        can_redo: false,
                        revision: 0,
                        unsaved: false,
                        document_path: None,
                    }));
                });
                editor
            })
            .expect("editor window");
        (handle, editor)
    }

    fn click_bounds(visual: &mut VisualTestContext, selector: &'static str) {
        let bounds = visual.debug_bounds(selector).expect("control bounds");
        let center = point(
            bounds.origin.x + bounds.size.width / 2.0,
            bounds.origin.y + bounds.size.height / 2.0,
        );
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
    }

    #[gpui::test]
    fn settings_preview_cancel_restores_palette_and_editor_focus(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        assert!(visual.debug_bounds("theme settings panel").is_some());

        let slider = visual
            .debug_bounds("theme-primary-r-slider")
            .expect("primary red slider");
        let start = point(
            slider.origin.x + slider.size.width / 2.0,
            slider.origin.y + slider.size.height / 2.0,
        );
        let end = point(start.x - px(12.0), start.y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(end, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(end, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_ne!(view.theme_settings.draft.primary.red, 0xf0);
            assert_ne!(theme::preferences(), ThemePreferences::default());
        });

        let draft = editor.read_with(cx, |view, _| view.theme_settings.draft);
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert!(view.theme_settings.open);
            assert_eq!(view.theme_settings.draft, draft);
        });

        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert!(!editor.read_with(cx, |view, _| view.theme_settings.open));
        assert_eq!(theme::preferences(), ThemePreferences::default());
        let focused = cx
            .update_window(handle, |_, window, cx| {
                editor.read_with(cx, |view, _| view.focus.is_focused(window))
            })
            .expect("window focus query");
        assert!(focused, "escape returns keyboard focus to the editor");
    }

    #[gpui::test]
    fn valid_theme_save_persists_and_returns_focus_to_editor(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.update_theme_channel(ThemeChannel::PrimaryRed, 235.0, cx)
        });
        visual.run_until_parked();
        click_bounds(&mut visual, "save theme settings");
        visual.run_until_parked();

        let saved = hex_preferences::load_theme_preferences(&directory.0.join("theme.json"))
            .expect("saved theme preferences");
        assert_eq!(saved.primary.red, 235);
        assert!(!editor.read_with(cx, |view, _| view.theme_settings.open));
        let focused = cx
            .update_window(handle, |_, window, cx| {
                editor.read_with(cx, |view, _| view.focus.is_focused(window))
            })
            .expect("window focus query");
        assert!(
            focused,
            "successful save returns keyboard focus to the editor"
        );
    }

    #[gpui::test]
    fn window_close_is_refused_while_theme_preferences_are_saving(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.update_theme_channel(ThemeChannel::SecondaryBlue, 120.0, cx);
            view.theme_settings.saving = true;
        });

        let allowed = cx
            .update_window(handle, |_, window, cx| {
                editor.update(cx, |view, cx| view.prepare_theme_window_close(window, cx))
            })
            .expect("window close guard");
        assert!(!allowed);
        editor.read_with(cx, |view, _| {
            assert!(view.theme_settings.open);
            assert!(view.theme_settings.saving);
            assert_eq!(view.theme_settings.draft.secondary.blue, 120);
            assert_eq!(
                view.theme_settings.error.as_deref(),
                Some("theme save in progress")
            );
        });
    }

    #[gpui::test]
    fn failed_theme_save_keeps_the_editable_preview_and_error_visible(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            // Loading used the valid missing-file path; point the save at a directory so the
            // asynchronous atomic publisher deterministically reports a recoverable I/O error.
            view.theme_settings.path = directory.0.clone();
            view.update_theme_channel(ThemeChannel::PrimaryRed, 220.0, cx)
        });
        click_bounds(&mut visual, "save theme settings");
        visual.run_until_parked();

        editor.read_with(cx, |view, _| {
            assert!(view.theme_settings.open);
            assert!(!view.theme_settings.saving);
            assert_eq!(view.theme_settings.draft.primary.red, 220);
            assert!(
                view.theme_settings
                    .error
                    .as_deref()
                    .is_some_and(|error| error.starts_with("could not save theme settings"))
            );
        });
        assert!(visual.debug_bounds("theme settings status").is_some());
    }

    #[gpui::test]
    fn contrast_invalid_saved_palette_is_preserved_but_not_applied(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        let initial = ThemePreferences::default();
        theme::apply_preferences(initial);
        let directory = TestDirectory::new();
        let mut invalid = initial;
        invalid.text = invalid.background;
        let path = directory.0.join("theme.json");
        hex_preferences::save_theme_preferences(&path, &invalid)
            .expect("persist structurally valid low-contrast colors");
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        assert_eq!(theme::preferences(), initial);
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.theme_settings.draft, invalid);
            assert!(
                view.theme_settings
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("need more contrast"))
            );
        });
        assert!(visual.debug_bounds("theme settings status").is_some());
        assert_eq!(
            hex_preferences::load_theme_preferences(&path).expect("preserved raw palette"),
            invalid
        );
    }
}
