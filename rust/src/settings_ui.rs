//! The settings overlay (key `o`) and the first-start language screen. The
//! overlay lists its sections on the left and shows the chosen one on the
//! right. Both draw themselves and report what the user did as actions;
//! app.rs carries those out.

use crate::args::XtreamCreds;
use crate::i18n::{t, tr, Lang, T};
use crate::pin::valid_pin;
use crate::portal::xtream::ConnCheck;
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
    /// Try these credentials (`user:pass@host:port`) without keeping them.
    TestConnection(String),
    /// Try these credentials (`user:pass@host:port`); keep and use them if
    /// they work.
    SavePortal(String),
    /// Show adult content if this is the PIN.
    Unlock(String),
    /// Choose the very first PIN (and unlock with it).
    SetPin(String),
    ChangePin {
        old: String,
        new: String,
    },
    /// Clear the PIN if this is the portal password.
    ResetPin(String),
    /// Hide adult content again.
    Lock,
}

/// What the overlay shows but app.rs owns.
pub struct SettingsView<'a> {
    pub lang: Lang,
    pub settings: &'a Settings,
    /// The portal credentials in use, if any: the portal form starts from them.
    pub creds: Option<&'a XtreamCreds>,
    /// How the connection test stands.
    pub conn: &'a ConnState,
    /// Adult content shows now (the catalog's state; the section keeps no copy).
    pub adult_unlocked: bool,
    pub has_pin: bool,
    /// Seconds left while PIN input is blocked after too many wrong tries.
    pub pin_wait_secs: Option<u64>,
    /// The last PIN outcome (wrong PIN, PIN changed, ...).
    pub adult_note: Option<T>,
}

/// The connection test's state, shown under the portal buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Testing,
    Done(ConnCheck),
    /// The fields cannot make credentials (e.g. a bad port).
    BadInput(T),
    /// The login worked but BongoPlayer.ini could not be written, so nothing was
    /// saved: where it should have gone and why not (the OS's words).
    WriteFailed {
        path: String,
        error: String,
    },
}

impl ConnState {
    /// The test or the save did not go through. Shown in red.
    pub fn is_failure(&self) -> bool {
        matches!(
            self,
            ConnState::Done(ConnCheck::BadLogin | ConnCheck::Unreachable | ConnCheck::Invalid)
                | ConnState::BadInput(_)
                | ConnState::WriteFailed { .. }
        )
    }
}

/// The status line under the portal buttons.
pub fn conn_text(lang: Lang, conn: &ConnState) -> Option<String> {
    Some(match conn {
        ConnState::Idle => return None,
        ConnState::Testing => t(lang, T::Testing).to_string(),
        ConnState::Done(ConnCheck::Ok {
            channels: Some(channels),
        }) => tr(lang, T::ConnOk, &[&channels.to_string()]),
        ConnState::Done(ConnCheck::Ok { channels: None }) => t(lang, T::ConnOkNoCount).to_string(),
        ConnState::Done(ConnCheck::BadLogin) => t(lang, T::ConnBadLogin).to_string(),
        ConnState::Done(ConnCheck::Unreachable) => t(lang, T::ConnUnreachable).to_string(),
        ConnState::Done(ConnCheck::Invalid) => t(lang, T::ConnInvalid).to_string(),
        ConnState::BadInput(key) => t(lang, *key).to_string(),
        ConnState::WriteFailed { path, error } => {
            tr(lang, T::CannotWrite, &[path.as_str(), error.as_str()])
        }
    })
}

/// What a failed test is shown in: the setup prompt's old error red.
const FAILURE_RED: Color32 = Color32::from_rgb(240, 130, 110);

/// The status line as label text: red when the test did not go through, else
/// the normal colour.
pub fn conn_rich_text(lang: Lang, conn: &ConnState) -> Option<egui::RichText> {
    let text = egui::RichText::new(conn_text(lang, conn)?);
    Some(if conn.is_failure() {
        text.color(FAILURE_RED)
    } else {
        text
    })
}

/// The portal fields while the section is open.
#[derive(Default)]
struct PortalForm {
    host: String,
    user: String,
    pass: String,
}

/// The PIN dialog that is open in the adult section, with its fields.
#[derive(Debug, Clone, Default)]
enum AdultDialog {
    #[default]
    Idle,
    Unlock {
        pin: String,
    },
    SetPin {
        first: String,
        second: String,
    },
    Change {
        old: String,
        first: String,
        second: String,
    },
    Forgot {
        password: String,
    },
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
    /// The portal form: made when the section is first shown, from the
    /// credentials in use, and kept while the overlay is open.
    portal: Option<PortalForm>,
    /// The PIN dialog of the adult section, if one is open.
    adult: AdultDialog,
    /// Focus the dialog's first field on the next frame (it was just opened).
    focus_dialog: bool,
    /// Why a PIN was turned down before it left the dialog (not 4 digits, not
    /// the same twice); what the app says about a PIN comes with the view.
    local_note: Option<T>,
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
                                    Section::Portal => {
                                        self.portal_pane(ui, view, focus_first, &mut actions)
                                    }
                                    Section::Controls => {
                                        controls_pane(ui, view, focus_first, &mut actions)
                                    }
                                    Section::Adult => {
                                        self.adult_pane(ui, view, focus_first, &mut actions)
                                    }
                                    // Filled in by Task 17.
                                    Section::News => {}
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

    /// Server, user and password, then the test and save buttons and how the
    /// test went. Test and Save hand app.rs the credentials as one string.
    fn portal_pane(
        &mut self,
        ui: &mut egui::Ui,
        view: &SettingsView,
        focus_first: bool,
        actions: &mut Vec<SettingsAction>,
    ) {
        let lang = view.lang;
        let form = self.portal.get_or_insert_with(|| match view.creds {
            Some(c) => PortalForm {
                host: format!("{}:{}", c.host, c.port),
                user: c.username.clone(),
                pass: c.password.clone(),
            },
            None => PortalForm::default(),
        });
        ui.label(t(lang, T::ServerLabel));
        let first = ui.add(
            egui::TextEdit::singleline(&mut form.host)
                .hint_text("server.example.com:8080")
                .desired_width(320.0),
        );
        if focus_first {
            first.request_focus();
        }
        ui.label(t(lang, T::UserLabel));
        ui.add(
            egui::TextEdit::singleline(&mut form.user)
                .hint_text(t(lang, T::UserHint))
                .desired_width(320.0),
        );
        ui.label(t(lang, T::PasswordLabel));
        ui.add(
            egui::TextEdit::singleline(&mut form.pass)
                .password(true)
                .desired_width(320.0),
        );
        let ready = !form.host.trim().is_empty()
            && !form.user.trim().is_empty()
            && !form.pass.is_empty()
            && *view.conn != ConnState::Testing;
        // Built when a button is pressed, not every frame.
        let creds = || format!("{}:{}@{}", form.user.trim(), form.pass, form.host.trim());
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            if ui
                .add_enabled(ready, egui::Button::new(t(lang, T::TestConnection)))
                .clicked()
            {
                actions.push(SettingsAction::TestConnection(creds()));
            }
            if ui
                .add_enabled(ready, egui::Button::new(t(lang, T::Save)))
                .clicked()
            {
                actions.push(SettingsAction::SavePortal(creds()));
            }
        });
        if let Some(text) = conn_rich_text(lang, view.conn) {
            ui.label(text);
        }
    }

    /// Whether adult content shows, with the buttons that change it or the PIN
    /// dialog one of them opened, then why the last PIN was turned down.
    fn adult_pane(
        &mut self,
        ui: &mut egui::Ui,
        view: &SettingsView,
        focus_first: bool,
        actions: &mut Vec<SettingsAction>,
    ) {
        let lang = view.lang;
        let status = if view.adult_unlocked {
            T::AdultVisible
        } else {
            T::AdultHidden
        };
        ui.label(egui::RichText::new(t(lang, status)).strong());
        ui.add_space(6.0);
        let blocked = view.pin_wait_secs.is_some();
        // A dialog just opened, or the section entered again with one open:
        // its first field takes the focus.
        let focus = std::mem::take(&mut self.focus_dialog) || focus_first;
        let mut dialog = std::mem::take(&mut self.adult);
        let mut open = None;
        let done = match &mut dialog {
            AdultDialog::Idle => {
                ui.horizontal(|ui| {
                    if view.adult_unlocked {
                        let r = ui.button(t(lang, T::Lock));
                        if focus_first {
                            r.request_focus();
                        }
                        if r.clicked() {
                            actions.push(SettingsAction::Lock);
                        }
                    } else {
                        let r = ui.add_enabled(!blocked, egui::Button::new(t(lang, T::Unlock)));
                        if focus_first {
                            r.request_focus();
                        }
                        if r.clicked() {
                            // The very first PIN is chosen, not entered.
                            open = Some(if view.has_pin {
                                AdultDialog::Unlock { pin: String::new() }
                            } else {
                                AdultDialog::SetPin {
                                    first: String::new(),
                                    second: String::new(),
                                }
                            });
                        }
                    }
                    if view.has_pin
                        && ui
                            .add_enabled(!blocked, egui::Button::new(t(lang, T::ChangePin)))
                            .clicked()
                    {
                        open = Some(AdultDialog::Change {
                            old: String::new(),
                            first: String::new(),
                            second: String::new(),
                        });
                    }
                    if view.has_pin
                        && !view.adult_unlocked
                        && ui.button(t(lang, T::ForgotPin)).clicked()
                    {
                        open = Some(AdultDialog::Forgot {
                            password: String::new(),
                        });
                    }
                });
                false
            }
            AdultDialog::Unlock { pin } => {
                let r = pin_field(ui, t(lang, T::EnterPin), pin, focus);
                let (ok, cancel) = ok_cancel(ui, lang, &r, !blocked);
                if ok {
                    if valid_pin(pin) {
                        actions.push(SettingsAction::Unlock(pin.clone()));
                    } else {
                        self.local_note = Some(T::PinFormat);
                    }
                }
                ok || cancel
            }
            AdultDialog::SetPin { first, second } => {
                pin_field(ui, t(lang, T::ChoosePin), first, focus);
                let r = pin_field(ui, t(lang, T::RepeatPin), second, false);
                let (ok, cancel) = ok_cancel(ui, lang, &r, true);
                if ok {
                    if !valid_pin(first) {
                        self.local_note = Some(T::PinFormat);
                    } else if first != second {
                        self.local_note = Some(T::PinMismatch);
                    } else {
                        actions.push(SettingsAction::SetPin(first.clone()));
                    }
                }
                ok || cancel
            }
            AdultDialog::Change { old, first, second } => {
                pin_field(ui, t(lang, T::CurrentPin), old, focus);
                pin_field(ui, t(lang, T::NewPin), first, false);
                let r = pin_field(ui, t(lang, T::RepeatPin), second, false);
                let (ok, cancel) = ok_cancel(ui, lang, &r, !blocked);
                if ok {
                    if !valid_pin(old) || !valid_pin(first) {
                        self.local_note = Some(T::PinFormat);
                    } else if first != second {
                        self.local_note = Some(T::PinMismatch);
                    } else {
                        actions.push(SettingsAction::ChangePin {
                            old: old.clone(),
                            new: first.clone(),
                        });
                    }
                }
                ok || cancel
            }
            AdultDialog::Forgot { password } => {
                ui.label(t(lang, T::PortalPassword));
                let r = ui.add(
                    egui::TextEdit::singleline(password)
                        .password(true)
                        .desired_width(220.0),
                );
                if focus {
                    r.request_focus();
                }
                let (ok, cancel) = ok_cancel(ui, lang, &r, !password.is_empty());
                if ok {
                    actions.push(SettingsAction::ResetPin(password.clone()));
                }
                ok || cancel
            }
        };
        if let Some(next) = open {
            self.adult = next;
            self.focus_dialog = true;
            self.local_note = None;
        } else if done {
            self.adult = AdultDialog::Idle;
            // Back on the buttons, so the keyboard can carry on.
            self.focus_first = true;
        } else {
            self.adult = dialog;
        }
        if let Some(secs) = view.pin_wait_secs {
            ui.label(tr(lang, T::PinBlocked, &[&secs.to_string()]));
        }
        if let Some(note) = self.local_note.or(view.adult_note) {
            ui.label(t(lang, note));
        }
    }
}

/// A masked PIN field: anything but digits is dropped as it is typed, and it
/// holds 4 at most.
fn pin_field(ui: &mut egui::Ui, label: &str, value: &mut String, focus: bool) -> egui::Response {
    ui.label(label);
    let r = ui.add(
        egui::TextEdit::singleline(value)
            .password(true)
            .desired_width(90.0),
    );
    value.retain(|c| c.is_ascii_digit());
    value.truncate(4);
    if focus {
        r.request_focus();
    }
    r
}

/// OK and Cancel under a dialog; Enter in its last field counts as OK.
/// Returns (OK, Cancel); OK is only given while `enabled`.
fn ok_cancel(ui: &mut egui::Ui, lang: Lang, last: &egui::Response, enabled: bool) -> (bool, bool) {
    let enter = last.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let (mut ok, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ok = ui
            .add_enabled(enabled, egui::Button::new(t(lang, T::Ok)))
            .clicked();
        cancel = ui.button(t(lang, T::Cancel)).clicked();
    });
    (enabled && (ok || enter), cancel)
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
            creds: None,
            conn: &ConnState::Idle,
            adult_unlocked: false,
            has_pin: false,
            pin_wait_secs: None,
            adult_note: None,
        }
    }

    /// A key press or release as egui gets it from the window.
    fn key_event(key: Key, pressed: bool) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Modifiers::NONE,
        }
    }

    /// One headless frame in which these keys are pressed.
    fn run_frame(ctx: &egui::Context, keys: &[Key], draw: impl FnOnce(&egui::Context)) {
        run_events(
            ctx,
            keys.iter().map(|&key| key_event(key, true)).collect(),
            draw,
        );
    }

    /// One headless frame in which these events arrive.
    fn run_events(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        draw: impl FnOnce(&egui::Context),
    ) {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            events,
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
        /// What app.rs would tell the adult section.
        adult_unlocked: bool,
        has_pin: bool,
        pin_wait_secs: Option<u64>,
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
                adult_unlocked: false,
                has_pin: false,
                pin_wait_secs: None,
            };
            // Two empty frames first, so the widgets exist before the first key.
            overlay.frame(&[]);
            overlay.frame(&[]);
            overlay
        }

        /// One frame with these keys pressed; what the overlay reports.
        fn frame(&mut self, keys: &[Key]) -> Vec<SettingsAction> {
            self.frame_with(keys.iter().map(|&key| key_event(key, true)).collect())
        }

        /// One frame in which this character is typed, as a keyboard makes it:
        /// the key press, the text it produces, the release.
        fn type_char(&mut self, key: Key, c: char) -> Vec<SettingsAction> {
            self.frame_with(vec![
                key_event(key, true),
                egui::Event::Text(c.to_string()),
                key_event(key, false),
            ])
        }

        /// One frame with these events; what the overlay reports.
        fn frame_with(&mut self, events: Vec<egui::Event>) -> Vec<SettingsAction> {
            let mut actions = Vec::new();
            let view = SettingsView {
                adult_unlocked: self.adult_unlocked,
                has_pin: self.has_pin,
                pin_wait_secs: self.pin_wait_secs,
                ..make_view(&self.settings)
            };
            let other_panel = self.other_panel;
            run_events(&self.ctx, events, |ctx| {
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

        /// `text` typed one character at a time, as a keyboard does; none of it
        /// may report anything.
        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                let key = Key::from_name(&c.to_string()).expect("a key for every character");
                let actions = self.type_char(key, c);
                assert!(actions.is_empty(), "typing {c:?} reported {actions:?}");
            }
        }

        /// Into the adult section and its first dialog: its first button opens
        /// it, and one more frame lets the dialog's first field take the focus.
        fn open_adult_dialog(&mut self) {
            self.press(Key::ArrowDown, 3); // Adult
            self.press(Key::Enter, 1); // into it: the first button has the focus
            self.press(Key::Enter, 1); // the button opens the dialog
            assert_eq!(self.frame(&[]), vec![]);
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

    #[test]
    fn the_status_line_says_how_the_test_went() {
        assert_eq!(conn_text(Lang::Nl, &ConnState::Idle), None);
        assert_eq!(
            conn_text(
                Lang::Nl,
                &ConnState::Done(ConnCheck::Ok {
                    channels: Some(13006)
                })
            )
            .as_deref(),
            Some("Verbonden: 13006 zenders")
        );
        // A good login whose channel list did not come: connected all the same.
        let no_count = ConnState::Done(ConnCheck::Ok { channels: None });
        assert_eq!(conn_text(Lang::Nl, &no_count).as_deref(), Some("Verbonden"));
        assert_eq!(conn_text(Lang::En, &no_count).as_deref(), Some("Connected"));
        assert_eq!(conn_text(Lang::De, &no_count).as_deref(), Some("Verbunden"));
        assert_eq!(
            conn_text(Lang::En, &ConnState::Done(ConnCheck::BadLogin)).as_deref(),
            Some("Wrong username or password")
        );
        assert_eq!(
            conn_text(Lang::De, &ConnState::BadInput(T::CredsBadPort)).as_deref(),
            Some("ungültige Portnummer")
        );
    }

    #[test]
    fn a_failed_write_names_the_file_and_the_reason_in_the_language_in_use() {
        let failed = ConnState::WriteFailed {
            path: r"C:\Apps\BongoPlayer.ini".into(),
            error: "Access is denied. (os error 5)".into(),
        };
        assert_eq!(
            conn_text(Lang::En, &failed).as_deref(),
            Some(r"could not write C:\Apps\BongoPlayer.ini: Access is denied. (os error 5)")
        );
        assert_eq!(
            conn_text(Lang::Nl, &failed).as_deref(),
            Some(r"kon C:\Apps\BongoPlayer.ini niet schrijven: Access is denied. (os error 5)")
        );
        assert_eq!(
            conn_text(Lang::De, &failed).as_deref(),
            Some(
                r"C:\Apps\BongoPlayer.ini konnte nicht geschrieben werden: Access is denied. (os error 5)"
            )
        );
    }

    #[test]
    fn only_a_test_that_went_wrong_is_a_failure() {
        let failures = [
            ConnState::Done(ConnCheck::BadLogin),
            ConnState::Done(ConnCheck::Unreachable),
            ConnState::Done(ConnCheck::Invalid),
            ConnState::BadInput(T::CredsBadPort),
            ConnState::WriteFailed {
                path: "BongoPlayer.ini".into(),
                error: "disk full".into(),
            },
        ];
        for state in failures {
            assert!(state.is_failure(), "{state:?}");
        }
        let fine = [
            ConnState::Idle,
            ConnState::Testing,
            ConnState::Done(ConnCheck::Ok { channels: Some(2) }),
            ConnState::Done(ConnCheck::Ok { channels: None }),
        ];
        for state in fine {
            assert!(!state.is_failure(), "{state:?}");
        }
    }

    /// What pressing Test and then Save on the keyboard gives, for this portal
    /// form and test state: into the Portal pane, Tab from the server field to
    /// the buttons, Enter on each. A disabled button is no Tab stop and takes
    /// no Enter.
    fn press_test_and_save(form: PortalForm, conn: &ConnState) -> Vec<SettingsAction> {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let view = SettingsView {
            conn,
            ..make_view(&settings)
        };
        let mut ui = SettingsUi::new();
        ui.portal = Some(form);
        let mut frame = |keys: &[Key]| {
            let mut actions = Vec::new();
            run_frame(&ctx, keys, |ctx| actions = ui.show(ctx, &view));
            actions
        };
        frame(&[]);
        frame(&[]);
        frame(&[Key::ArrowDown]); // Portal
        frame(&[Key::Enter]); // into it: the server field has the focus
        let mut pressed = Vec::new();
        for _ in 0..3 {
            frame(&[Key::Tab]); // the user, the password, then Test
        }
        pressed.extend(frame(&[Key::Enter]));
        frame(&[Key::Tab]); // Save
        pressed.extend(frame(&[Key::Enter]));
        pressed
    }

    #[test]
    fn test_and_save_are_disabled_while_testing_and_while_a_field_is_empty() {
        let form = |host: &str, user: &str, pass: &str| PortalForm {
            host: host.into(),
            user: user.into(),
            pass: pass.into(),
        };
        // The control: filled in, nothing running, both buttons work.
        let one_string = "bob:secret@srv.example.net:8080".to_string();
        assert_eq!(
            press_test_and_save(
                form("srv.example.net:8080", "bob", "secret"),
                &ConnState::Idle
            ),
            vec![
                SettingsAction::TestConnection(one_string.clone()),
                SettingsAction::SavePortal(one_string)
            ]
        );
        // A test is still running.
        let filled = form("srv.example.net:8080", "bob", "secret");
        assert_eq!(press_test_and_save(filled, &ConnState::Testing), vec![]);
        // One field empty; the server and the user also when they hold only blanks.
        let one_missing = [
            ("server", form("", "bob", "secret")),
            ("blank server", form("  ", "bob", "secret")),
            ("user", form("srv.example.net:8080", "", "secret")),
            ("blank user", form("srv.example.net:8080", "  ", "secret")),
            ("password", form("srv.example.net:8080", "bob", "")),
        ];
        for (what, incomplete) in one_missing {
            assert_eq!(
                press_test_and_save(incomplete, &ConnState::Idle),
                vec![],
                "{what}"
            );
        }
    }

    #[test]
    fn keys_typed_into_the_server_field_stay_in_the_field() {
        let mut o = Overlay::new();
        o.press(Key::ArrowDown, 1); // Portal
        o.press(Key::Enter, 1); // into it: the server field has the focus
        for (key, c) in [
            (Key::O, 'o'),
            (Key::Num1, '1'),
            (Key::Space, ' '),
            (Key::F, 'f'),
        ] {
            let actions = o.type_char(key, c);
            assert!(actions.is_empty(), "typing {c:?} reported {actions:?}");
        }
        let host = o.ui.portal.as_ref().map(|form| form.host.as_str());
        assert_eq!(host, Some("o1 f"));
    }

    #[test]
    fn test_and_save_hand_back_the_credentials_in_use_as_one_string() {
        // '@', '=' and ':' in the password come back exactly as they went in.
        let creds = XtreamCreds {
            username: "bob".into(),
            password: "p@ss=w:rd".into(),
            host: "srv.example.net".into(),
            port: 8080,
        };
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let view = SettingsView {
            creds: Some(&creds),
            ..make_view(&settings)
        };
        let mut ui = SettingsUi::new();
        let mut frame = |keys: &[Key]| {
            let mut actions = Vec::new();
            run_frame(&ctx, keys, |ctx| actions = ui.show(ctx, &view));
            actions
        };
        frame(&[]);
        frame(&[]);
        frame(&[Key::ArrowDown]); // Portal
        frame(&[Key::Enter]); // into it: the server field has the focus
        for _ in 0..3 {
            frame(&[Key::Tab]); // the user, the password, then Test
        }
        let one_string = "bob:p@ss=w:rd@srv.example.net:8080";
        assert_eq!(
            frame(&[Key::Enter]),
            vec![SettingsAction::TestConnection(one_string.into())]
        );
        frame(&[Key::Tab]); // Save
        assert_eq!(
            frame(&[Key::Enter]),
            vec![SettingsAction::SavePortal(one_string.into())]
        );
        assert_eq!(crate::args::parse_xtream_creds(one_string).unwrap(), creds);
    }

    /// The text in the open dialog's first field.
    fn first_field(ui: &SettingsUi) -> &str {
        match &ui.adult {
            AdultDialog::Unlock { pin } => pin,
            AdultDialog::SetPin { first, .. } => first,
            AdultDialog::Change { old, .. } => old,
            AdultDialog::Forgot { password } => password,
            AdultDialog::Idle => panic!("no dialog is open"),
        }
    }

    #[test]
    fn keys_typed_into_the_pin_fields_stay_in_the_fields() {
        // Both dialogs that take a PIN: choosing the first one, and entering it.
        for has_pin in [false, true] {
            let mut o = Overlay::new();
            o.has_pin = has_pin;
            o.open_adult_dialog();
            for (key, c) in [
                (Key::Num1, '1'),
                (Key::O, 'o'),
                (Key::Num2, '2'),
                (Key::Space, ' '),
                (Key::F, 'f'),
                (Key::Num3, '3'),
                (Key::Num4, '4'),
                (Key::Num5, '5'),
            ] {
                let actions = o.type_char(key, c);
                assert!(
                    actions.is_empty(),
                    "typing {c:?} reported {actions:?}, has_pin: {has_pin}"
                );
            }
            // Letters and the space are dropped; four digits are all it keeps.
            assert_eq!(first_field(&o.ui), "1234", "has_pin: {has_pin}");
            // The overlay is still there: Esc leaves the field, the next one closes.
            assert_eq!(o.frame(&[Key::Escape]), vec![]);
            assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
        }
    }

    #[test]
    fn coming_back_to_an_open_dialog_puts_the_focus_in_its_first_field() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        assert_eq!(o.frame(&[Key::Escape]), vec![]); // back to the list
        o.press(Key::Enter, 1); // into the section again
        o.type_text("34");
        assert_eq!(first_field(&o.ui), "1234");
    }

    #[test]
    fn the_unlock_dialog_hands_over_four_digits_and_nothing_else() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("123");
        // Too short: nothing leaves the dialog and the user is told why.
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert_eq!(o.ui.local_note, Some(T::PinFormat));
        // Once more, with four digits.
        assert_eq!(o.frame(&[]), vec![]); // the first button has the focus again
        o.press(Key::Enter, 1); // it opens the dialog
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("1234".into())]
        );
        assert_eq!(o.ui.local_note, None);
    }

    #[test]
    fn a_pin_typed_and_confirmed_within_one_frame_is_handed_over() {
        // A fast typist on a slow frame: the digits and the Enter arrive together.
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        let mut events: Vec<egui::Event> = "4321"
            .chars()
            .map(|c| egui::Event::Text(c.to_string()))
            .collect();
        events.push(key_event(Key::Enter, true));
        assert_eq!(
            o.frame_with(events),
            vec![SettingsAction::Unlock("4321".into())]
        );
    }

    #[test]
    fn choosing_the_first_pin_asks_for_it_twice() {
        let mut o = Overlay::new();
        o.open_adult_dialog(); // no PIN yet: the dialog chooses one
        o.type_text("1234");
        o.press(Key::Tab, 1); // the repeat
        o.type_text("1235");
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert_eq!(o.ui.local_note, Some(T::PinMismatch));
        // Again, and this time twice the same.
        assert_eq!(o.frame(&[]), vec![]);
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        o.press(Key::Tab, 1);
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SetPin("1234".into())]
        );
    }

    #[test]
    fn while_unlocked_the_first_button_locks() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.adult_unlocked = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Lock has the focus
        assert_eq!(o.frame(&[Key::Enter]), vec![SettingsAction::Lock]);
    }

    #[test]
    fn no_dialog_opens_while_pin_input_is_blocked() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.pin_wait_secs = Some(30);
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it
        o.press(Key::Enter, 1); // Unlock is disabled
        o.press(Key::Tab, 1);
        o.press(Key::Enter, 1); // so is Change PIN
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
    }

    #[test]
    fn changing_the_pin_hands_over_the_old_and_the_new_one() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Unlock has the focus
        o.press(Key::Tab, 1); // Change PIN
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        o.press(Key::Tab, 1);
        o.type_text("5678");
        o.press(Key::Tab, 1);
        o.type_text("5678");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ChangePin {
                old: "1234".into(),
                new: "5678".into()
            }]
        );
    }

    #[test]
    fn a_forgotten_pin_is_reset_with_the_portal_password() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Unlock has the focus
        o.press(Key::Tab, 2); // Change PIN, then Forgot PIN?
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        // A password takes any character, `o` and the space included.
        o.type_text("foo bar");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ResetPin("foo bar".into())]
        );
    }
}
