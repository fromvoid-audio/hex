//! Application-wide four-color theme preview and background persistence.
use super::{EditorView, ThemeSettingsEscape};
use crate::theme::{self, PaletteRole};
use gpui::{
    AnyWindowHandle, AppContext, Context, Div, Entity, FocusHandle, Stateful, Subscription, Window,
    div, prelude::*, px, rgb, rgba,
};
use hex_preferences::ThemePreferences;
use hex_widgets::{
    ColorPicker, ColorPickerConfig, ColorPickerEvent, HsvColor, NumericControl,
    NumericControlConfig, NumericControlEvent, NumericControlKind, NumericEventPhase, NumericScale,
    NumericSpec, RgbColor,
};
use std::{collections::HashMap, path::PathBuf};

#[cfg(all(target_os = "macos", target_arch = "aarch64"))]
mod vst3_editor;

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

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum ThemeRole {
    Background,
    Text,
    Primary,
    Secondary,
}

impl ThemeRole {
    const ALL: [Self; 4] = [Self::Background, Self::Text, Self::Primary, Self::Secondary];

    fn name(self) -> &'static str {
        match self {
            Self::Background => "background",
            Self::Text => "text",
            Self::Primary => "primary",
            Self::Secondary => "secondary",
        }
    }

    fn color(self, colors: ThemePreferences) -> hex_preferences::RgbColor {
        match self {
            Self::Background => colors.background,
            Self::Text => colors.text,
            Self::Primary => colors.primary,
            Self::Secondary => colors.secondary,
        }
    }

    fn set(self, colors: &mut ThemePreferences, color: hex_preferences::RgbColor) {
        match self {
            Self::Background => colors.background = color,
            Self::Text => colors.text = color,
            Self::Primary => colors.primary = color,
            Self::Secondary => colors.secondary = color,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
enum HsbChannel {
    Hue,
    Saturation,
    Brightness,
}

impl HsbChannel {
    const ALL: [Self; 3] = [Self::Hue, Self::Saturation, Self::Brightness];

    fn name(self) -> &'static str {
        match self {
            Self::Hue => "h",
            Self::Saturation => "s",
            Self::Brightness => "b",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Hue => "hue",
            Self::Saturation => "sat",
            Self::Brightness => "bright",
        }
    }

    fn range(self) -> (f64, f64, u8) {
        match self {
            Self::Hue => (0.0, 359.0, 0),
            Self::Saturation | Self::Brightness => (0.0, 100.0, 0),
        }
    }

    fn get(self, hsv: HsvColor) -> f64 {
        match self {
            Self::Hue => hsv.hue,
            Self::Saturation => hsv.saturation,
            Self::Brightness => hsv.value,
        }
    }

    fn set(self, hsv: &mut HsvColor, value: f64) {
        match self {
            Self::Hue => hsv.hue = value,
            Self::Saturation => hsv.saturation = value,
            Self::Brightness => hsv.value = value,
        }
    }
}

struct ColorControl {
    entity: Entity<NumericControl>,
    _subscription: Subscription,
}

pub(super) struct ThemeSettingsState {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    vst3_editor: vst3_editor::State,
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
    selected_role: ThemeRole,
    role_hues: HashMap<ThemeRole, f64>,
    error: Option<String>,
    focus: FocusHandle,
    controls: HashMap<ThemeChannel, ColorControl>,
    hsb_controls: HashMap<HsbChannel, ColorControl>,
    picker: Option<Entity<ColorPicker>>,
    picker_subscription: Option<Subscription>,
}

impl ThemeSettingsState {
    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn test_path(&self) -> &std::path::Path {
        &self.path
    }
    pub(super) fn new(path: PathBuf, window: &Window, cx: &mut Context<EditorView>) -> Self {
        let colors = theme::preferences();
        Self {
            #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
            vst3_editor: vst3_editor::State::new(path.with_file_name("vst3-editor.json")),
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
            selected_role: ThemeRole::Primary,
            role_hues: ThemeRole::ALL
                .into_iter()
                .map(|role| {
                    let color = role.color(colors);
                    let hsv = hex_widgets::rgb_to_hsv(
                        RgbColor::new(color.red, color.green, color.blue),
                        0.0,
                    );
                    (role, hsv.hue)
                })
                .collect(),
            error: None,
            focus: cx.focus_handle(),
            controls: HashMap::new(),
            hsb_controls: HashMap::new(),
            picker: None,
            picker_subscription: None,
        }
    }
}

#[cfg(not(test))]
pub(super) fn default_preferences_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"));
    hex_preferences::theme_preferences_path(&home)
}

impl EditorView {
    pub(super) fn start_theme_preferences_load(&mut self, cx: &mut Context<Self>) {
        let path = self.theme_settings.path.clone();
        let generation = self.theme_settings.load_generation;
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::load_theme_preferences(&path)
                })
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
                    hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
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
        if self.controller_input_is_open() {
            self.close_controller_input_menu(window, cx);
        }
        self.prepare_controller_settings(cx);
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        self.load_vst3_editor_settings(cx);
        self.project_rate_settings.clear_draft();
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
        let style = crate::theme::control_style();
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
        for channel in HsbChannel::ALL {
            let (minimum, maximum, precision) = channel.range();
            let spec = match NumericSpec::new(
                minimum,
                maximum,
                minimum,
                NumericScale::Linear,
                precision,
                None::<std::sync::Arc<str>>,
            ) {
                Ok(spec) => spec,
                Err(error) => {
                    self.theme_settings.error = Some(error.to_string());
                    return;
                }
            };
            let hsv = self.selected_hsv();
            let config = match NumericControlConfig::new(
                format!("theme-hsb-{}", channel.name()),
                theme::text(channel.label()),
                NumericControlKind::HorizontalSlider,
                spec,
                channel.get(hsv),
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
                        view.update_theme_hsb(channel, event.value, cx);
                    }
                });
            self.theme_settings.hsb_controls.insert(
                channel,
                ColorControl {
                    entity,
                    _subscription: subscription,
                },
            );
        }
        let role = self.theme_settings.selected_role;
        let color = role.color(self.theme_settings.draft);
        let rgb = RgbColor::new(color.red, color.green, color.blue);
        let hue = self
            .theme_settings
            .role_hues
            .get(&role)
            .copied()
            .unwrap_or(0.0);
        let config = match ColorPickerConfig::new("theme-color-picker", rgb, hue, style) {
            Ok(config) => config,
            Err(error) => {
                self.theme_settings.error = Some(error.into());
                return;
            }
        };
        let entity = cx.new(|picker_cx| ColorPicker::from_config(picker_cx, config));
        let subscription = cx.subscribe(&entity, |view, _, event: &ColorPickerEvent, cx| {
            view.update_theme_picker(*event, cx);
        });
        self.theme_settings.picker = Some(entity);
        self.theme_settings.picker_subscription = Some(subscription);
        self.sync_theme_controls(cx);
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
        if self.theme_settings.selected_role.name() == channel.color_name() {
            self.sync_theme_controls(cx);
        }
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
        let style = crate::theme::control_style();
        for (channel, control) in &self.theme_settings.controls {
            let value = f64::from(channel.get(colors));
            control.entity.update(cx, |control, cx| {
                let _ = control.set_external_style(style, cx);
                let _ = control.set_external_value(value, cx);
            });
        }
        let role = self.theme_settings.selected_role;
        let color = role.color(colors);
        let hue = self
            .theme_settings
            .role_hues
            .get(&role)
            .copied()
            .unwrap_or(0.0);
        if let Some(picker) = self.theme_settings.picker.clone() {
            let hsv = picker.update(cx, |picker, cx| {
                picker.set_external_color(
                    RgbColor::new(color.red, color.green, color.blue),
                    hue,
                    cx,
                );
                picker.hsv()
            });
            self.theme_settings.role_hues.insert(role, hsv.hue);
            for (channel, control) in &self.theme_settings.hsb_controls {
                let value = channel.get(hsv);
                control.entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    let _ = control.set_external_value(value, cx);
                });
            }
        }
    }

    fn selected_hsv(&self) -> HsvColor {
        let role = self.theme_settings.selected_role;
        let color = role.color(self.theme_settings.draft);
        let preferred = self
            .theme_settings
            .role_hues
            .get(&role)
            .copied()
            .unwrap_or(0.0);
        hex_widgets::rgb_to_hsv(RgbColor::new(color.red, color.green, color.blue), preferred)
    }

    fn select_theme_role(&mut self, role: ThemeRole, cx: &mut Context<Self>) {
        self.theme_settings.selected_role = role;
        if let Some(picker) = self.theme_settings.picker.as_ref() {
            let color = role.color(self.theme_settings.draft);
            let rgb = RgbColor::new(color.red, color.green, color.blue);
            let hue = self
                .theme_settings
                .role_hues
                .get(&role)
                .copied()
                .unwrap_or(0.0);
            picker.update(cx, |picker, cx| {
                picker.cancel_pointer(cx);
                picker.set_external_color(rgb, hue, cx);
            });
        }
        self.sync_theme_controls(cx);
        cx.notify();
    }

    fn update_theme_hsb(&mut self, channel: HsbChannel, value: f64, cx: &mut Context<Self>) {
        if self.theme_settings.saving {
            return;
        }
        let (minimum, maximum, _) = channel.range();
        if !value.is_finite() || !(minimum..=maximum).contains(&value) {
            self.theme_settings.error = Some("hsv value is outside its valid range".into());
            return;
        }
        let Some(picker) = self.theme_settings.picker.as_ref() else {
            return;
        };
        picker.update(cx, |picker, cx| {
            let mut hsv = picker.hsv();
            channel.set(&mut hsv, value);
            let _ = picker.set_hsv(hsv, cx);
        });
    }

    fn update_theme_picker(&mut self, event: ColorPickerEvent, cx: &mut Context<Self>) {
        if self.theme_settings.saving {
            return;
        }
        let role = self.theme_settings.selected_role;
        role.set(
            &mut self.theme_settings.draft,
            hex_preferences::RgbColor::new(event.color.red, event.color.green, event.color.blue),
        );
        self.theme_settings.role_hues.insert(role, event.hsv.hue);
        let valid = theme::colors_are_legible(self.theme_settings.draft);
        if valid {
            theme::apply_preferences(self.theme_settings.draft);
        }
        self.theme_settings.error =
            (!valid).then(|| "text needs 4.5:1 contrast; accent colors need 3:1 contrast".into());
        self.sync_theme_controls(cx);
        cx.notify();
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
        let mut hsv = self.selected_hsv();
        for (channel, control) in &self.theme_settings.hsb_controls {
            let result = control.entity.update(cx, |control, cx| {
                let prepared = control
                    .prepare_finish_edit_for_owner(cx)
                    .map_err(|error| error.to_string())?;
                if let Some(prepared) = prepared {
                    channel.set(&mut hsv, prepared.value);
                    control.accept_finish_edit_for_owner(prepared, cx);
                }
                Ok::<(), String>(())
            });
            result.map_err(|error| error.to_string())?;
        }
        let Some(color) = hex_widgets::hsv_to_rgb(hsv) else {
            return Err("hsv value is outside its valid range".into());
        };
        self.theme_settings.selected_role.set(
            &mut self.theme_settings.draft,
            hex_preferences::RgbColor::new(color.red, color.green, color.blue),
        );
        self.theme_settings
            .role_hues
            .insert(self.theme_settings.selected_role, hsv.hue);
        let colors = self.theme_settings.draft;
        let valid = theme::colors_are_legible(colors);
        if valid {
            theme::apply_preferences(colors);
        }
        self.sync_theme_controls(cx);
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
        if self.project_rate_settings.applying() {
            return;
        }
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
        #[cfg(test)]
        let test_environment = std::sync::Arc::clone(&self.test_environment);
        let this = cx.entity().downgrade();
        cx.spawn(async move |_, cx| {
            let result = cx
                .background_executor()
                .spawn(async move {
                    #[cfg(test)]
                    let _test_environment = test_environment;
                    hex_preferences::save_theme_preferences(&path, &colors)
                })
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
                        self.theme_settings.hsb_controls.clear();
                        self.theme_settings.picker = None;
                        self.theme_settings.picker_subscription = None;
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
                    hex_session::service::text::lowercase_outside_verbatim(&error.to_string())
                ));
            }
        }
        false
    }

    pub(super) fn cancel_theme_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.project_rate_settings.applying() {
            return;
        }
        self.project_rate_settings.clear_draft();
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
        self.theme_settings.hsb_controls.clear();
        self.theme_settings.picker = None;
        self.theme_settings.picker_subscription = None;
        self.focus.focus(window);
        cx.notify();
    }

    pub(super) fn prepare_theme_window_close(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if self.project_rate_settings.applying() {
            return false;
        }
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
            self.theme_settings.hsb_controls.clear();
            self.theme_settings.picker = None;
            self.theme_settings.picker_subscription = None;
            self.focus.focus(window);
            cx.notify();
        }
        true
    }

    #[cfg(all(test, feature = "ui-test-support"))]
    pub(super) fn mark_theme_save_pending_for_update_test(&mut self) {
        self.theme_settings.saving = true;
    }

    pub(super) fn render_theme_settings(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        if !self.theme_settings.open {
            return None;
        }
        let colors = self.theme_settings.draft;
        let style = crate::theme::control_style();
        let error = self.theme_settings.error.clone();
        let writable = self.theme_settings.writable;
        let saving = self.theme_settings.saving;
        let mut rows = div().flex().gap(px(6.0));
        for role in ThemeRole::ALL {
            let color = role.color(colors);
            let selected = role == self.theme_settings.selected_role;
            let id = format!("theme-role-{}", role.name());
            rows = rows.child(
                div()
                    .id(gpui::SharedString::from(id.clone()))
                    .debug_selector(move || id.clone())
                    .flex_1()
                    .h(px(28.0))
                    .px(px(6.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .border_1()
                    .border_color(rgb(if selected { style.accent } else { style.border }))
                    .bg(rgb(color.packed()))
                    .hover(|hover| hover.border_color(rgb(style.accent)))
                    .text_xs()
                    .text_color(rgb(theme::readable_text(color.packed(), style.text)))
                    .on_click(cx.listener(move |view, _, _, cx| view.select_theme_role(role, cx)))
                    .child(theme::text(role.name())),
            );
        }
        let role = self.theme_settings.selected_role;
        let color = role.color(colors);
        let hex = format!("#{:02x}{:02x}{:02x}", color.red, color.green, color.blue);
        let mut controls = div().flex().gap(px(4.0));
        for channel in ThemeChannel::ALL
            .into_iter()
            .filter(|channel| channel.color_name() == role.name())
        {
            if let Some(control) = self.theme_settings.controls.get(&channel) {
                control.entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(!saving, cx);
                    let _ = control.set_external_value(f64::from(channel.get(colors)), cx);
                });
                controls = controls.child(
                    div()
                        .flex_1()
                        .min_w(px(72.0))
                        .h(px(26.0))
                        .child(control.entity.clone()),
                );
            }
        }
        let mut element = div().flex().flex_col().gap(px(4.0));
        for channel in HsbChannel::ALL {
            if let Some(control) = self.theme_settings.hsb_controls.get(&channel) {
                control.entity.update(cx, |control, cx| {
                    let _ = control.set_external_style(style, cx);
                    control.set_enabled(!saving, cx);
                });
                element = element.child(div().h(px(24.0)).child(control.entity.clone()));
            }
        }
        let picker = self.theme_settings.picker.clone();
        let control = picker.map_or_else(
            || div().w(px(180.0)).h(px(132.0)),
            |entity| div().child(entity.clone()),
        );
        let content = div()
            .flex()
            .items_start()
            .gap(px(10.0))
            .child(control)
            .child(
                div()
                    .flex_1()
                    .min_w(px(300.0))
                    .flex()
                    .flex_col()
                    .gap(px(5.0))
                    .child(
                        div()
                            .flex()
                            .justify_between()
                            .text_xs()
                            .child(theme::text(role.name()))
                            .child(theme::text(hex)),
                    )
                    .child(element)
                    .child(
                        div()
                            .text_xs()
                            .text_color(rgb(style.muted))
                            .child(theme::text("rgb")),
                    )
                    .child(controls),
            );
        let audio_rates = self.project_rate_settings_row(cx);
        let settings = self.audio_import_settings_row(cx);
        let track_defaults = self.track_defaults_row(cx);
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
            .children([audio_rates, settings, track_defaults])
            .child(self.controller_input_settings(cx));
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        {
            card = card.child(self.vst3_editor_settings_row(cx));
        }
        card = card
            .child(div().text_sm().child(theme::text("theme colors")))
            .child(rows)
            .child(content);
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
                .debug_selector(|| "theme settings backdrop".into())
                .occlude()
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
        hex_widgets::action(theme::text(label), theme::action_style())
            .id(id)
            .debug_selector(move || id.into())
            .h(px(22.0))
            .px(px(7.0))
            .border_1()
            .border_color(rgb(theme::color(PaletteRole::Border)))
            .text_xs()
            .on_click(cx.listener(move |view, _, window, cx| handler(view, window, cx)))
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
        size,
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
                let service = Service::new(48_000, 32, 8_000_000).expect("test service");
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
                        canonical_audio_source: None,
                        comparison_audio: None,
                        project: Arc::new(Project::new(48_000).expect("project")),
                        waveforms: Default::default(),
                        source_overviews: Default::default(),
                        audio_clip_waveforms: std::collections::BTreeMap::new(),
                        midi_preview: Arc::new(Default::default()),
                        gain_preview: Ok(Arc::new(hex_engine::GainAutomationPreview::default())),
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
        let deadline = crate::test_wait::hang_deadline();
        loop {
            cx.run_until_parked();
            if editor.read_with(cx, |view, _| {
                !view.theme_settings.loading && !view.audio_import_settings.is_loading()
            }) {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "settings did not load"
            );
            std::thread::yield_now();
        }
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
    fn sample_rate_selector_cancel_and_apply_use_the_current_project(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();

        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        click_bounds(&mut visual, "project rate 96000");
        click_bounds(&mut visual, "cancel theme changes");
        editor.read_with(&visual, |view, _| {
            assert!(
                !view.theme_settings.open,
                "cancel closes the settings panel"
            );
            assert_eq!(view.state.as_ref().unwrap().project.sample_rate(), 48_000);
            assert_eq!(view.worker_requests_pending, 0);
        });

        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        click_bounds(&mut visual, "apply project rate");
        editor.read_with(&visual, |view, _| {
            assert_eq!(
                view.worker_requests_pending, 0,
                "cancelled draft was discarded"
            );
        });
        click_bounds(&mut visual, "project rate 96000");
        click_bounds(&mut visual, "apply project rate");
        let deadline = crate::test_wait::hang_deadline();
        loop {
            let done = editor.update(&mut visual, |view, cx| {
                view.tick(cx);
                view.state.as_ref().unwrap().project.sample_rate() == 96_000
            });
            if done {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "sample rate did not apply"
            );
            std::thread::yield_now();
        }
        editor.read_with(&visual, |view, _| {
            let state = view.state.as_ref().unwrap();
            assert!(state.can_undo);
            assert!(!view.project_rate_settings.applying());
        });
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
        assert!(visual.debug_bounds("settings sample rates").is_some());

        let slider = visual
            .debug_bounds("theme-primary-r-slider")
            .expect("primary red slider");
        let start = point(
            slider.origin.x + slider.size.width / 2.0,
            slider.origin.y + slider.size.height / 2.0,
        );
        let end = point(start.x, start.y + px(12.0));
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
    fn settings_modal_blocks_the_underlying_ruler(cx: &mut TestAppContext) {
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        let backdrop = visual.debug_bounds("theme settings backdrop").unwrap();
        let ruler = visual.debug_bounds("timeline-ruler").unwrap();
        // Exercise modal occlusion even when added settings rows move the card
        // away from the ruler; the backdrop still owns all underlying input.
        let left = ruler.left().max(backdrop.left());
        let right = ruler.right().min(backdrop.right());
        let top = ruler.top().max(backdrop.top());
        let bottom = ruler.bottom().min(backdrop.bottom());
        assert!(
            left < right && top < bottom,
            "ruler and backdrop must overlap"
        );
        let overlap = point(left + px(1.0), (top + bottom) / 2.0);
        visual.simulate_mouse_down(overlap, MouseButton::Left, Modifiers::default());
        assert!(
            !editor.read_with(cx, |view, _| view.scrub_active()),
            "the modal surface blocks the underlying ruler"
        );
        visual.simulate_mouse_up(overlap, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert!(editor.read_with(cx, |view, _| view.loop_rail_gesture.is_none()));
    }

    #[gpui::test]
    fn selected_role_picker_drag_updates_hsb_rgb_preview_and_saved_preferences(
        cx: &mut TestAppContext,
    ) {
        let _restore = RestoreTheme(theme::preferences());
        let defaults = ThemePreferences::default();
        theme::apply_preferences(defaults);
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        visual.simulate_resize(size(px(880.0), px(540.0)));
        visual.run_until_parked();
        let panel = visual
            .debug_bounds("theme settings panel")
            .expect("settings panel at minimum editor size");
        let backdrop = visual
            .debug_bounds("theme settings backdrop")
            .expect("settings backdrop at minimum editor size");
        assert!(panel.origin.y >= backdrop.origin.y);
        assert!(panel.origin.y + panel.size.height <= backdrop.origin.y + backdrop.size.height);
        for selector in [
            "reset theme colors",
            "cancel theme changes",
            "save theme settings",
            "theme-role-background",
            "theme-role-text",
            "theme-role-primary",
            "theme-role-secondary",
            "theme-color-picker-field",
            "theme-color-picker-hue",
            "theme-hsb-h-slider",
            "theme-hsb-s-slider",
            "theme-hsb-b-slider",
            "theme-primary-r-slider",
        ] {
            let control = visual
                .debug_bounds(selector)
                .expect("settings action bounds");
            assert!(control.origin.x >= panel.origin.x);
            assert!(control.origin.x + control.size.width <= panel.origin.x + panel.size.width);
            assert!(control.origin.y >= panel.origin.y);
            assert!(control.origin.y + control.size.height <= panel.origin.y + panel.size.height);
            assert!(
                control.origin.y + control.size.height <= backdrop.origin.y + backdrop.size.height
            );
        }

        let center = visual
            .debug_bounds("theme-role-secondary")
            .unwrap()
            .center();
        visual.simulate_mouse_down(center, MouseButton::Left, Modifiers::default());
        assert!(
            !editor.read_with(cx, |view, _| view.scrub_active()),
            "modal clicks must not start an underlying seek"
        );
        assert!(
            editor.read_with(cx, |view, _| view.loop_rail_gesture.is_none()),
            "modal clicks must not capture the underlying loop rail"
        );
        visual.simulate_mouse_up(center, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        assert_eq!(
            editor.read_with(cx, |view, _| view.theme_settings.selected_role),
            ThemeRole::Secondary,
        );
        let field = visual
            .debug_bounds("theme-color-picker-field")
            .expect("saturation and brightness field");
        let start = point(
            field.origin.x + field.size.width / 4.0,
            field.origin.y + field.size.height / 2.0,
        );
        let outside = point(field.origin.x + field.size.width + px(32.0), field.origin.y);
        visual.simulate_mouse_down(start, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_move(outside, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(outside, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();

        editor.read_with(cx, |view, _| {
            let changed = view.theme_settings.draft.secondary;
            assert_ne!(changed, defaults.secondary);
            assert_eq!(theme::preferences().secondary, changed);
            let hsv = view.selected_hsv();
            assert_eq!(hsv.saturation, 100.0);
            assert_eq!(hsv.value, 100.0);
            assert_eq!(
                view.theme_settings.hsb_controls[&HsbChannel::Saturation]
                    .entity
                    .read_with(cx, |control, _| control.value()),
                100.0
            );
            assert_eq!(
                view.theme_settings.hsb_controls[&HsbChannel::Brightness]
                    .entity
                    .read_with(cx, |control, _| control.value()),
                100.0
            );
        });

        click_bounds(&mut visual, "reset theme colors");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.theme_settings.draft, defaults);
            assert_eq!(theme::preferences(), defaults);
        });

        let hue_value = visual
            .debug_bounds("theme-hsb-h-value")
            .expect("hue direct value field");
        let hue_click = point(
            hue_value.origin.x + hue_value.size.width / 2.0,
            hue_value.origin.y + hue_value.size.height / 2.0,
        );
        visual.simulate_mouse_down(hue_click, MouseButton::Left, Modifiers::default());
        visual.simulate_mouse_up(hue_click, MouseButton::Left, Modifiers::default());
        visual.simulate_input("120");
        visual.simulate_keystrokes("enter");
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.selected_hsv().hue, 120.0);
            let expected =
                hex_widgets::hsv_to_rgb(view.selected_hsv()).expect("valid selected hsv");
            let actual = view.theme_settings.draft.secondary;
            assert_eq!(
                actual,
                hex_preferences::RgbColor::new(expected.red, expected.green, expected.blue)
            );
            assert_ne!(actual, defaults.secondary);
        });

        click_bounds(&mut visual, "save theme settings");
        visual.run_until_parked();
        let reloaded = hex_preferences::load_theme_preferences(&directory.0.join("theme.json"))
            .expect("saved palette reloads");
        assert_eq!(reloaded.secondary, theme::preferences().secondary);
    }

    #[gpui::test]
    fn changing_picker_role_with_equal_rgb_retires_the_old_pointer(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        editor.update(cx, |view, cx| {
            view.select_theme_role(ThemeRole::Primary, cx)
        });
        visual.run_until_parked();
        let field = visual.debug_bounds("theme-color-picker-field").unwrap();
        visual.simulate_mouse_down(field.center(), MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        let retained = editor.update(cx, |view, cx| {
            let color = view.theme_settings.draft.primary;
            // Source identity changes even when its current value is identical.
            view.theme_settings.draft.secondary = color;
            view.select_theme_role(ThemeRole::Secondary, cx);
            color
        });
        visual.run_until_parked();
        let moved = field.origin + point(field.size.width * 0.8, field.size.height * 0.2);
        visual.simulate_mouse_move(moved, Some(MouseButton::Left), Modifiers::default());
        visual.simulate_mouse_up(moved, MouseButton::Left, Modifiers::default());
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.theme_settings.selected_role, ThemeRole::Secondary);
            assert_eq!(view.theme_settings.draft.primary, retained);
            assert_eq!(
                view.theme_settings.draft.secondary, retained,
                "an old pointer must not edit the newly selected role"
            );
        });
    }

    #[gpui::test]
    fn picker_contrast_error_keeps_last_legible_preview_until_cancel(cx: &mut TestAppContext) {
        let _restore = RestoreTheme(theme::preferences());
        let defaults = ThemePreferences::default();
        theme::apply_preferences(defaults);
        let directory = TestDirectory::new();
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();

        editor.update(cx, |view, cx| {
            view.select_theme_role(ThemeRole::Text, cx);
            let background = view.theme_settings.draft.background;
            let hsv = hex_widgets::rgb_to_hsv(
                RgbColor::new(background.red, background.green, background.blue),
                0.0,
            );
            let picker = view
                .theme_settings
                .picker
                .clone()
                .expect("shared color picker");
            picker.update(cx, |picker, cx| {
                assert!(picker.set_hsv(hsv, cx));
            });
        });
        visual.run_until_parked();
        editor.read_with(cx, |view, _| {
            assert_eq!(view.theme_settings.draft.text, defaults.background);
            assert_eq!(theme::preferences(), defaults);
            assert!(
                view.theme_settings
                    .error
                    .as_deref()
                    .is_some_and(|error| error.contains("4.5:1 contrast"))
            );
        });
        assert!(visual.debug_bounds("theme settings status").is_some());
        visual.simulate_keystrokes("escape");
        visual.run_until_parked();
        assert_eq!(theme::preferences(), defaults);
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

    #[gpui::test]
    fn auto_warp_setting_click_persists_reloads_and_save_refusal_keeps_active_policy(
        cx: &mut TestAppContext,
    ) {
        fn policy(
            editor: &gpui::Entity<EditorView>,
            cx: &TestAppContext,
        ) -> hex_project::AudioImportWarp {
            editor.read_with(cx, |view, _| {
                let request = view
                    .capture_audio_import_policy(hex_session::service::Request::Import(
                        PathBuf::from("/captured sample.wav"),
                    ))
                    .unwrap();
                match request {
                    hex_session::service::Request::Import(_)
                    | hex_session::service::Request::ImportToNewTrackWithGain { .. } => {
                        hex_project::AudioImportWarp::Off
                    }
                    hex_session::service::Request::ImportAudioBatch { options, .. } => options.warp,
                    _ => panic!("arrangement import did not retain its policy"),
                }
            })
        }
        let _restore = RestoreTheme(theme::preferences());
        theme::apply_preferences(ThemePreferences::default());
        let directory = TestDirectory::new();
        let path = directory.0.join("audio-import.json");
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.run_until_parked();
        assert_eq!(policy(&editor, cx), hex_project::AudioImportWarp::Off);
        assert!(
            !path.exists(),
            "loading defaults must not publish a settings file"
        );
        visual.simulate_keystrokes("cmd-,");
        visual.run_until_parked();
        assert!(visual.debug_bounds("theme settings panel").is_some());
        click_bounds(&mut visual, "auto warp imports");
        visual.run_until_parked();
        assert!(
            hex_preferences::load_audio_import_preferences(&path)
                .unwrap()
                .auto_warp_beats
        );
        assert_eq!(policy(&editor, cx), hex_project::AudioImportWarp::Beats);

        let (reopened_handle, reopened_editor) = editor_fixture(cx, &directory);
        let mut reopened = VisualTestContext::from_window(reopened_handle, cx);
        reopened.run_until_parked();
        assert_eq!(
            policy(&reopened_editor, cx),
            hex_project::AudioImportWarp::Beats
        );
        reopened.simulate_keystrokes("cmd-,");
        reopened.run_until_parked();
        // A directory at the independent preference-file path deterministically
        // refuses publication without changing the active import policy.
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        click_bounds(&mut reopened, "auto warp imports");
        reopened.run_until_parked();
        assert_eq!(
            policy(&reopened_editor, cx),
            hex_project::AudioImportWarp::Beats
        );
        assert!(path.is_dir());
        assert!(
            reopened
                .debug_bounds("audio import settings status")
                .is_some()
        );
        assert!(reopened.debug_bounds("theme settings panel").is_some());
    }

    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    #[gpui::test]
    fn vst3_space_setting_defaults_on_and_reopens_with_the_saved_choice(cx: &mut TestAppContext) {
        fn wait_for_choice(editor: &Entity<EditorView>, cx: &mut TestAppContext, expected: bool) {
            let deadline = crate::test_wait::hang_deadline();
            loop {
                cx.run_until_parked();
                if editor.read_with(cx, |view, cx| {
                    view.theme_settings.vst3_editor.visible_choice(cx) == Some(expected)
                }) {
                    break;
                }
                assert!(
                    std::time::Instant::now() < deadline,
                    "vst3 space setting did not settle"
                );
                std::thread::yield_now();
            }
        }

        let _restore = RestoreTheme(theme::preferences());
        let directory = TestDirectory::new();
        let path = directory.0.join("vst3-editor.json");
        let (handle, editor) = editor_fixture(cx, &directory);
        let mut visual = VisualTestContext::from_window(handle, cx);
        visual.simulate_keystrokes("cmd-,");
        wait_for_choice(&editor, cx, true);
        assert!(!path.exists());

        click_bounds(&mut visual, "vst3 space transport");
        wait_for_choice(&editor, cx, false);
        assert!(
            !hex_preferences::load_vst3_editor_preferences(&path)
                .unwrap()
                .spacebar_controls_transport
        );

        let (handle, reopened_editor) = editor_fixture(cx, &directory);
        let mut reopened = VisualTestContext::from_window(handle, cx);
        reopened.simulate_keystrokes("cmd-,");
        wait_for_choice(&reopened_editor, cx, false);
        click_bounds(&mut reopened, "vst3 space transport");
        wait_for_choice(&reopened_editor, cx, true);
        assert!(
            hex_preferences::load_vst3_editor_preferences(&path)
                .unwrap()
                .spacebar_controls_transport
        );
    }
}
