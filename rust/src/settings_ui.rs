//! The settings overlay (key `o`) and the first-start language screen. The
//! overlay lists its sections on the left and shows the chosen one on the
//! right. Both draw themselves and report what the user did as actions;
//! app.rs carries those out.

use crate::i18n::{t, Lang, T};
use crate::settings::Settings;
use egui::{Color32, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Language,
    Portal,
    Controls,
    Adult,
    News,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Language,
        Section::Portal,
        Section::Controls,
        Section::Adult,
        Section::News,
    ];

    fn title(self) -> T {
        match self {
            Section::Language => T::SectionLanguage,
            Section::Portal => T::SectionPortal,
            Section::Controls => T::SectionControls,
            Section::Adult => T::SectionAdult,
            Section::News => T::SectionNews,
        }
    }
}

/// What the user did in the overlay.
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    Close,
    SetLanguage(Lang),
    SetSwapZap(bool),
}

/// What the overlay shows but app.rs owns.
pub struct SettingsView<'a> {
    pub lang: Lang,
    pub settings: &'a Settings,
}

/// The overlay's own state while it is open.
#[derive(Default)]
pub struct SettingsUi {
    section: usize,
    /// Keyboard focus is in the section's controls, not in the list.
    in_pane: bool,
    /// Focus the section's first control on the next frame.
    focus_first: bool,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn show(&mut self, ctx: &egui::Context, view: &SettingsView) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let lang = view.lang;
        // Taken before any widget sees them: in the list, arrows and Enter
        // belong to the overlay, not to a focused control.
        ctx.input_mut(|i| {
            if i.consume_key(Modifiers::NONE, Key::Escape) {
                if self.in_pane {
                    self.in_pane = false;
                } else {
                    actions.push(SettingsAction::Close);
                }
            }
            if self.in_pane {
                return;
            }
            if i.consume_key(Modifiers::NONE, Key::O) {
                actions.push(SettingsAction::Close);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                self.section = (self.section + 1).min(Section::ALL.len() - 1);
            }
            if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                self.section = self.section.saturating_sub(1);
            }
            if i.consume_key(Modifiers::NONE, Key::Enter)
                || i.consume_key(Modifiers::NONE, Key::ArrowRight)
            {
                self.in_pane = true;
                self.focus_first = true;
            }
        });
        let width = (ctx.screen_rect().width() - 40.0).clamp(480.0, 760.0);
        egui::Area::new(egui::Id::new("__settings__"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_rgb(22, 22, 28))
                    .inner_margin(egui::Margin::same(16.0))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.label(
                            egui::RichText::new(t(lang, T::SettingsTitle))
                                .heading()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(8.0);
                        let row = ui.horizontal_top(|ui| {
                            let list = ui.vertical(|ui| {
                                ui.set_width(170.0);
                                for (i, s) in Section::ALL.iter().enumerate() {
                                    if ui
                                        .selectable_label(i == self.section, t(lang, s.title()))
                                        .clicked()
                                    {
                                        self.section = i;
                                        self.in_pane = true;
                                        self.focus_first = true;
                                    }
                                }
                            });
                            // The gap a separator would take. Its line is drawn below, at the
                            // row's height: ui.separator() here would be as tall as the window.
                            ui.add_space(14.0);
                            ui.vertical(|ui| {
                                ui.set_width(width - 200.0);
                                let focus_first = std::mem::take(&mut self.focus_first);
                                match Section::ALL[self.section] {
                                    Section::Language => {
                                        language_pane(ui, view, focus_first, &mut actions)
                                    }
                                    Section::Controls => {
                                        controls_pane(ui, view, focus_first, &mut actions)
                                    }
                                    // Filled in by Tasks 15-17.
                                    Section::Portal | Section::Adult | Section::News => {}
                                }
                            });
                            list.response.rect.right() + 11.0
                        });
                        ui.painter().vline(
                            row.inner,
                            row.response.rect.y_range(),
                            ui.visuals().widgets.noninteractive.bg_stroke,
                        );
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(t(lang, T::SettingsNav))
                                .small()
                                .color(Color32::from_white_alpha(120)),
                        );
                    });
            });
        actions
    }
}

fn language_pane(
    ui: &mut egui::Ui,
    view: &SettingsView,
    focus_first: bool,
    actions: &mut Vec<SettingsAction>,
) {
    for (i, l) in Lang::ALL.iter().enumerate() {
        let r = ui.radio(view.lang == *l, l.native_name());
        if focus_first && i == 0 {
            r.request_focus();
        }
        if r.clicked() && view.lang != *l {
            actions.push(SettingsAction::SetLanguage(*l));
        }
    }
}

fn controls_pane(
    ui: &mut egui::Ui,
    view: &SettingsView,
    focus_first: bool,
    actions: &mut Vec<SettingsAction>,
) {
    let mut swap = view.settings.swap_zap_keys;
    let r = ui.checkbox(&mut swap, t(view.lang, T::SwapZapKeys));
    if focus_first {
        r.request_focus();
    }
    if r.changed() {
        actions.push(SettingsAction::SetSwapZap(swap));
    }
}

/// The first-start language choice: three big buttons, each in its own
/// language. Up/Down move, Enter or a click picks. Returns the choice.
pub fn language_screen(ctx: &egui::Context, selected: &mut Lang) -> Option<Lang> {
    let mut picked = None;
    ctx.input_mut(|i| {
        let pos = Lang::ALL.iter().position(|l| l == selected).unwrap_or(0);
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            *selected = Lang::ALL[(pos + 1).min(Lang::ALL.len() - 1)];
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            *selected = Lang::ALL[pos.saturating_sub(1)];
        }
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            picked = Some(*selected);
        }
    });
    egui::CentralPanel::default()
        .frame(egui::Frame::none().fill(Color32::from_rgb(14, 14, 18)))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(ui.available_height() * 0.25);
                ui.label(
                    egui::RichText::new("Taal  ·  Language  ·  Sprache")
                        .size(28.0)
                        .color(Color32::WHITE),
                );
                ui.add_space(24.0);
                for l in Lang::ALL {
                    let button = egui::Button::new(egui::RichText::new(l.native_name()).size(22.0))
                        .min_size(egui::vec2(260.0, 48.0))
                        .selected(l == *selected);
                    if ui.add(button).clicked() {
                        picked = Some(l);
                    }
                    ui.add_space(8.0);
                }
            });
        });
    picked
}
