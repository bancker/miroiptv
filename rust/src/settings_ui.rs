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

/// The id of the overlay's Area. Everything it draws lies on the layer built
/// from it (see `overlay_layer`).
const AREA_ID: &str = "__settings__";

/// The overlay's own layer: the Area is built from it, and a focused widget
/// counts as the overlay's only when it lies on it.
fn overlay_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Foreground, egui::Id::new(AREA_ID))
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
    /// Keyboard focus is in the section's controls, not in the list. Enter,
    /// Right and a click on a row set it; so does any control that is not a
    /// list row getting the focus (Tab, a click into a text field).
    in_pane: bool,
    /// Focus the section's first control on the next frame.
    focus_first: bool,
    /// The list rows' widget ids from the last frame: they tell a focused row
    /// from a focused control.
    list_ids: Vec<egui::Id>,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn show(&mut self, ctx: &egui::Context, view: &SettingsView) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let lang = view.lang;
        // The mode follows the keyboard focus: a control of the overlay that
        // is not a list row having it puts the overlay in its section, whatever
        // gave it the focus (Tab, Enter, a click). Focus elsewhere never
        // counts: another panel's frame (a toast, the favorites list) is a Tab
        // stop too, and says nothing about the overlay.
        let focused = ctx.memory(|m| m.focused());
        if let Some(id) = focused {
            if !self.list_ids.contains(&id)
                && ctx
                    .read_response(id)
                    .is_some_and(|r| r.layer_id == overlay_layer())
            {
                self.in_pane = true;
            }
        }
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
                // On a focused row, that row's section opens.
                if let Some(row) =
                    focused.and_then(|id| self.list_ids.iter().position(|r| *r == id))
                {
                    self.section = row;
                }
                self.in_pane = true;
                self.focus_first = true;
            }
        });
        let width = (ctx.screen_rect().width() - 40.0).clamp(480.0, 760.0);
        let layer = overlay_layer();
        egui::Area::new(layer.id)
            .order(layer.order)
            // Its default click sense is a focusable widget: a first Tab stop
            // that shows nothing, and no list row or control.
            .sense(egui::Sense::hover())
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
                                self.list_ids.clear();
                                for (i, s) in Section::ALL.iter().enumerate() {
                                    let label =
                                        ui.selectable_label(i == self.section, t(lang, s.title()));
                                    self.list_ids.push(label.id);
                                    if label.clicked() {
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
/// language. Up/Down move the highlight and Enter picks it; after Tab, Enter
/// picks the button with the focus instead. A click picks too. Returns the
/// choice.
pub fn language_screen(ctx: &egui::Context, selected: &mut Lang) -> Option<Lang> {
    let mut picked = None;
    // A focused button gets Enter itself: its click picks its language.
    let nothing_focused = ctx.memory(|m| m.focused()).is_none();
    ctx.input_mut(|i| {
        let pos = Lang::ALL.iter().position(|l| l == selected).unwrap_or(0);
        if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
            *selected = Lang::ALL[(pos + 1).min(Lang::ALL.len() - 1)];
        }
        if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
            *selected = Lang::ALL[pos.saturating_sub(1)];
        }
        if nothing_focused && i.consume_key(Modifiers::NONE, Key::Enter) {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The view the overlay is drawn with. Built here only, so a field added
    /// to `SettingsView` is added in one place.
    fn make_view(settings: &Settings) -> SettingsView<'_> {
        SettingsView {
            lang: Lang::En,
            settings,
        }
    }

    /// One headless frame in which these keys are pressed.
    fn run_frame(ctx: &egui::Context, keys: &[Key], draw: impl FnOnce(&egui::Context)) {
        let press = |key: Key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::NONE,
        };
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events: keys.iter().map(|&key| press(key)).collect(),
            ..Default::default()
        };
        let _ = ctx.run(raw, draw);
    }

    /// Another panel on screen: a default Area in the corner where the app's
    /// toast sits. egui gives such an Area a click sense, so its frame is a
    /// focusable widget (a Tab stop) that is created before the overlay's own
    /// widgets. Nothing lies below it, so Down from there moves no focus.
    fn show_other_panel(ctx: &egui::Context) {
        egui::Area::new(egui::Id::new("__other__"))
            .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
            .show(ctx, |ui| {
                ui.label("x");
            });
    }

    /// The same kind of panel with a button; `focus` gives the button the
    /// keyboard focus.
    fn show_other_panel_with_button(ctx: &egui::Context, focus: bool) {
        egui::Area::new(egui::Id::new("__other__"))
            .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
            .show(ctx, |ui| {
                let button = ui.button("b");
                if focus {
                    button.request_focus();
                }
            });
    }

    /// The overlay on a headless egui context, one frame per call.
    struct Overlay {
        ctx: egui::Context,
        ui: SettingsUi,
        settings: Settings,
        /// Draw another default Area before the overlay in every frame.
        other_panel: bool,
    }

    impl Overlay {
        fn new() -> Self {
            Self::start(false)
        }

        /// With another default Area on screen in every frame, as while a
        /// toast shows.
        fn with_other_panel() -> Self {
            Self::start(true)
        }

        fn start(other_panel: bool) -> Self {
            let mut overlay = Overlay {
                ctx: egui::Context::default(),
                ui: SettingsUi::new(),
                settings: Settings::default(),
                other_panel,
            };
            // Two empty frames first, so the widgets exist before the first key.
            overlay.frame(&[]);
            overlay.frame(&[]);
            overlay
        }

        /// One frame with these keys pressed; what the overlay reports.
        fn frame(&mut self, keys: &[Key]) -> Vec<SettingsAction> {
            let mut actions = Vec::new();
            let view = make_view(&self.settings);
            let other_panel = self.other_panel;
            run_frame(&self.ctx, keys, |ctx| {
                if other_panel {
                    show_other_panel(ctx);
                }
                actions = self.ui.show(ctx, &view)
            });
            actions
        }

        /// `times` frames with the same key; none may report anything.
        fn press(&mut self, key: Key, times: usize) {
            for _ in 0..times {
                let actions = self.frame(&[key]);
                assert!(actions.is_empty(), "{key:?} reported {actions:?}");
            }
        }
    }

    /// The first-start language screen on a headless egui context.
    struct Screen {
        ctx: egui::Context,
        selected: Lang,
    }

    impl Screen {
        /// English is preselected, as on a machine with an English Windows.
        fn new() -> Self {
            let mut screen = Screen {
                ctx: egui::Context::default(),
                selected: Lang::En,
            };
            screen.frame(&[]);
            screen.frame(&[]);
            screen
        }

        fn frame(&mut self, keys: &[Key]) -> Option<Lang> {
            let mut picked = None;
            run_frame(&self.ctx, keys, |ctx| {
                picked = language_screen(ctx, &mut self.selected)
            });
            picked
        }
    }

    #[test]
    fn o_and_esc_close_the_overlay_from_the_list() {
        assert_eq!(Overlay::new().frame(&[Key::O]), vec![SettingsAction::Close]);
        assert_eq!(
            Overlay::new().frame(&[Key::Escape]),
            vec![SettingsAction::Close]
        );
    }

    #[test]
    fn up_and_down_choose_the_section_and_right_opens_it() {
        let mut o = Overlay::new();
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::ArrowUp, 1); // Controls, the third section
        o.press(Key::ArrowRight, 1); // into it: the checkbox has the focus
        assert_eq!(
            o.frame(&[Key::Space]),
            vec![SettingsAction::SetSwapZap(true)]
        );
    }

    #[test]
    fn enter_opens_the_language_section_on_its_first_radio() {
        let mut o = Overlay::new();
        o.press(Key::Enter, 1);
        // The first radio is Nederlands; the view's language is English.
        assert_eq!(
            o.frame(&[Key::Space]),
            vec![SettingsAction::SetLanguage(Lang::Nl)]
        );
    }

    #[test]
    fn in_a_section_o_is_just_a_letter_and_esc_goes_back_first() {
        let mut o = Overlay::new();
        o.press(Key::ArrowDown, 2);
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[Key::O]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn tabbing_through_the_list_rows_keeps_the_overlay_in_the_list() {
        // The first Tab goes to the first row (the overlay's own frame is not
        // a stop), and with a row focused `o` and Esc still close the overlay.
        let mut o = Overlay::new();
        o.press(Key::Tab, 2);
        assert_eq!(o.frame(&[Key::O]), vec![SettingsAction::Close]);
        let mut o = Overlay::new();
        o.press(Key::Tab, 2);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn tabbing_into_a_control_puts_the_overlay_in_its_section() {
        let mut o = Overlay::new();
        // The Controls section is shown; Tab walks its five list rows, then
        // lands on the checkbox.
        o.press(Key::ArrowDown, 2);
        o.press(Key::Tab, 6);
        // With the focus on a control `o` is just a letter, Esc goes back to
        // the list and only the next Esc closes.
        assert_eq!(o.frame(&[Key::O]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn enter_on_a_focused_list_row_opens_that_rows_section() {
        let mut o = Overlay::new();
        // Tab to the third row (Controls) while the pane still shows Language.
        o.press(Key::Tab, 3);
        o.press(Key::Enter, 1);
        // The Controls checkbox has the focus, not a Language radio.
        assert_eq!(
            o.frame(&[Key::Space]),
            vec![SettingsAction::SetSwapZap(true)]
        );
    }

    #[test]
    fn with_another_panel_visible_tab_then_o_closes_the_overlay() {
        // The first Tab lands on the other panel's frame, not on the overlay:
        // the overlay is still in its list, so `o` closes it.
        let mut o = Overlay::with_other_panel();
        o.press(Key::Tab, 1);
        assert_eq!(o.frame(&[Key::O]), vec![SettingsAction::Close]);
    }

    #[test]
    fn with_another_panel_visible_tab_then_esc_closes_the_overlay() {
        let mut o = Overlay::with_other_panel();
        o.press(Key::Tab, 1);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn with_another_panel_visible_the_arrows_still_choose_the_section() {
        let mut o = Overlay::with_other_panel();
        o.press(Key::Tab, 1);
        o.press(Key::ArrowDown, 2); // Controls
        o.press(Key::Enter, 1);
        assert_eq!(
            o.frame(&[Key::Space]),
            vec![SettingsAction::SetSwapZap(true)]
        );
    }

    #[test]
    fn an_overlay_opened_while_a_button_elsewhere_has_the_focus_starts_in_its_list() {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        // A button in another panel has the keyboard focus when the overlay opens.
        for _ in 0..2 {
            run_frame(&ctx, &[], |ctx| show_other_panel_with_button(ctx, true));
        }
        let mut overlay = SettingsUi::new();
        let mut actions = Vec::new();
        run_frame(&ctx, &[Key::O], |ctx| {
            show_other_panel_with_button(ctx, false);
            actions = overlay.show(ctx, &make_view(&settings));
        });
        assert_eq!(actions, vec![SettingsAction::Close]);
    }

    #[test]
    fn enter_on_the_language_screen_picks_the_selected_language_when_nothing_has_focus() {
        assert_eq!(Screen::new().frame(&[Key::Enter]), Some(Lang::En));
        let mut screen = Screen::new();
        assert_eq!(screen.frame(&[Key::ArrowDown]), None);
        assert_eq!(screen.selected, Lang::De);
        assert_eq!(screen.frame(&[Key::Enter]), Some(Lang::De));
    }

    #[test]
    fn enter_on_the_language_screen_picks_the_button_with_the_focus() {
        let mut screen = Screen::new();
        // Tab walks Nederlands, English, Deutsch; English stays preselected.
        for _ in 0..3 {
            assert_eq!(screen.frame(&[Key::Tab]), None);
        }
        assert_eq!(screen.frame(&[Key::Enter]), Some(Lang::De));
        assert_eq!(screen.selected, Lang::En);
    }
}
