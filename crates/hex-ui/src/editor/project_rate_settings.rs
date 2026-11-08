//! A local settings draft. The Session worker owns the applied mode, effective
//! rate and history. The current output owns the nominal rate readout.
use super::{
    EditorView,
    pending_edit::{Disposition, Receipt},
};
use crate::theme;
use gpui::{Context, Div, div, prelude::*, px, rgb};
use hex_project::SampleRateMode;
use hex_session::service::{Event, ProjectSampleRate, Request};

const CHOICES: [ProjectSampleRate; 7] = [
    ProjectSampleRate::FollowInterface,
    ProjectSampleRate::Fixed(44_100),
    ProjectSampleRate::Fixed(48_000),
    ProjectSampleRate::Fixed(88_200),
    ProjectSampleRate::Fixed(96_000),
    ProjectSampleRate::Fixed(176_400),
    ProjectSampleRate::Fixed(192_000),
];

#[derive(Default)]
pub(super) struct State {
    draft: Option<ProjectSampleRate>,
    pending: Option<Pending>,
    error: Option<String>,
}

struct Pending {
    generation: u64,
    revision: u64,
    choice: ProjectSampleRate,
    receipt: Receipt,
}

impl State {
    pub(super) fn applying(&self) -> bool {
        self.pending.is_some()
    }

    pub(super) fn clear_draft(&mut self) {
        self.draft = None;
        self.error = None;
    }

    pub(super) fn matches(&self, request: &Request) -> bool {
        self.pending.as_ref().is_some_and(|pending| {
            pending.receipt.is_queued() && matches!(
                request,
                Request::ChangeProjectSampleRate { project_generation, revision, choice }
                if (*project_generation, *revision, *choice) == (pending.generation, pending.revision, pending.choice)
            )
        })
    }

    pub(super) fn submitted(&mut self, count: usize) {
        if let Some(pending) = &mut self.pending {
            pending.receipt.submitted(count);
        }
    }

    pub(super) fn refused(&mut self, reason: String) {
        self.pending = None;
        self.error = Some(reason);
    }

    pub(super) fn observe(&mut self, generation: u64, event: &Event) {
        if self.pending.as_mut().is_some_and(|pending| {
            pending
                .receipt
                .observe(generation, event.project_replaced, event.request_complete)
                != Disposition::Waiting
        }) {
            self.pending = None;
            self.error = event.error.as_deref().map(str::to_owned);
            if self.error.is_none() {
                self.draft = None;
            }
        } else if event.project_replaced {
            self.clear_draft();
        }
    }
}

impl EditorView {
    fn rate_choices(&self) -> Option<(ProjectSampleRate, ProjectSampleRate, u32)> {
        let project = &self.state.as_ref()?.project;
        let rate = project.sample_rate();
        let current = match project.sample_rate_mode() {
            SampleRateMode::Fixed => ProjectSampleRate::Fixed(rate),
            SampleRateMode::FollowInterface => ProjectSampleRate::FollowInterface,
        };
        Some((
            current,
            self.project_rate_settings.draft.unwrap_or(current),
            rate,
        ))
    }

    fn interface_rate(&self) -> Option<u32> {
        self.audio.as_ref()?.status()?.interface_sample_rate()
    }

    fn rate_can_apply(&self) -> bool {
        !self.project_rate_settings.applying()
            && self.rate_choices().is_some_and(|(current, draft, rate)| {
                current != draft
                    || (draft == ProjectSampleRate::FollowInterface
                        && self
                            .interface_rate()
                            .is_some_and(|interface| interface != rate))
            })
    }

    fn apply_project_rate_settings(&mut self, cx: &mut Context<Self>) {
        if !self.rate_can_apply() {
            return;
        }
        let (Some((_, choice, _)), Some(state)) = (self.rate_choices(), &self.state) else {
            return;
        };
        if self.edit_pending() {
            self.project_rate_settings.error =
                Some("finish the current edit before applying a sample rate".into());
            cx.notify();
            return;
        }
        let generation = self.project_generation;
        let revision = state.revision;
        self.project_rate_settings.error = None;
        self.project_rate_settings.pending = Some(Pending {
            generation,
            revision,
            choice,
            receipt: Receipt::new(generation),
        });
        if !self.enqueue(Request::ChangeProjectSampleRate {
            project_generation: generation,
            revision,
            choice,
        }) {
            let reason = self
                .error
                .as_deref()
                .unwrap_or("sample rate request refused")
                .to_owned();
            self.project_rate_settings.refused(reason);
        }
        cx.notify();
    }

    pub(super) fn project_rate_settings_row(&self, cx: &mut Context<Self>) -> Div {
        let choices = self.rate_choices();
        let rate_label = |rate: Option<u32>| {
            rate.map_or_else(|| "unavailable".into(), |rate| format!("{rate} hz"))
        };
        let rates = div()
            .flex()
            .gap(px(16.0))
            .text_xs()
            .child(theme::text(format!(
                "project rate · {}",
                rate_label(choices.map(|(_, _, rate)| rate))
            )))
            .child(theme::text(format!(
                "interface rate · {}",
                rate_label(self.interface_rate())
            )));
        let style = theme::control_style();
        let applying = self.project_rate_settings.applying();
        let available = choices.is_some() && !applying;
        let selectors = CHOICES.into_iter().map(|choice| {
            let label = match choice {
                ProjectSampleRate::FollowInterface => "follow interface".into(),
                ProjectSampleRate::Fixed(rate) => rate.to_string(),
            };
            let selected = choices.is_some_and(|(_, draft, _)| draft == choice);
            let id = format!("project rate {label}");
            hex_widgets::action(theme::text(label), theme::action_style())
                .id(gpui::SharedString::from(id.clone()))
                .debug_selector(move || id.clone())
                .border_1()
                .border_color(rgb(if selected { style.accent } else { style.border }))
                .px(px(4.0))
                .text_xs()
                .opacity(if available { 1.0 } else { 0.45 })
                .on_click(cx.listener(move |view, _, _, cx| {
                    if view.rate_choices().is_some() && !view.project_rate_settings.applying() {
                        view.project_rate_settings.draft = Some(choice);
                        view.project_rate_settings.error = None;
                        cx.notify();
                    }
                }))
        });
        let controls = div()
            .flex()
            .items_center()
            .gap(px(4.0))
            .children(selectors)
            .child(
                hex_widgets::action(
                    theme::text(if applying {
                        "applying…"
                    } else {
                        "apply rate"
                    }),
                    theme::action_style(),
                )
                .id("apply project rate")
                .debug_selector(|| "apply project rate".into())
                .px(px(5.0))
                .text_xs()
                .opacity(if self.rate_can_apply() { 1.0 } else { 0.45 })
                .on_click(cx.listener(|view, _, _, cx| view.apply_project_rate_settings(cx))),
            );
        div()
            .debug_selector(|| "settings sample rates".into())
            .flex()
            .flex_col()
            .gap(px(3.0))
            .children([rates, controls])
            .when_some(
                self.project_rate_settings.error.as_deref(),
                |row, reason| row.child(div().text_xs().child(theme::text(reason))),
            )
    }
}
