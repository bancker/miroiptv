//! The settings overlay (key `o`) and the first-start language screen. The
//! overlay lists its sections on the left and shows the chosen one on the
//! right. Both draw themselves and report what the user did as actions;
//! app.rs carries those out.

use crate::args::{credentials_line, parse_xtream_creds, without_newlines, XtreamCreds};
use crate::i18n::{t, tr, Lang, T};
use crate::news::{default_rules, NewsRule};
use crate::pin::valid_pin;
use crate::portal::xtream::ConnCheck;
use crate::settings::Settings;
use crate::sources::{is_web_address, Editing, SourceDraft, SourceId, SourceKind, Sources};
use egui::{Color32, Key, Modifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Language,
    Sources,
    Controls,
    Adult,
    News,
}

impl Section {
    pub const ALL: [Section; 5] = [
        Section::Language,
        Section::Sources,
        Section::Controls,
        Section::Adult,
        Section::News,
    ];

    fn title(self) -> T {
        match self {
            Section::Language => T::SectionLanguage,
            Section::Sources => T::SectionSources,
            Section::Controls => T::SectionControls,
            Section::Adult => T::SectionAdult,
            Section::News => T::SectionNews,
        }
    }
}

/// The id of the overlay's Area. Everything it draws lies on the layer built
/// from it (see `overlay_layer`).
const AREA_ID: &str = "__settings__";

/// The ids of the sources section's name, server and address fields: the
/// keyboard is sent to them by id (see `SettingsUi::sources_pane`).
const SOURCE_NAME_ID: &str = "__source_name__";
const SOURCE_SERVER_ID: &str = "__source_server__";
const SOURCE_ADDRESS_ID: &str = "__source_address__";
const SOURCE_USER_ID: &str = "__source_user__";
const SOURCE_PASS_ID: &str = "__source_pass__";

/// The id of a text field of the form of `editing`. egui keeps a field's cursor
/// and undo history under its id, so the id is the source's: no source gets
/// another's (Ctrl+Z in the password of one must not bring back what was typed
/// in another's). A new source has no number yet, and gets one of its own
/// (`seq`, see `SourceForm::blank`).
fn field_id(field: &str, editing: Editing, seq: u32) -> egui::Id {
    let source = match editing {
        Editing::New => (0u8, seq),
        Editing::CommandLine => (1, 0),
        Editing::Saved(id) => (2, id),
    };
    egui::Id::new((field, source))
}

/// Counts the forms made: the `seq` of a form (see `field_id`).
static FORMS_MADE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

/// Drops what egui kept of the text fields of this form: their cursors and undo
/// histories.
fn forget_fields(ctx: &egui::Context, form: &SourceForm) {
    let fields = [
        SOURCE_NAME_ID,
        SOURCE_SERVER_ID,
        SOURCE_ADDRESS_ID,
        SOURCE_USER_ID,
        SOURCE_PASS_ID,
    ];
    for field in fields {
        let id = form.field_id(field);
        ctx.data_mut(|data| data.remove::<egui::text_edit::TextEditState>(id));
    }
}

/// Room the overlay keeps between its frame and the window's edge.
const OVERLAY_SCREEN_MARGIN: f32 = 12.0;
/// Space between the overlay's frame and what is in it.
const OVERLAY_FRAME_MARGIN: f32 = 16.0;
/// The gap above the key hints at the bottom of the overlay: the layout leaves
/// it and the height cap reserves it.
const OVERLAY_HINT_GAP: f32 = 8.0;

// The columns of the news rules, in points. The patterns box takes what is left
// of the row.
const NEWS_CHANNEL_WIDTH: f32 = 110.0;
const NEWS_LABEL_WIDTH: f32 = 50.0;
const NEWS_REMOVE_WIDTH: f32 = 20.0;
/// The patterns box is never narrower than this, whatever the window.
const NEWS_PATTERNS_MIN_WIDTH: f32 = 60.0;
/// The lines the patterns hint may take under the rules: one, or two when a
/// narrow pane wraps it. The rules leave room for both.
const NEWS_HINT_LINES: f32 = 2.0;
/// The gaps in that tail: before the buttons, before the hint and after it.
const NEWS_TAIL_GAPS: f32 = 3.0;
/// The rules never get less height than this, whatever the window.
const NEWS_MIN_ROWS_HEIGHT: f32 = 60.0;
/// The scroll marker's width, the room it takes beside the view it belongs to
/// (the marker lies at the far end of it), and the least it is long.
const SCROLL_MARKER_WIDTH: f32 = 3.0;
const SCROLL_MARKER_ROOM: f32 = 6.0;
const SCROLL_MARKER_MIN_LENGTH: f32 = 12.0;

/// The overlay's width in a window this wide: the window less 20 pt on each
/// side, between 480 and 760.
fn overlay_width(window_width: f32) -> f32 {
    (window_width - 40.0).clamp(480.0, 760.0)
}

/// The overlay's own layer: the Area is built from it, and a focused widget
/// counts as the overlay's only when it lies on it.
fn overlay_layer() -> egui::LayerId {
    egui::LayerId::new(egui::Order::Foreground, egui::Id::new(AREA_ID))
}

/// What the user did in the overlay. `Debug` for the tests only: a PIN, a
/// password or a source's address rides in some of them.
#[derive(Clone, PartialEq)]
#[cfg_attr(test, derive(Debug))]
pub enum SettingsAction {
    Close,
    SetLanguage(Lang),
    SetSwapZap(bool),
    /// Try these credentials (`user:pass@host:port`) without keeping them.
    TestConnection(String),
    /// Keep a source (see `SourceDraft`): an Xtream one is tested first and
    /// kept only if it logs in.
    SaveSource(SourceDraft),
    /// Put this saved source in use.
    ActivateSource(SourceId),
    /// Remove this saved source, with its favorites, presets, defect marks and
    /// saved list.
    DeleteSource(SourceId),
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
    /// The news rules as they stand now: sent with every change, so a typed
    /// letter is one of these.
    SetNewsRules(Vec<NewsRule>),
}

/// What the overlay shows but app.rs owns.
pub struct SettingsView<'a> {
    pub lang: Lang,
    pub settings: &'a Settings,
    /// The portal credentials in use, if any: the portal form starts from them.
    pub creds: Option<&'a XtreamCreds>,
    /// They came from `--xtream`, which beats the ini on every start: the portal
    /// section says that a Save only counts when the app starts without it.
    pub creds_from_command_line: bool,
    /// The saved sources.
    pub sources: &'a Sources,
    /// The one in use: `None` while the portal of `--xtream` runs (then
    /// `creds` are its credentials).
    pub active_source: Option<SourceId>,
    /// How the connection test stands.
    pub conn: &'a ConnState,
    /// Adult content shows now (the catalog's state; the section keeps no copy).
    pub adult_unlocked: bool,
    pub has_pin: bool,
    /// Seconds left while PIN input is blocked after too many wrong tries.
    pub pin_wait_secs: Option<u64>,
    /// The last PIN outcome (wrong PIN, PIN changed, ...).
    pub adult_note: Option<T>,
    /// The news rules in force; the news section starts from them.
    pub news_rules: &'a [NewsRule],
    /// Per news rule, the channel it points at, if any (the catalog's word).
    pub news_found: &'a [Option<String>],
}

/// The connection test's state, shown under the portal buttons.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConnState {
    Idle,
    Testing,
    Done(ConnCheck),
    /// The fields cannot make credentials (e.g. a bad port).
    BadInput(T),
    /// The login worked but BongoIPTVPlayer.ini could not be written, so nothing was
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

/// A note the user should not miss, in the colour the help gives its keys.
const NOTE_AMBER: Color32 = Color32::from_rgb(230, 220, 110);

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

/// The portal fields while the section is open. `Debug` for the tests only:
/// it would print the password.
#[derive(Clone, Default)]
#[cfg_attr(test, derive(Debug))]
struct PortalForm {
    host: String,
    user: String,
    pass: String,
}

impl PortalForm {
    fn of_creds(c: &XtreamCreds) -> Self {
        PortalForm {
            host: format!("{}:{}", c.host, c.port),
            user: c.username.clone(),
            pass: c.password.clone(),
        }
    }

    /// The fields of a saved credentials line; all of it in the server field
    /// when it is no such line (a hand-edited ini).
    fn of_line(line: &str) -> Self {
        match parse_xtream_creds(line) {
            Ok(c) => Self::of_creds(&c),
            Err(_) => PortalForm {
                host: line.to_owned(),
                ..PortalForm::default()
            },
        }
    }
}

/// The Sources section's form while the overlay is open: the source it shows,
/// and what is typed in it. `Debug` for the tests only: it would print the
/// password, an M3U address and a credentials line.
#[derive(Clone)]
#[cfg_attr(test, derive(Debug))]
struct SourceForm {
    editing: Editing,
    /// Tells the fields of two forms of new sources apart (see `field_id`).
    seq: u32,
    kind: SourceKind,
    name: String,
    /// The Xtream fields.
    portal: PortalForm,
    /// The M3U list's URL or file.
    address: String,
    /// Why the last Save went nowhere: no name, no usable address.
    problem: Option<T>,
    /// The M3U address is an Xtream portal's link: the credentials line the
    /// question offers to save as an Xtream source instead. It is the question
    /// about the address as it stands: an edit of the address ends it.
    xtream_offer: Option<String>,
    /// This form asked for the connection test whose verdict (testing,
    /// connected, wrong login, unreachable) the status line shows: Test, or Save
    /// of an Xtream source. The status line is the app's one, so no other form
    /// shows it, and the answer to a Save is shown only while this form is.
    owns_verdict: bool,
    /// Delete was pressed once; the second press deletes.
    confirm_delete: bool,
    /// The keyboard goes to the form's first control next frame (the kind of a
    /// new source, else the name).
    focus_form: bool,
    /// The keyboard goes to the server or address field next frame.
    focus_address: bool,
}

/// What the Sources section shows the next time it draws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NextForm {
    Source(Editing),
    /// The source an Xtream Save kept, with the verdict of its test.
    Kept(Editing),
    /// A new M3U list, the keyboard in its address field.
    NewList,
}

impl SourceForm {
    /// What this form's text field `field` is known by (see `field_id`).
    fn field_id(&self, field: &str) -> egui::Id {
        field_id(field, self.editing, self.seq)
    }

    /// An empty form for `editing`, named after its kind's default.
    fn blank(editing: Editing, kind: SourceKind, lang: Lang) -> Self {
        SourceForm {
            editing,
            seq: FORMS_MADE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            kind,
            name: kind.default_name(lang).to_owned(),
            portal: PortalForm::default(),
            address: String::new(),
            problem: None,
            xtream_offer: None,
            owns_verdict: false,
            confirm_delete: false,
            focus_form: false,
            focus_address: false,
        }
    }

    /// The form of what `editing` names, as the view has it. A saved source
    /// that is gone gives a new one.
    fn of(editing: Editing, view: &SettingsView) -> Self {
        let lang = view.lang;
        match editing {
            Editing::New => Self::blank(Editing::New, SourceKind::Xtream, lang),
            Editing::CommandLine => SourceForm {
                // Save keeps the portal as the first saved one: under its name.
                name: view.sources.first_xtream().map_or_else(
                    || SourceKind::Xtream.default_name(lang).to_owned(),
                    |s| s.display_name(lang),
                ),
                portal: view.creds.map(PortalForm::of_creds).unwrap_or_default(),
                ..Self::blank(Editing::CommandLine, SourceKind::Xtream, lang)
            },
            Editing::Saved(id) => match view.sources.get(id) {
                Some(s) => {
                    let mut form = Self::blank(editing, s.kind, lang);
                    form.name = s.display_name(lang);
                    match s.kind {
                        SourceKind::Xtream => form.portal = PortalForm::of_line(&s.address),
                        SourceKind::M3u => form.address = s.address.clone(),
                    }
                    form
                }
                None => Self::blank(Editing::New, SourceKind::Xtream, lang),
            },
        }
    }

    /// The form the section opens with: the portal of `--xtream` when it runs,
    /// else the source in use, else the first saved one, else a new one,
    /// filled in from the credentials in use if there are any.
    fn first(view: &SettingsView) -> Self {
        if view.creds_from_command_line {
            return Self::of(Editing::CommandLine, view);
        }
        let saved = view
            .active_source
            .filter(|id| view.sources.get(*id).is_some())
            .or_else(|| view.sources.list().first().map(|s| s.id));
        match saved {
            Some(id) => Self::of(Editing::Saved(id), view),
            None => SourceForm {
                portal: view.creds.map(PortalForm::of_creds).unwrap_or_default(),
                ..Self::blank(Editing::New, SourceKind::Xtream, view.lang)
            },
        }
    }

    /// What Save hands the app, or why it cannot: no name, or for a list an
    /// address that is no http(s) URL or file that exists.
    fn draft(&self) -> Result<SourceDraft, T> {
        let name = without_newlines(&self.name).trim().to_owned();
        if name.is_empty() {
            return Err(T::SourceNameMissing);
        }
        let address = match self.kind {
            SourceKind::Xtream => {
                credentials_line(&self.portal.user, &self.portal.pass, &self.portal.host)
            }
            SourceKind::M3u => {
                let address = typed_address(&self.address);
                if is_web_address(&address) {
                    address
                } else if std::path::Path::new(&address).is_file() {
                    absolute(address)
                } else {
                    return Err(T::SourceAddressInvalid);
                }
            }
        };
        Ok(SourceDraft {
            editing: self.editing,
            kind: self.kind,
            name,
            address,
        })
    }

    /// Save: the draft, or why not. On a list whose address is an Xtream
    /// portal's link the question comes first, unless the list was saved with
    /// that address (the user kept it as a list, and a rename needs no question).
    fn save(&mut self, actions: &mut Vec<SettingsAction>, saved: &Sources) {
        self.problem = None;
        self.xtream_offer = None;
        if self.kind == SourceKind::M3u && !self.address_is_as_saved(saved) {
            if let Some(line) = crate::m3u::xtream_link(&typed_address(&self.address)) {
                self.xtream_offer = Some(line);
                return;
            }
        }
        self.push_draft(actions);
    }

    /// The form shows a saved source whose address is the one typed in it.
    fn address_is_as_saved(&self, saved: &Sources) -> bool {
        match self.editing {
            Editing::Saved(id) => saved
                .get(id)
                .is_some_and(|s| s.address == typed_address(&self.address)),
            _ => false,
        }
    }

    fn push_draft(&mut self, actions: &mut Vec<SettingsAction>) {
        match self.draft() {
            Ok(draft) => {
                // A test of the login comes of it: its verdict is this form's.
                self.owns_verdict |= draft.kind == SourceKind::Xtream;
                actions.push(SettingsAction::SaveSource(draft));
            }
            Err(problem) => self.problem = Some(problem),
        }
    }
}

/// An M3U address as typed or pasted, without line breaks and the spaces around
/// it, and without the one pair of quotes Explorer's "Copy as path" puts around
/// a path.
fn typed_address(address: &str) -> String {
    let address = without_newlines(address);
    let address = address.trim();
    let unquoted = address
        .strip_prefix('"')
        .and_then(|a| a.strip_suffix('"'))
        .unwrap_or(address);
    unquoted.to_owned()
}

/// The path of a file as an absolute one. A relative path means the folder the
/// app runs in just now, which the next start may not be run in.
fn absolute(path: String) -> String {
    if std::path::Path::new(&path).is_absolute() {
        return path;
    }
    std::path::absolute(&path).map_or(path, |p| p.to_string_lossy().into_owned())
}

/// The PIN dialog that is open in the adult section, with its fields. `Debug`
/// for the tests only: it holds the PINs and the portal password as typed.
#[derive(Clone, Default)]
#[cfg_attr(test, derive(Debug))]
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

impl AdultDialog {
    /// The same dialog with nothing typed in it.
    fn empty(&mut self) {
        match self {
            AdultDialog::Idle => {}
            AdultDialog::Unlock { pin } => pin.clear(),
            AdultDialog::SetPin { first, second } => {
                first.clear();
                second.clear();
            }
            AdultDialog::Change { old, first, second } => {
                old.clear();
                first.clear();
                second.clear();
            }
            AdultDialog::Forgot { password } => password.clear(),
        }
    }
}

/// The PIN action a dialog sent to the app, whose answer the next frame's view
/// shows: it decides whether the dialog is done or stays for another try.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PinAction {
    Unlock,
    SetPin,
    ChangePin,
    ResetPin,
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
    /// The sources form: made when the section is first shown, kept while the
    /// overlay is open.
    sources: Option<SourceForm>,
    /// What the section shows the next time it draws (the app asks after a
    /// save or a switch; the setup prompt's link asks for a new list).
    show_next: Option<NextForm>,
    /// The form of this source is going to be shown with the keyboard on its
    /// name: `settle_keyboard` gives it now (see there).
    name_due: Option<Editing>,
    /// The PIN dialog of the adult section, if one is open.
    adult: AdultDialog,
    /// The field of the dialog (0 is the first) that takes the focus on the next
    /// frame: the dialog was just opened, or a try left it empty.
    focus_dialog: Option<usize>,
    /// Why a PIN was turned down before it left the dialog (not 4 digits, not
    /// the same twice); what the app says about a PIN comes with the view.
    local_note: Option<T>,
    /// The PIN action sent last frame: the app has answered it by now, and the
    /// view says how (see `settle_pin`).
    pin_sent: Option<PinAction>,
    /// The news rules being edited: made when the section is first shown, from
    /// the rules in force, and kept while the overlay is open.
    news: Option<Vec<NewsRule>>,
    /// The rule whose channel field takes the focus on the next frame (a rule
    /// just added, or the one that took a removed rule's place).
    news_focus: Option<usize>,
}

impl SettingsUi {
    pub fn new() -> Self {
        Self::default()
    }

    /// The News section is the one shown. Whatever the app works out only for
    /// it (which channel each rule found) can wait until then.
    pub fn shows_news(&self) -> bool {
        Section::ALL[self.section] == Section::News
    }

    /// The sources section shows this source next, the keyboard on the first
    /// control of its form (its name; the Xtream kind of a new source) when the
    /// user is in the section, not in the list of sections. The app's answer to
    /// a list saved, a source used or deleted, in the frame of that action: it
    /// calls `settle_keyboard` when it has dealt with the frame's actions.
    pub fn show_source(&mut self, editing: Editing) {
        self.show_next = Some(NextForm::Source(editing));
        self.name_due = self.keyboard_goes_to_name_of(editing);
    }

    /// The same for the source an Xtream Save kept, once the login has been
    /// tried, which can be long after. The form that asked takes it, with the
    /// verdict, if it is the one still on show. A form the user moved on to
    /// (another row, New source) stays as it is, and so does the keyboard: the
    /// source is kept all the same and the list shows it. An answer with a form
    /// change on its way already is the user's move as well.
    pub fn show_kept_source(&mut self, editing: Editing) {
        let asked = self.sources.as_ref().is_some_and(|form| form.owns_verdict);
        if asked && self.show_next.is_none() {
            self.show_next = Some(NextForm::Kept(editing));
            self.name_due = self.keyboard_goes_to_name_of(editing);
        }
    }

    /// Whether the verdict of a connection test shows: the sources section is
    /// on show, with the Xtream form that asked for the test (`owns_verdict`),
    /// no problem of its own in the verdict's place, and no other form on its
    /// way. The app says in a toast what does not show.
    pub fn shows_verdict(&self) -> bool {
        Section::ALL[self.section] == Section::Sources
            && self.show_next.is_none()
            && self.sources.as_ref().is_some_and(|form| {
                form.kind == SourceKind::Xtream && form.owns_verdict && form.problem.is_none()
            })
    }

    /// Whether the keyboard goes to the name of the form of `editing` when it
    /// is shown: the user is in the sources section, and the form starts with a
    /// name (a new source starts with its kind).
    fn keyboard_goes_to_name_of(&self, editing: Editing) -> Option<Editing> {
        let in_section = self.in_pane && Section::ALL[self.section] == Section::Sources;
        (in_section && editing != Editing::New).then_some(editing)
    }

    /// The keyboard goes to the name of the source `show_source` asked for, now.
    /// The app calls it in the frame it gave its answer in, after the section
    /// has drawn that frame. In egui 0.27 a Tab gives the focus to the widget
    /// after the one that has it, as that one is drawn; a text field that only
    /// just got the focus takes the Tab itself. With the focus on the name from
    /// this frame on, a Tab in the very next frame, which is when the form is
    /// drawn, moves on from the name to the next stop.
    pub fn settle_keyboard(&mut self, ctx: &egui::Context) {
        if let Some(editing) = self.name_due.take() {
            ctx.memory_mut(|m| m.request_focus(field_id(SOURCE_NAME_ID, editing, 0)));
        }
    }

    /// Opens on the sources section with a new M3U list in the form, the
    /// keyboard in its address field: the setup prompt's link.
    pub fn open_new_list(&mut self) {
        self.section = Section::ALL
            .iter()
            .position(|s| *s == Section::Sources)
            .unwrap_or(0);
        self.in_pane = true;
        self.show_next = Some(NextForm::NewList);
    }

    /// The kind and the address of the source the form shows, if it is open.
    pub fn form_address(&self) -> Option<(SourceKind, String)> {
        self.sources.as_ref().map(|f| match f.kind {
            SourceKind::Xtream => (f.kind, f.portal.host.clone()),
            SourceKind::M3u => (f.kind, f.address.clone()),
        })
    }

    pub fn show(&mut self, ctx: &egui::Context, view: &SettingsView) -> Vec<SettingsAction> {
        let mut actions = Vec::new();
        let lang = view.lang;
        if let Some(sent) = self.pin_sent.take() {
            self.settle_pin(sent, view);
        }
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
                if self.pin_dialog_open() {
                    // Esc backs out one level at a time: an open PIN dialog
                    // first, with what was typed in it, then the section.
                    self.adult = AdultDialog::Idle;
                    self.local_note = None;
                    self.focus_first = true;
                } else if self.in_pane {
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
        let width = overlay_width(ctx.screen_rect().width());
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
                    .inner_margin(egui::Margin::same(OVERLAY_FRAME_MARGIN))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.label(
                            egui::RichText::new(t(lang, T::SettingsTitle))
                                .heading()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(8.0);
                        // The overlay never grows taller than its window: a section
                        // gets the height that is left after the window's edge, the
                        // frame's margins, the title above and the key hints below,
                        // and scrolls what does not fit in it.
                        let above = ui.cursor().min.y - ui.min_rect().min.y;
                        let hints = OVERLAY_HINT_GAP
                            + 2.0 * ui.spacing().item_spacing.y
                            + ui.text_style_height(&egui::TextStyle::Small);
                        let section_height = ctx.screen_rect().height()
                            - 2.0 * (OVERLAY_SCREEN_MARGIN + OVERLAY_FRAME_MARGIN)
                            - above
                            - hints;
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
                                    Section::Sources => self.sources_pane(
                                        ui,
                                        view,
                                        focus_first,
                                        &mut actions,
                                        section_height,
                                    ),
                                    Section::Controls => {
                                        controls_pane(ui, view, focus_first, &mut actions)
                                    }
                                    Section::Adult => {
                                        self.adult_pane(ui, view, focus_first, &mut actions)
                                    }
                                    Section::News => self.news_pane(
                                        ui,
                                        view,
                                        focus_first,
                                        &mut actions,
                                        section_height,
                                    ),
                                }
                            });
                            list.response.rect.right() + 11.0
                        });
                        ui.painter().vline(
                            row.inner,
                            row.response.rect.y_range(),
                            ui.visuals().widgets.noninteractive.bg_stroke,
                        );
                        ui.add_space(OVERLAY_HINT_GAP);
                        ui.label(
                            egui::RichText::new(t(lang, T::SettingsNav))
                                .small()
                                .color(Color32::from_white_alpha(120)),
                        );
                    });
            });
        actions
    }

    /// The sources: the form of the chosen one (the one in use at first), then
    /// the list to choose another, use it, add one or delete one. Test hands
    /// app.rs the credentials as one string; Save hands it the source.
    ///
    /// Where the keyboard goes when the form changes (`SourceForm::focus_form`
    /// and `focus_address`, `focus_first`) is settled so that a Tab pressed
    /// right after moves on from there, in whichever frame it comes. In egui
    /// 0.27 a Tab gives the focus to the widget after the one that has it, as
    /// that one is drawn. A text field is drawn twice (its box, its text) and
    /// takes that Tab itself when it only just got the focus. So the field a
    /// click or Enter makes the first of the form (the name, the server) gets
    /// the focus then, by its id, though it is not drawn yet. Any other
    /// control (the Xtream radio) gets it in the frame after, just before it
    /// is drawn.
    fn sources_pane(
        &mut self,
        ui: &mut egui::Ui,
        view: &SettingsView,
        focus_first: bool,
        actions: &mut Vec<SettingsAction>,
        max_height: f32,
    ) {
        let lang = view.lang;
        // A source the app asks for takes the keyboard only from someone who
        // is in the form: not from the list of sections, e.g. when a test of
        // credentials ends while the user is looking at another section.
        let in_form = self.in_pane;
        let next = self.show_next.take().map(|next| match next {
            NextForm::Source(editing) => SourceForm {
                focus_form: in_form,
                ..SourceForm::of(editing, view)
            },
            NextForm::Kept(editing) => SourceForm {
                focus_form: in_form,
                owns_verdict: true,
                ..SourceForm::of(editing, view)
            },
            NextForm::NewList => SourceForm {
                focus_address: true,
                ..SourceForm::blank(Editing::New, SourceKind::M3u, lang)
            },
        });
        // A form of a source that is gone (deleted) starts again from the view.
        let gone = matches!(
            self.sources.as_ref().map(|f| f.editing),
            Some(Editing::Saved(id)) if view.sources.get(id).is_none()
        );
        if gone {
            // The next source added may get its number: it gets none of its fields'
            // cursors and undo history.
            if let Some(old) = self.sources.as_ref() {
                forget_fields(ui.ctx(), old);
            }
        }
        if let Some(form) = next {
            self.sources = Some(form);
        } else if gone || self.sources.is_none() {
            self.sources = Some(SourceForm::first(view));
        }
        let form = self.sources.as_mut().expect("made above");
        let scrolled = marker_scroll_area("__sources__")
            .max_height(max_height)
            .auto_shrink([false, true])
            // Tab jumps to a row; no gliding there.
            .animated(false)
            .show(ui, |ui| {
                // Both taken every frame: a request left standing would pull the
                // focus back here in the next one. The form's first control
                // wins over the address field. The overlay's first frame is a
                // sizing pass in which everything is disabled and drops the
                // focus it is given: a request waits for the frame after it.
                let (to_form, to_address) = if ui.is_enabled() {
                    let form_flag = std::mem::take(&mut form.focus_form);
                    let address_flag = std::mem::take(&mut form.focus_address);
                    (form_flag, (address_flag || focus_first) && !form_flag)
                } else {
                    (false, false)
                };
                let name_id = form.field_id(SOURCE_NAME_ID);
                ui.label(t(lang, T::SourceNameLabel));
                if to_form && form.editing != Editing::New {
                    ui.memory_mut(|m| m.request_focus(name_id));
                }
                let name = ui.add(
                    egui::TextEdit::singleline(&mut form.name)
                        .id(name_id)
                        .desired_width(320.0),
                );
                reveal_on_focus(&name);
                if to_form && form.editing != Editing::New {
                    name.scroll_to_me(None);
                }
                if form.editing == Editing::New {
                    let before = form.kind;
                    let (xtream, m3u) = ui
                        .horizontal(|ui| {
                            let xtream_id = ui.next_auto_id();
                            if to_form {
                                ui.memory_mut(|m| m.request_focus(xtream_id));
                            }
                            let x = ui.radio_value(
                                &mut form.kind,
                                SourceKind::Xtream,
                                t(lang, T::KindXtream),
                            );
                            let m = ui.radio_value(
                                &mut form.kind,
                                SourceKind::M3u,
                                t(lang, T::KindM3u),
                            );
                            (x, m)
                        })
                        .inner;
                    reveal_on_focus(&xtream);
                    reveal_on_focus(&m3u);
                    if to_form {
                        xtream.scroll_to_me(None);
                    }
                    if form.kind != before {
                        // A name that is still the old kind's default (in
                        // whichever language the form was made) follows the kind.
                        if before.is_default_name(&form.name) {
                            form.name = form.kind.default_name(lang).to_owned();
                        }
                        form.problem = None;
                        form.xtream_offer = None;
                    }
                }
                let address_id = form.field_id(match form.kind {
                    SourceKind::Xtream => SOURCE_SERVER_ID,
                    SourceKind::M3u => SOURCE_ADDRESS_ID,
                });
                let (user_id, pass_id) =
                    (form.field_id(SOURCE_USER_ID), form.field_id(SOURCE_PASS_ID));
                let address_field = match form.kind {
                    SourceKind::Xtream => {
                        ui.label(t(lang, T::ServerLabel));
                        if to_address {
                            ui.memory_mut(|m| m.request_focus(address_id));
                        }
                        let server = ui.add(
                            egui::TextEdit::singleline(&mut form.portal.host)
                                .id(address_id)
                                .hint_text("server.example.com:8080")
                                .desired_width(320.0),
                        );
                        ui.label(t(lang, T::UserLabel));
                        let user = ui.add(
                            egui::TextEdit::singleline(&mut form.portal.user)
                                .id(user_id)
                                .hint_text(t(lang, T::UserHint))
                                .desired_width(320.0),
                        );
                        ui.label(t(lang, T::PasswordLabel));
                        let pass = ui.add(
                            egui::TextEdit::singleline(&mut form.portal.pass)
                                .id(pass_id)
                                .password(true)
                                .desired_width(320.0),
                        );
                        reveal_on_focus(&user);
                        reveal_on_focus(&pass);
                        server
                    }
                    SourceKind::M3u => {
                        ui.label(t(lang, T::SourceAddressLabel));
                        if to_address {
                            ui.memory_mut(|m| m.request_focus(address_id));
                        }
                        ui.add(
                            egui::TextEdit::singleline(&mut form.address)
                                .id(address_id)
                                .hint_text(t(lang, T::SourceAddressHint))
                                .desired_width(320.0),
                        )
                    }
                };
                reveal_on_focus(&address_field);
                if to_address {
                    address_field.scroll_to_me(None);
                }
                // The question was about the address as it was: an edit ends it.
                if address_field.changed() {
                    form.xtream_offer = None;
                }
                if view.creds_from_command_line && form.editing == Editing::CommandLine {
                    ui.add_space(4.0);
                    ui.label(
                        egui::RichText::new(t(lang, T::PortalFromCommandLine))
                            .small()
                            .color(NOTE_AMBER),
                    );
                }
                ui.add_space(6.0);
                let testing = *view.conn == ConnState::Testing;
                ui.horizontal(|ui| match form.kind {
                    SourceKind::Xtream => {
                        let filled = !form.portal.host.trim().is_empty()
                            && !form.portal.user.trim().is_empty()
                            && !form.portal.pass.is_empty();
                        let test =
                            action_button(ui, filled && !testing, t(lang, T::TestConnection));
                        reveal_on_focus(&test);
                        if test.clicked() {
                            let p = &form.portal;
                            actions.push(SettingsAction::TestConnection(credentials_line(
                                &p.user, &p.pass, &p.host,
                            )));
                            form.owns_verdict = true;
                        }
                        let save = action_button(ui, filled && !testing, t(lang, T::Save));
                        reveal_on_focus(&save);
                        if save.clicked() {
                            form.save(actions, view.sources);
                        }
                    }
                    SourceKind::M3u => {
                        let save =
                            action_button(ui, !form.address.trim().is_empty(), t(lang, T::Save));
                        reveal_on_focus(&save);
                        if save.clicked() {
                            form.save(actions, view.sources);
                        }
                    }
                });
                if let Some(problem) = form.problem {
                    ui.label(egui::RichText::new(t(lang, problem)).color(FAILURE_RED));
                } else if form.kind == SourceKind::Xtream && form.owns_verdict {
                    if let Some(text) = conn_rich_text(lang, view.conn) {
                        ui.label(text);
                    }
                }
                if let Some(line) = form.xtream_offer.clone() {
                    ui.label(egui::RichText::new(t(lang, T::XtreamLinkFound)).color(NOTE_AMBER));
                    ui.horizontal(|ui| {
                        let as_xtream = ui.button(t(lang, T::UseAsXtream));
                        let as_list = ui.button(t(lang, T::KeepAsM3u));
                        reveal_on_focus(&as_xtream);
                        reveal_on_focus(&as_list);
                        if as_xtream.clicked() {
                            if SourceKind::M3u.is_default_name(&form.name) {
                                form.name = SourceKind::Xtream.default_name(lang).to_owned();
                            }
                            form.kind = SourceKind::Xtream;
                            // A saved list stays a list, as it was saved: the
                            // portal is a source of its own.
                            form.editing = Editing::New;
                            form.portal = PortalForm::of_line(&line);
                            form.address.clear();
                            form.xtream_offer = None;
                            form.focus_address = true;
                            let server = form.field_id(SOURCE_SERVER_ID);
                            ui.memory_mut(|m| m.request_focus(server));
                        }
                        if as_list.clicked() {
                            form.xtream_offer = None;
                            form.push_draft(actions);
                        }
                    });
                }
                ui.add_space(8.0);
                ui.separator();
                ui.label(egui::RichText::new(t(lang, T::SourcesListTitle)).strong());
                let in_use = t(lang, T::SourceActiveMark);
                let mut chosen = None;
                if view.creds_from_command_line {
                    let text = format!("{} · {in_use}", t(lang, T::CommandLineSource));
                    let row = ui.selectable_label(form.editing == Editing::CommandLine, text);
                    reveal_on_focus(&row);
                    if row.clicked() {
                        chosen = Some(Editing::CommandLine);
                    }
                }
                for s in view.sources.list() {
                    let kind = match s.kind {
                        SourceKind::Xtream => t(lang, T::KindXtream),
                        SourceKind::M3u => t(lang, T::KindM3u),
                    };
                    let mut text = format!("{} · {kind}", s.display_name(lang));
                    if view.active_source == Some(s.id) {
                        text.push_str(&format!(" · {in_use}"));
                    }
                    let row = ui.selectable_label(form.editing == Editing::Saved(s.id), text);
                    reveal_on_focus(&row);
                    if row.clicked() {
                        chosen = Some(Editing::Saved(s.id));
                    }
                }
                // Use and Delete act on the saved source the form shows, never
                // on the one in use.
                let other = match form.editing {
                    Editing::Saved(id) if view.active_source != Some(id) => Some(id),
                    _ => None,
                };
                ui.horizontal(|ui| {
                    let use_it = action_button(ui, other.is_some(), t(lang, T::ButtonUseSource));
                    reveal_on_focus(&use_it);
                    if use_it.clicked() {
                        if let Some(id) = other {
                            actions.push(SettingsAction::ActivateSource(id));
                        }
                    }
                    let new = ui.button(t(lang, T::ButtonNewSource));
                    reveal_on_focus(&new);
                    if new.clicked() {
                        chosen = Some(Editing::New);
                    }
                    let label = if form.confirm_delete {
                        T::ButtonConfirmDelete
                    } else {
                        T::ButtonDeleteSource
                    };
                    let delete = action_button(ui, other.is_some(), t(lang, label));
                    reveal_on_focus(&delete);
                    if delete.clicked() {
                        match other {
                            Some(id) if form.confirm_delete => {
                                actions.push(SettingsAction::DeleteSource(id));
                                form.confirm_delete = false;
                            }
                            Some(_) => form.confirm_delete = true,
                            None => {}
                        }
                    }
                });
                chosen
            });
        paint_scroll_marker(ui, &scrolled);
        if let Some(editing) = scrolled.inner {
            *form = SourceForm {
                focus_form: true,
                ..SourceForm::of(editing, view)
            };
            // A new source starts with a radio, whose id is not known yet.
            if editing != Editing::New {
                let name = form.field_id(SOURCE_NAME_ID);
                ui.memory_mut(|m| m.request_focus(name));
            }
        }
    }

    /// The app has answered the PIN a dialog sent last frame. A PIN that went
    /// through ends the dialog (the keyboard goes back to the buttons). One the
    /// app turned down (a wrong PIN, a wrong portal password) leaves the dialog
    /// open and empty, the keyboard in its first field, and the app's note beside
    /// it. The app answers within the frame it got the action in, so what the view
    /// shows now is this attempt's answer, never an older one's: it clears its
    /// note before it looks at a PIN.
    fn settle_pin(&mut self, sent: PinAction, view: &SettingsView) {
        let went_through = match sent {
            PinAction::Unlock => view.adult_unlocked,
            PinAction::SetPin => view.has_pin,
            // The PIN stays a PIN: only the app's note says it was changed.
            PinAction::ChangePin => view.adult_note == Some(T::PinChanged),
            PinAction::ResetPin => !view.has_pin,
        };
        if went_through {
            self.adult = AdultDialog::Idle;
            self.local_note = None;
            self.focus_first = true;
        } else if !matches!(self.adult, AdultDialog::Idle) {
            self.adult.empty();
            self.focus_dialog = Some(0);
        }
    }

    /// A PIN dialog is open and the keyboard is in the adult section that shows it.
    fn pin_dialog_open(&self) -> bool {
        self.in_pane
            && Section::ALL[self.section] == Section::Adult
            && !matches!(self.adult, AdultDialog::Idle)
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
        let pushed_before = actions.len();
        // A dialog just opened or emptied, or the section entered again with one
        // open: the keyboard goes to the field this names (the first, mostly).
        let focus = self.focus_dialog.take().or(focus_first.then_some(0));
        let mut dialog = std::mem::take(&mut self.adult);
        let mut open = None;
        let mut sent = None;
        let done = match &mut dialog {
            AdultDialog::Idle => {
                // The first focus goes to the first button that works: Lock,
                // Unlock or, while PIN input is blocked, Forgot PIN. A disabled
                // button keeps no focus, and the keyboard would be left with none.
                ui.horizontal(|ui| {
                    if view.adult_unlocked {
                        let r = ui.button(t(lang, T::Lock));
                        if focus_first {
                            r.request_focus();
                        }
                        if r.clicked() {
                            actions.push(SettingsAction::Lock);
                            // Lock is replaced by Unlock, which is disabled while
                            // PIN input is blocked and then keeps no focus: the
                            // first button that works takes it, next frame.
                            self.focus_first = true;
                        }
                    } else {
                        let r = ui.add_enabled(!blocked, egui::Button::new(t(lang, T::Unlock)));
                        if focus_first && !blocked {
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
                    if view.has_pin && !view.adult_unlocked {
                        let r = ui.button(t(lang, T::ForgotPin));
                        if focus_first && blocked {
                            r.request_focus();
                        }
                        if r.clicked() {
                            open = Some(AdultDialog::Forgot {
                                password: String::new(),
                            });
                        }
                    }
                });
                false
            }
            AdultDialog::Unlock { pin } => {
                let r = pin_field(ui, t(lang, T::EnterPin), pin, focus == Some(0));
                let (ok, cancel) = ok_cancel(ui, lang, &r, !blocked);
                if ok {
                    if valid_pin(pin) {
                        actions.push(SettingsAction::Unlock(pin.clone()));
                        sent = Some(PinAction::Unlock);
                    } else {
                        // Not a PIN: the dialog stays, empty, for another try.
                        self.local_note = Some(T::PinFormat);
                        pin.clear();
                        self.focus_dialog = Some(0);
                    }
                }
                cancel
            }
            AdultDialog::SetPin { first, second } => {
                let r1 = pin_field(ui, t(lang, T::ChoosePin), first, focus == Some(0));
                let r = pin_field(ui, t(lang, T::RepeatPin), second, focus == Some(1));
                // Enter in a field that is not the last goes on to the next one.
                if entered(ui, &r1) {
                    r.request_focus();
                }
                let (ok, cancel) = ok_cancel(ui, lang, &r, true);
                if ok {
                    match new_pin_problem(first, second) {
                        None => {
                            actions.push(SettingsAction::SetPin(first.clone()));
                            sent = Some(PinAction::SetPin);
                        }
                        Some(problem) => {
                            // The pair goes, the keyboard to its first field.
                            self.local_note = Some(problem);
                            first.clear();
                            second.clear();
                            self.focus_dialog = Some(0);
                        }
                    }
                }
                cancel
            }
            AdultDialog::Change { old, first, second } => {
                let r_old = pin_field(ui, t(lang, T::CurrentPin), old, focus == Some(0));
                let r_new = pin_field(ui, t(lang, T::NewPin), first, focus == Some(1));
                let r = pin_field(ui, t(lang, T::RepeatPin), second, focus == Some(2));
                if entered(ui, &r_old) {
                    r_new.request_focus();
                }
                if entered(ui, &r_new) {
                    r.request_focus();
                }
                let (ok, cancel) = ok_cancel(ui, lang, &r, !blocked);
                if ok {
                    let old_is_no_pin = !valid_pin(old);
                    let pair_problem = new_pin_problem(first, second);
                    if !old_is_no_pin && pair_problem.is_none() {
                        actions.push(SettingsAction::ChangePin {
                            old: old.clone(),
                            new: first.clone(),
                        });
                        sent = Some(PinAction::ChangePin);
                    } else {
                        // What is wrong goes, and the keyboard to the first of it.
                        // The shape of the current PIN is told first, as it always was.
                        self.local_note = if old_is_no_pin {
                            Some(T::PinFormat)
                        } else {
                            pair_problem
                        };
                        if old_is_no_pin {
                            old.clear();
                        }
                        if pair_problem.is_some() {
                            first.clear();
                            second.clear();
                        }
                        self.focus_dialog = Some(if old_is_no_pin { 0 } else { 1 });
                    }
                }
                cancel
            }
            AdultDialog::Forgot { password } => {
                ui.label(t(lang, T::PortalPassword));
                let r = ui.add(
                    egui::TextEdit::singleline(password)
                        .password(true)
                        .desired_width(220.0),
                );
                if focus == Some(0) {
                    r.request_focus();
                }
                let (ok, cancel) = ok_cancel(ui, lang, &r, !password.is_empty());
                if ok {
                    actions.push(SettingsAction::ResetPin(without_newlines(password)));
                    sent = Some(PinAction::ResetPin);
                }
                cancel
            }
        };
        if let Some(next) = open {
            self.adult = next;
            self.focus_dialog = Some(0);
            self.local_note = None;
        } else if done {
            // Cancel, the only way out of a dialog besides a PIN that went through.
            self.adult = AdultDialog::Idle;
            self.local_note = None;
            // Back on the buttons, so the keyboard can carry on.
            self.focus_first = true;
        } else {
            self.adult = dialog;
        }
        // What was pushed is the app's to answer now: an older note of ours must
        // not stay in front of the one it sets.
        if actions.len() > pushed_before {
            self.local_note = None;
        }
        if sent.is_some() {
            self.pin_sent = sent;
        }
        if let Some(secs) = view.pin_wait_secs {
            ui.label(tr(lang, T::PinBlocked, &[&secs.to_string()]));
        }
        if let Some(note) = self.local_note.or(view.adult_note) {
            ui.label(t(lang, note));
        }
    }

    /// The news rules: a header, then per rule a row (channel, title patterns,
    /// badge label, seconds to skip, remove) with the channel it found under it,
    /// then Add and Restore defaults and a hint on the patterns. The rules scroll
    /// when `max_height` cannot hold them; the rest stays in view.
    fn news_pane(
        &mut self,
        ui: &mut egui::Ui,
        view: &SettingsView,
        focus_first: bool,
        actions: &mut Vec<SettingsAction>,
        max_height: f32,
    ) {
        let lang = view.lang;
        let rules = self.news.get_or_insert_with(|| view.news_rules.to_vec());
        // The rule whose channel field takes the focus: the first when the section
        // was just entered, else the one an Add or a Remove asked for.
        let focus_row = self.news_focus.take().or(focus_first.then_some(0));
        let last = rules.len().saturating_sub(1);
        let (mut changed, mut remove) = (false, None);
        // The app looks the channels up once this section is shown, so a frame
        // can come before the lines: they stay blank, in their room, rather than
        // say "not found" for a lookup that has not been made.
        let found_in = view.news_found.len() == rules.len();

        let (gap, row_height) = (ui.spacing().item_spacing.x, ui.spacing().interact_size.y);
        let skip_width = ui.spacing().interact_size.x;
        // The rows leave room for the marker that shows where the rules are
        // scrolled to, whether it shows or not: the columns do not shift.
        let row_width = ui.available_width() - SCROLL_MARKER_ROOM;
        let fixed = NEWS_CHANNEL_WIDTH + NEWS_LABEL_WIDTH + skip_width + NEWS_REMOVE_WIDTH;
        let patterns_width = (row_width - fixed - 4.0 * gap).max(NEWS_PATTERNS_MIN_WIDTH);

        ui.horizontal_top(|ui| {
            news_header(ui, NEWS_CHANNEL_WIDTH, t(lang, T::NewsColChannel));
            news_header(ui, patterns_width, t(lang, T::NewsColPatterns));
            news_header(ui, NEWS_LABEL_WIDTH, t(lang, T::NewsColLabel));
            let skip_and_remove = skip_width + gap + NEWS_REMOVE_WIDTH;
            news_header(ui, skip_and_remove, t(lang, T::NewsColSkip));
        });
        // What is left for the rules: the buttons and the hint stay in view below
        // them (see NEWS_HINT_LINES and NEWS_TAIL_GAPS).
        let above = ui.cursor().min.y - ui.min_rect().min.y;
        let below = ui.spacing().interact_size.y
            + NEWS_HINT_LINES * ui.text_style_height(&egui::TextStyle::Small)
            + NEWS_TAIL_GAPS * ui.spacing().item_spacing.y;
        let scrolled = marker_scroll_area("__news_rules__")
            .max_height((max_height - above - below).max(NEWS_MIN_ROWS_HEIGHT))
            .max_width(row_width)
            .auto_shrink([false, true])
            // Tab jumps to a rule; no gliding there.
            .animated(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 4.0;
                for (i, r) in rules.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        let size = |width| egui::vec2(width, row_height);
                        let channel = ui.add_sized(
                            size(NEWS_CHANNEL_WIDTH),
                            egui::TextEdit::singleline(&mut r.channel),
                        );
                        if focus_row.is_some_and(|row| row.min(last) == i) {
                            channel.request_focus();
                        }
                        let patterns = ui.add_sized(
                            size(patterns_width),
                            egui::TextEdit::singleline(&mut r.patterns),
                        );
                        let label = ui.add_sized(
                            size(NEWS_LABEL_WIDTH),
                            egui::TextEdit::singleline(&mut r.label),
                        );
                        let skip = ui.add_sized(
                            size(skip_width),
                            egui::DragValue::new(&mut r.skip_secs).clamp_range(0..=600),
                        );
                        let x = ui
                            .add_sized(size(NEWS_REMOVE_WIDTH), egui::Button::new("x"))
                            .on_hover_text(t(lang, T::NewsRemove));
                        for response in [&channel, &patterns, &label, &skip, &x] {
                            reveal_on_focus(response);
                        }
                        changed |= channel.changed()
                            | patterns.changed()
                            | label.changed()
                            | skip.changed();
                        if x.clicked() {
                            // Whether the keyboard pressed it, for the focus below.
                            remove = Some((i, x.has_focus()));
                        }
                    });
                    // A space is a line of the same height as the words it stands in for.
                    let found = match view.news_found.get(i) {
                        _ if !found_in => " ".to_string(),
                        Some(Some(name)) => tr(lang, T::NewsFound, &[name.as_str()]),
                        _ => t(lang, T::NewsNotFound).to_string(),
                    };
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(found)
                                .small()
                                .color(Color32::from_white_alpha(120)),
                        )
                        .truncate(true),
                    );
                }
            });
        paint_scroll_marker(ui, &scrolled);
        if let Some((i, by_keyboard)) = remove {
            rules.remove(i);
            changed = true;
            // A click leaves the focus as egui has it, nowhere: a key typed next
            // must not edit a rule the user did not pick.
            if by_keyboard {
                self.news_focus = Some(i);
            }
        }
        ui.horizontal(|ui| {
            let add = ui.button(t(lang, T::NewsAddRule));
            // Without rules there is no first field: start on this button.
            if focus_row.is_some() && rules.is_empty() {
                add.request_focus();
            }
            if add.clicked() {
                rules.push(NewsRule {
                    channel: String::new(),
                    patterns: "*".into(),
                    label: String::new(),
                    skip_secs: 0,
                });
                self.news_focus = Some(rules.len() - 1);
                changed = true;
            }
            if ui.button(t(lang, T::NewsRestoreDefaults)).clicked() {
                *rules = default_rules();
                changed = true;
            }
        });
        ui.label(
            egui::RichText::new(t(lang, T::NewsPatternsHelp))
                .small()
                .color(Color32::from_white_alpha(120)),
        );
        if changed {
            actions.push(SettingsAction::SetNewsRules(rules.clone()));
        }
    }
}

/// A column header of the news rules: `width` wide, on a second line where the
/// text needs one.
fn news_header(ui: &mut egui::Ui, width: f32, text: &str) {
    ui.allocate_ui_with_layout(
        egui::vec2(width, 0.0),
        egui::Layout::top_down(egui::Align::Min),
        |ui| {
            ui.set_width(width);
            ui.add(egui::Label::new(egui::RichText::new(text).small()).wrap(true));
        },
    );
}

/// A button that works while `enabled`. Else it is shown faded and is no Tab
/// stop: egui 0.27 lets a disabled widget take the Tab press and drop the focus
/// with it, which would leave the keyboard nowhere.
pub fn action_button(ui: &mut egui::Ui, enabled: bool, text: &str) -> egui::Response {
    let button = egui::Button::new(text);
    if enabled {
        ui.add(button)
    } else {
        ui.add_enabled(false, button.sense(egui::Sense::hover()))
    }
}

/// Tab does not scroll to the widget it focuses, so a rule below the fold has to
/// ask for it.
fn reveal_on_focus(response: &egui::Response) {
    if response.gained_focus() {
        response.scroll_to_me(None);
    }
}

/// A vertical scroll area for a list the keyboard walks through: no scroll bar
/// and no area to drag, since each is a Tab stop in egui (the list's own
/// widgets are the stops, and `reveal_on_focus` scrolls to the one that has the
/// focus). The wheel still scrolls; `paint_scroll_marker` shows where the view is.
pub fn marker_scroll_area(id_source: impl std::hash::Hash) -> egui::ScrollArea {
    egui::ScrollArea::vertical()
        .id_source(id_source)
        .drag_to_scroll(false)
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
}

/// A thin bar in the room beside the right edge of a scroll area's view, shown
/// when the content does not fit it, to tell which part is in view. It senses
/// nothing (egui's own scroll bar is a Tab stop, see `marker_scroll_area`).
pub fn paint_scroll_marker<R>(ui: &egui::Ui, scrolled: &egui::scroll_area::ScrollAreaOutput<R>) {
    // `inner_rect` is the room the area was offered. It ends up as wide as its
    // content when it shrinks to fit it, and as tall as the room when the
    // content does not fit.
    let (offered, content) = (scrolled.inner_rect, scrolled.content_size);
    if content.y <= offered.height() {
        return;
    }
    let right = offered.left() + offered.width().min(content.x);
    let handle = (offered.height() * offered.height() / content.y).max(SCROLL_MARKER_MIN_LENGTH);
    let passed = (scrolled.state.offset.y / (content.y - offered.height())).clamp(0.0, 1.0);
    let top = offered.top() + (offered.height() - handle) * passed;
    let bar = egui::Rect::from_min_size(
        egui::pos2(right + SCROLL_MARKER_ROOM - SCROLL_MARKER_WIDTH, top),
        egui::vec2(SCROLL_MARKER_WIDTH, handle),
    );
    ui.painter().rect_filled(
        bar,
        SCROLL_MARKER_WIDTH / 2.0,
        Color32::from_white_alpha(90),
    );
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

/// Why a new PIN and its repeat cannot go to the app, if they cannot: the PIN is
/// not 4 digits, or the two differ.
fn new_pin_problem(first: &str, second: &str) -> Option<T> {
    if !valid_pin(first) {
        Some(T::PinFormat)
    } else if first != second {
        Some(T::PinMismatch)
    } else {
        None
    }
}

/// Enter was pressed in this single-line field (egui takes its focus away then).
fn entered(ui: &egui::Ui, field: &egui::Response) -> bool {
    field.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter))
}

/// OK and Cancel under a dialog; Enter in its last field counts as OK.
/// Returns (OK, Cancel); OK is only given while `enabled`.
fn ok_cancel(ui: &mut egui::Ui, lang: Lang, last: &egui::Response, enabled: bool) -> (bool, bool) {
    let enter = entered(ui, last);
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

    /// No saved sources: what a view without any shows.
    static NO_SOURCES: Sources = Sources::EMPTY;

    /// The view the overlay is drawn with. Built here only, so a field added
    /// to `SettingsView` is added in one place.
    fn make_view(settings: &Settings) -> SettingsView<'_> {
        SettingsView {
            lang: Lang::En,
            settings,
            creds: None,
            conn: &ConnState::Idle,
            creds_from_command_line: false,
            sources: &NO_SOURCES,
            active_source: None,
            adult_unlocked: false,
            has_pin: false,
            pin_wait_secs: None,
            adult_note: None,
            news_rules: settings.news_rules(),
            news_found: &[],
        }
    }

    /// A new Xtream source's form with these fields, as the section holds it.
    fn new_portal_form(portal: PortalForm) -> SourceForm {
        SourceForm {
            portal,
            ..SourceForm::blank(Editing::New, SourceKind::Xtream, Lang::En)
        }
    }

    /// What Save on a new portal hands the app for this credentials line.
    fn new_portal(line: &str) -> SettingsAction {
        SettingsAction::SaveSource(SourceDraft {
            editing: Editing::New,
            kind: SourceKind::Xtream,
            name: "Portal".into(),
            address: line.into(),
        })
    }

    /// Sources 1 (Xtream, no name of its own so the default one, "Portal" in
    /// English; in use) and 2 ("Gratis", M3U).
    fn two_sources() -> Sources {
        Sources::parse(
            "source=1;xtream;;bob:secret@srv.example.net:8080\n\
             source=2;m3u;Gratis;https://iptv-org.github.io/iptv/index.m3u\n\
             active=1\n",
        )
    }

    /// The text typed into the focused field in one go, as a paste brings it.
    fn typed(text: &str) -> egui::Event {
        egui::Event::Text(text.into())
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

    /// Ctrl+Z as egui gets it: the focused text field undoes its last change.
    fn ctrl_z() -> egui::Event {
        egui::Event::Key {
            key: Key::Z,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::COMMAND,
        }
    }

    /// Shift+Tab as egui gets it: the focus goes to the previous widget.
    fn shift_tab() -> egui::Event {
        egui::Event::Key {
            key: Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Modifiers::SHIFT,
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

    /// The window the headless frames are drawn in, unless a test says otherwise.
    const SCREEN: egui::Vec2 = egui::vec2(1280.0, 720.0);

    /// One headless frame in which these events arrive.
    fn run_events(
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        draw: impl FnOnce(&egui::Context),
    ) {
        let _ = run_events_in(ctx, SCREEN, events, draw);
    }

    /// One headless frame in a window of this size; what it painted.
    fn run_events_in(
        ctx: &egui::Context,
        screen: egui::Vec2,
        events: Vec<egui::Event>,
        draw: impl FnOnce(&egui::Context),
    ) -> egui::FullOutput {
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, screen)),
            events,
            ..Default::default()
        };
        ctx.run(raw, draw)
    }

    /// The scroll markers a frame painted (see `paint_scroll_marker`).
    fn painted_markers(output: &egui::FullOutput) -> Vec<egui::Rect> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(bar) if bar.rect.width() == SCROLL_MARKER_WIDTH => Some(bar.rect),
                _ => None,
            })
            .collect()
    }

    /// Every text a frame painted.
    fn painted_texts(output: &egui::FullOutput) -> Vec<String> {
        output
            .shapes
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                _ => None,
            })
            .collect()
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
        /// What app.rs would tell the sources section: whether the portal in
        /// use comes from `--xtream`, the credentials in use, the saved
        /// sources and the one in use.
        creds_from_command_line: bool,
        creds: Option<XtreamCreds>,
        sources: Sources,
        active_source: Option<SourceId>,
        /// How the connection test stands, as the app says.
        conn: ConnState,
        /// What app.rs would tell the adult section.
        adult_unlocked: bool,
        has_pin: bool,
        pin_wait_secs: Option<u64>,
        /// The app's note on the last PIN.
        adult_note: Option<T>,
        /// The window the overlay is drawn in.
        screen: egui::Vec2,
        /// The language the overlay is drawn in.
        lang: Lang,
        /// What app.rs would tell the News section it found.
        news_found: Vec<Option<String>>,
        /// Every text the last frame painted.
        painted: Vec<String>,
        /// The scroll markers it painted.
        markers: Vec<egui::Rect>,
    }

    impl Overlay {
        fn new() -> Self {
            Self::start(false, SCREEN)
        }

        /// With another default Area on screen in every frame, as while a
        /// toast shows.
        fn with_other_panel() -> Self {
            Self::start(true, SCREEN)
        }

        /// In a window of this size.
        fn in_window(width: f32, height: f32) -> Self {
            Self::start(false, egui::vec2(width, height))
        }

        fn start(other_panel: bool, screen: egui::Vec2) -> Self {
            let mut overlay = Overlay {
                ctx: egui::Context::default(),
                ui: SettingsUi::new(),
                settings: Settings::default(),
                other_panel,
                creds_from_command_line: false,
                creds: None,
                sources: Sources::default(),
                active_source: None,
                conn: ConnState::Idle,
                adult_unlocked: false,
                has_pin: false,
                pin_wait_secs: None,
                adult_note: None,
                screen,
                lang: Lang::En,
                news_found: Vec::new(),
                painted: Vec::new(),
                markers: Vec::new(),
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
                lang: self.lang,
                creds_from_command_line: self.creds_from_command_line,
                sources: &self.sources,
                active_source: self.active_source,
                creds: self.creds.as_ref(),
                conn: &self.conn,
                adult_unlocked: self.adult_unlocked,
                has_pin: self.has_pin,
                pin_wait_secs: self.pin_wait_secs,
                adult_note: self.adult_note,
                news_found: &self.news_found,
                ..make_view(&self.settings)
            };
            let other_panel = self.other_panel;
            let output = run_events_in(&self.ctx, self.screen, events, |ctx| {
                if other_panel {
                    show_other_panel(ctx);
                }
                actions = self.ui.show(ctx, &view)
            });
            self.painted = painted_texts(&output);
            self.markers = painted_markers(&output);
            actions
        }

        /// What app.rs does with an action of the sources section that it
        /// answers by showing a source (a list saved, a source used or deleted),
        /// in the frame of the action, after the section has drawn it.
        fn the_app_answers(&mut self, editing: Editing) {
            self.ui.show_source(editing);
            self.ui.settle_keyboard(&self.ctx);
        }

        /// The same for the answer to an Xtream Save, which comes when its test is
        /// over, in whatever frame that is.
        fn the_app_answers_a_test(&mut self, editing: Editing) {
            self.ui.show_kept_source(editing);
            self.ui.settle_keyboard(&self.ctx);
        }

        /// A click with the primary mouse button at `pos`: the pointer moves
        /// there, presses and releases, one frame each; what the overlay reports
        /// over the three.
        fn click_at(&mut self, pos: egui::Pos2) -> Vec<SettingsAction> {
            let button = |pressed| egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Modifiers::NONE,
            };
            let mut actions = self.frame_with(vec![egui::Event::PointerMoved(pos)]);
            actions.extend(self.frame_with(vec![button(true)]));
            actions.extend(self.frame_with(vec![button(false)]));
            actions
        }

        /// The widget with the keyboard focus is in sight: not cut off by the
        /// edge of the scrolling rules, inside the overlay, inside the window.
        fn assert_focus_in_view(&self, what: &str) {
            let field = self.focused_response();
            assert_eq!(
                field.interact_rect, field.rect,
                "{what}: cut off by the list's edge"
            );
            assert!(self.overlay_rect().contains_rect(field.rect), "{what}");
            assert!(
                self.screen_rect().contains_rect(self.overlay_rect()),
                "{what}"
            );
        }

        /// Into the News section: its first control has the focus, the first
        /// rule's channel field (the Add button without rules).
        fn open_news_pane(&mut self) {
            self.press(Key::ArrowDown, 4);
            self.press(Key::Enter, 1);
        }

        fn screen_rect(&self) -> egui::Rect {
            egui::Rect::from_min_size(egui::Pos2::ZERO, self.screen)
        }

        /// Where the overlay is, as of the last frame.
        fn overlay_rect(&self) -> egui::Rect {
            let area = egui::Id::new(AREA_ID);
            self.ctx
                .memory(|m| m.area_rect(area))
                .expect("the overlay has been drawn")
        }

        /// The widget with the keyboard focus, as of the last frame.
        fn focused_response(&self) -> egui::Response {
            let id = self
                .ctx
                .memory(|m| m.focused())
                .expect("a widget has the focus");
            self.ctx.read_response(id).expect("it was drawn")
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

        /// Whether the last frame painted this text.
        fn says(&self, text: &str) -> bool {
            self.painted.iter().any(|p| p == text)
        }

        /// The app turns the PIN down: it stays as it was and says why.
        fn app_turns_the_pin_down(&mut self, note: T) {
            self.adult_note = Some(note);
            assert_eq!(self.frame(&[]), vec![]);
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
    /// form and test state: into the Sources pane, on a new Xtream source's
    /// form, Tab from the server field to the buttons, Enter on each. A
    /// disabled button is no Tab stop and takes no Enter.
    fn press_test_and_save(form: PortalForm, conn: &ConnState) -> Vec<SettingsAction> {
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let view = SettingsView {
            conn,
            ..make_view(&settings)
        };
        let mut ui = SettingsUi::new();
        ui.sources = Some(new_portal_form(form));
        let mut frame = |keys: &[Key]| {
            let mut actions = Vec::new();
            run_frame(&ctx, keys, |ctx| actions = ui.show(ctx, &view));
            actions
        };
        frame(&[]);
        frame(&[]);
        frame(&[Key::ArrowDown]); // Sources
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
                new_portal(&one_string)
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
        o.press(Key::ArrowDown, 1); // Sources
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
        let host = o.ui.sources.as_ref().map(|form| form.portal.host.as_str());
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
        frame(&[Key::ArrowDown]); // Sources
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
        assert_eq!(frame(&[Key::Enter]), vec![new_portal(one_string)]);
        assert_eq!(crate::args::parse_xtream_creds(one_string).unwrap(), creds);
    }

    #[test]
    fn test_and_save_hand_over_what_was_pasted_cleaned() {
        // egui keeps the CR/LF of a paste in a single-line field.
        let pasted = PortalForm {
            host: " https://srv.example.net:8080/\r\n".into(),
            user: "bob\n".into(),
            pass: "se cret\r\n".into(),
        };
        let one_string = "bob:se cret@srv.example.net:8080".to_string();
        assert_eq!(
            press_test_and_save(pasted, &ConnState::Idle),
            vec![
                SettingsAction::TestConnection(one_string.clone()),
                new_portal(&one_string)
            ]
        );
    }

    #[test]
    fn the_portal_section_says_when_the_portal_comes_from_the_command_line() {
        for lang in Lang::ALL {
            let note = t(lang, T::PortalFromCommandLine);
            for from_cli in [false, true] {
                let mut o = Overlay::new();
                o.lang = lang;
                o.creds_from_command_line = from_cli;
                o.press(Key::ArrowDown, 1); // Sources
                assert_eq!(o.frame(&[]), vec![]);
                let shown = o.painted.iter().any(|p| p == note);
                assert_eq!(shown, from_cli, "{lang:?}, --xtream given: {from_cli}");
            }
        }
    }

    #[test]
    fn new_source_is_six_tabs_from_the_server_field_with_the_note_or_without() {
        // Server, user, password, Test, Save, then one row (the --xtream portal's,
        // or the saved one in use), then New: the note is no Tab stop, and Use and
        // Delete, disabled for the source in use, are none either.
        for from_cli in [false, true] {
            let mut o = Overlay::new();
            o.creds_from_command_line = from_cli;
            if from_cli {
                // Filled in, as --xtream gives them: Test and Save are stops.
                o.creds = parse_xtream_creds("bob:secret@srv.example.net:8080").ok();
            } else {
                o.sources = Sources::parse("portal=bob:secret@srv.example.net:8080\n");
                o.active_source = Some(1);
            }
            o.press(Key::ArrowDown, 1);
            o.press(Key::Enter, 1); // into the section: the server field
            o.press(Key::Tab, 6);
            assert_eq!(o.frame(&[Key::Enter]), vec![], "--xtream: {from_cli}");
            o.frame(&[]);
            let form = o.ui.sources.as_ref().unwrap();
            assert_eq!(form.editing, Editing::New, "--xtream: {from_cli}");
        }
    }

    #[test]
    fn an_m3u_list_is_added_by_keyboard() {
        // The keys of the e2e scenario M-add, as far as its Save.
        let mut o = Overlay::new();
        // The scenario's ini: the mock's portal saved as source 1, in use.
        o.sources = Sources::parse("portal=tester:secret@127.0.0.1:18080\n");
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1); // source 1's server field
        o.press(Key::Tab, 6); // past user, password, Test, Save and its row: New
        o.press(Key::Enter, 1); // a new source: the Xtream radio has the focus
        o.press(Key::Tab, 1); // the M3U radio
        o.press(Key::Enter, 1); // M3U: its address field follows the radios
        o.press(Key::Tab, 1);
        o.frame_with(vec![typed("https://lists.example/tv.m3u")]);
        o.press(Key::Tab, 1); // Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::M3u,
                name: "M3U list".into(),
                address: "https://lists.example/tv.m3u".into(),
            })]
        );
    }

    #[test]
    fn save_refuses_an_empty_name_and_an_address_that_is_no_url_or_file() {
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: " ".into(),
            address: "https://lists.example/tv.m3u".into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1); // the address field
        o.press(Key::Tab, 1); // Save
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        assert!(
            o.painted.iter().any(|p| p == "give the source a name"),
            "{:?}",
            o.painted
        );
        o.ui.sources.as_mut().unwrap().name = "Lijst".into();
        o.ui.sources.as_mut().unwrap().address = "no address at all".into();
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        assert!(
            o.painted
                .iter()
                .any(|p| p == "not an http(s) address or an existing file"),
            "{:?}",
            o.painted
        );
    }

    #[test]
    fn an_existing_file_is_an_address_too() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, "#EXTM3U\n").unwrap();
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Lijst".into(),
            address: file.display().to_string(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 1);
        let actions = o.frame(&[Key::Enter]);
        assert!(
            matches!(actions.as_slice(), [SettingsAction::SaveSource(d)] if d.address == file.display().to_string()),
            "{actions:?}"
        );
    }

    #[test]
    fn a_get_php_link_by_keyboard_becomes_an_xtream_source_to_save() {
        // The keys of the e2e scenario M-getphp.
        let link = "http://127.0.0.1:18080/get.php?username=tester&password=secret&type=m3u_plus";
        let mut o = Overlay::new();
        // The scenario's ini: the mock's portal saved as source 1, in use.
        o.sources = Sources::parse("portal=tester:secret@127.0.0.1:18080\n");
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 6);
        o.press(Key::Enter, 1); // New
        o.press(Key::Tab, 1);
        o.press(Key::Enter, 1); // M3U
        o.press(Key::Tab, 1);
        o.frame_with(vec![typed(link)]);
        o.press(Key::Tab, 1); // Save
        assert_eq!(o.frame(&[Key::Enter]), vec![], "the question first");
        o.frame(&[]);
        assert!(o
            .painted
            .iter()
            .any(|p| p.starts_with("This is an Xtream portal's link")));
        o.press(Key::Tab, 1); // "As an Xtream source"
        o.press(Key::Enter, 1); // the form is an Xtream one now, the server field focused
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.kind, SourceKind::Xtream);
        assert_eq!(
            (
                form.portal.host.as_str(),
                form.portal.user.as_str(),
                form.portal.pass.as_str()
            ),
            ("127.0.0.1:18080", "tester", "secret")
        );
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![new_portal("tester:secret@127.0.0.1:18080")]
        );
    }

    #[test]
    fn a_get_php_link_kept_as_a_list_is_saved_as_typed() {
        let link = "http://srv.example.net/get.php?username=u&password=p";
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Lijst".into(),
            address: link.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 1);
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.press(Key::Tab, 2); // past "As an Xtream source": "As an M3U list"
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::M3u,
                name: "Lijst".into(),
                address: link.into(),
            })]
        );
    }

    #[test]
    fn the_list_marks_the_source_in_use_and_use_sends_the_chosen_one() {
        // The keys of the e2e scenario M-switch.
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1); // source 1's server field
        o.frame(&[]);
        assert!(
            o.painted.iter().any(|p| p == "Portal · Xtream · in use"),
            "{:?}",
            o.painted
        );
        assert!(
            o.painted.iter().any(|p| p == "Gratis · M3U"),
            "{:?}",
            o.painted
        );
        o.press(Key::Tab, 6); // user, password, Test, Save, row 1, row 2
        o.press(Key::Enter, 1); // row 2 chosen: its form, the keyboard on its name
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(2));
        o.press(Key::Tab, 5); // address, Save, row 1, row 2, Use this source
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ActivateSource(2)]
        );
    }

    #[test]
    fn delete_asks_twice_and_is_not_there_for_the_source_in_use() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 6);
        o.press(Key::Enter, 1); // row 2
        o.press(Key::Tab, 7); // address, Save, row 1, row 2, Use, New, Delete
        assert_eq!(o.frame(&[Key::Enter]), vec![], "a second press deletes");
        o.frame(&[]);
        assert!(o
            .painted
            .iter()
            .any(|p| p == "Really delete (with favorites)?"));
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::DeleteSource(2)]
        );
        // Source 1 is in use: past its row and row 2 comes New, not Use.
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 7);
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::New);
    }

    #[test]
    fn the_command_line_portal_is_a_row_of_its_own_and_no_saved_one_is_marked() {
        let mut o = Overlay::new();
        o.creds_from_command_line = true;
        o.sources = two_sources();
        o.active_source = None; // app.rs: the --xtream portal runs
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        o.frame(&[]);
        assert!(o
            .painted
            .iter()
            .any(|p| p == "--xtream (command line) · in use"));
        assert!(
            o.painted.iter().any(|p| p == "Portal · Xtream"),
            "{:?}",
            o.painted
        );
    }

    // ---- the sources pane: its Tab stops, focus, texts and limits ----

    /// Twelve saved lists, the first in use: someone who keeps many sources.
    fn twelve_sources() -> Sources {
        let lines: String = (1..=12)
            .map(|i| format!("source={i};m3u;Lijst {i};https://a.example/{i}.m3u\n"))
            .collect();
        Sources::parse(&format!("{lines}active=1\n"))
    }

    /// Into the Sources section: its form's server or address field has the focus.
    fn open_sources_pane(o: &mut Overlay) {
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
    }

    /// Shift+Tab, and the frame in which egui has moved the focus back.
    fn tab_back(o: &mut Overlay) {
        o.frame_with(vec![shift_tab()]);
        assert_eq!(o.frame(&[]), vec![]);
    }

    #[test]
    fn the_section_is_called_bronnen_sources_quellen() {
        // The Portal row's place: second in the list.
        assert_eq!(Section::ALL[1], Section::Sources);
        for (lang, title) in [
            (Lang::Nl, "Bronnen"),
            (Lang::En, "Sources"),
            (Lang::De, "Quellen"),
        ] {
            let mut o = Overlay::new();
            o.lang = lang;
            assert_eq!(o.frame(&[]), vec![]);
            assert!(o.says(title), "{lang:?}: {:?}", o.painted);
        }
    }

    #[test]
    fn an_empty_name_or_address_is_never_handed_to_the_app() {
        let list = |name: &str, address: &str| SourceForm {
            name: name.into(),
            address: address.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        };
        assert_eq!(
            list(" \r\n", "https://lists.example/tv.m3u").draft().err(),
            Some(T::SourceNameMissing)
        );
        for address in ["", "   ", "\r\n", " \t "] {
            assert_eq!(
                list("Lijst", address).draft().err(),
                Some(T::SourceAddressInvalid),
                "{address:?}"
            );
        }
        // Save is no Tab stop for a list without an address: from the address
        // field the next stop is New source, which starts the form over.
        for address in ["", "   "] {
            let mut o = Overlay::new();
            o.ui.sources = Some(list("Lijst", address));
            open_sources_pane(&mut o);
            o.press(Key::Tab, 1);
            assert_eq!(o.frame(&[Key::Enter]), vec![], "{address:?}");
            o.frame(&[]);
            let form = o.ui.sources.as_ref().unwrap();
            assert_eq!(form.kind, SourceKind::Xtream, "New source, not Save");
            assert_eq!(form.problem, None, "{address:?}");
        }
    }

    #[test]
    fn a_disabled_button_is_no_tab_stop() {
        // A new Xtream source with a name and nothing else: Test and Save are
        // disabled, and so are Use and Delete. From the password field the next
        // stop is New source (egui 0.27 would let a disabled button take the
        // Tab and leave the keyboard nowhere).
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Mijn bron".into(),
            ..SourceForm::blank(Editing::New, SourceKind::Xtream, Lang::En)
        });
        open_sources_pane(&mut o);
        o.press(Key::Tab, 3); // user, password, New source
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.name, "Portal", "New source started the form over");
    }

    #[test]
    fn use_and_delete_are_no_tab_stops_for_the_source_in_use() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        // user, password, Test, Save, row 1, row 2, New: then Delete is disabled
        // and the next Tab is back on the list.
        o.press(Key::Tab, 8);
        assert_eq!(o.frame(&[]), vec![]);
        let row = o.ctx.memory(|m| m.focused()).expect("a widget has focus");
        assert_eq!(o.ui.list_ids.first(), Some(&row));
    }

    #[test]
    fn a_new_source_starts_with_the_name_the_two_kinds_and_then_the_server_field() {
        // Backwards from the server field: the M3U radio, the Xtream radio, then
        // the name, the first stop of the pane.
        let mut o = Overlay::new();
        open_sources_pane(&mut o);
        tab_back(&mut o);
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().kind, SourceKind::M3u);
        tab_back(&mut o);
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().kind, SourceKind::Xtream);
        tab_back(&mut o);
        assert!(o.type_char(Key::X, 'x').is_empty());
        assert_eq!(o.ui.sources.as_ref().unwrap().name, "Portalx");
    }

    #[test]
    fn a_saved_source_has_no_kind_radios_and_its_name_comes_first() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        tab_back(&mut o);
        assert!(o.type_char(Key::X, 'x').is_empty());
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(
            (form.editing, form.name.as_str()),
            (Editing::Saved(1), "Portalx")
        );
    }

    #[test]
    fn the_name_of_a_new_source_follows_its_kind_until_it_is_a_name_of_its_own() {
        let mut o = Overlay::new();
        open_sources_pane(&mut o);
        let name = |o: &Overlay| o.ui.sources.as_ref().unwrap().name.clone();
        tab_back(&mut o); // the M3U radio
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(name(&o), "M3U list", "the default name of the kind");
        tab_back(&mut o); // the Xtream radio
        tab_back(&mut o); // the name
        assert!(o.type_char(Key::X, 'x').is_empty());
        o.press(Key::Tab, 1); // the Xtream radio
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().kind, SourceKind::Xtream);
        assert_eq!(name(&o), "M3U listx", "a name of its own stays");
    }

    #[test]
    fn the_command_line_portal_keeps_the_name_of_the_first_saved_portal_when_saved() {
        // Save on the --xtream row updates the first saved portal, "Thuis": the
        // name field shows it, so Save does not rename it to the default.
        let mut o = Overlay::new();
        o.creds_from_command_line = true;
        o.creds = parse_xtream_creds("bob:secret@srv.example.net:8080").ok();
        o.sources = Sources::parse(
            "source=1;m3u;Lijst;https://a.example/tv.m3u\n\
             source=2;xtream;Thuis;u:p@h:80\n\
             active=2\n",
        );
        o.active_source = None; // the --xtream portal runs
        open_sources_pane(&mut o);
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::CommandLine,
                kind: SourceKind::Xtream,
                name: "Thuis".into(),
                address: "bob:secret@srv.example.net:8080".into(),
            })]
        );
        // With no saved portal there is nothing to keep: the default name.
        let mut o = Overlay::new();
        o.creds_from_command_line = true;
        o.creds = parse_xtream_creds("bob:secret@srv.example.net:8080").ok();
        o.sources = Sources::parse("source=1;m3u;Lijst;https://a.example/tv.m3u\nactive=1\n");
        open_sources_pane(&mut o);
        o.press(Key::Tab, 4);
        let actions = o.frame(&[Key::Enter]);
        assert!(
            matches!(actions.as_slice(), [SettingsAction::SaveSource(d)] if d.name == "Portal"),
            "{actions:?}"
        );
    }

    #[test]
    fn an_https_get_php_link_is_saved_as_a_list_without_a_question() {
        // The Xtream client speaks plain HTTP only: a source made of this link
        // would never connect, so only an http:// link is offered as Xtream.
        let link = "https://srv.example.net/get.php?username=u&password=p";
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Lijst".into(),
            address: link.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        open_sources_pane(&mut o);
        o.press(Key::Tab, 1); // Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::M3u,
                name: "Lijst".into(),
                address: link.into(),
            })]
        );
        o.frame(&[]);
        assert!(!o.says(t(Lang::En, T::XtreamLinkFound)), "{:?}", o.painted);
    }

    #[test]
    fn a_get_php_link_typed_into_a_saved_list_becomes_a_new_xtream_source() {
        // The saved list keeps its kind and its address: the Xtream source the
        // link is offered as is a new one, or its Save would go to a source that
        // is not an Xtream one.
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 6);
        o.press(Key::Enter, 1); // row 2, "Gratis"
        o.ui.sources.as_mut().unwrap().address =
            "http://srv.example.net:8080/get.php?username=u&password=p".into();
        o.press(Key::Tab, 2); // address, Save
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        o.press(Key::Tab, 1); // "As an Xtream source"
        o.press(Key::Enter, 1);
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(
            (form.editing, form.kind),
            (Editing::New, SourceKind::Xtream)
        );
        assert_eq!(form.name, "Gratis", "a name of its own stays");
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::Xtream,
                name: "Gratis".into(),
                address: "u:p@srv.example.net:8080".into(),
            })]
        );
    }

    #[test]
    fn a_second_save_asks_again_instead_of_keeping_an_old_question() {
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Lijst".into(),
            address: "http://srv.example.net/get.php?username=u&password=p".into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        open_sources_pane(&mut o);
        o.press(Key::Tab, 1);
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert!(o.ui.sources.as_ref().unwrap().xtream_offer.is_some());
        // The address is another one by now: the old question goes with Save.
        o.ui.sources.as_mut().unwrap().address = "https://lists.example/tv.m3u".into();
        let actions = o.frame(&[Key::Enter]);
        assert!(matches!(
            actions.as_slice(),
            [SettingsAction::SaveSource(_)]
        ));
        assert!(o.ui.sources.as_ref().unwrap().xtream_offer.is_none());
    }

    #[test]
    fn a_form_whose_source_is_deleted_starts_again_from_the_sources_that_are_left() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 6);
        o.press(Key::Enter, 1); // row 2
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(2));
        // The app deletes it.
        o.sources = Sources::parse("portal=bob:secret@srv.example.net:8080\n");
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(1));
    }

    #[test]
    fn a_saved_line_that_is_no_credentials_fills_the_server_field_as_it_is() {
        // A hand-edited ini: all of the line in the server field, so the user
        // sees what is there.
        let mut o = Overlay::new();
        o.sources = Sources::parse("source=1;xtream;;not credentials at all\nactive=1\n");
        o.active_source = Some(1);
        o.frame(&[Key::ArrowDown]);
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.portal.host, "not credentials at all");
        assert_eq!(
            (form.portal.user.as_str(), form.portal.pass.as_str()),
            ("", "")
        );
    }

    #[test]
    fn show_source_puts_the_keyboard_on_the_name_of_that_source() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        // What the app asks after a save or a switch.
        o.ui.show_source(Editing::Saved(2));
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(2));
        assert!(o.type_char(Key::X, 'x').is_empty());
        assert_eq!(o.ui.sources.as_ref().unwrap().name, "Gratisx");
        // A new source: the keyboard is on its Xtream radio.
        o.ui.show_source(Editing::New);
        assert_eq!(o.frame(&[]), vec![]);
        o.press(Key::Tab, 1); // the M3U radio
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().kind, SourceKind::M3u);
    }

    #[test]
    fn a_source_the_app_shows_does_not_take_the_keyboard_from_the_list() {
        // The user left the form while a test of credentials ran: when it ends,
        // the form shows the source it kept, and the keyboard stays where the
        // user put it.
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        o.press(Key::ArrowDown, 1); // Sources, in the list
        o.the_app_answers(Editing::Saved(2));
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(2));
        assert_eq!(o.ctx.memory(|m| m.focused()), None);
        // Enter on the Sources row goes into the form as ever.
        o.press(Key::Enter, 1);
        assert!(o.type_char(Key::X, 'x').is_empty());
        let address = o.ui.sources.as_ref().unwrap().address.clone();
        assert!(address.contains('x'), "typed into the address: {address}");
    }

    #[test]
    fn the_answer_to_a_save_from_a_form_the_user_left_for_the_list_keeps_the_keyboard_there() {
        let line = "bob:secret@srv.example.net:8080";
        let mut o = Overlay::new();
        o.ui.sources = Some(new_portal_form(PortalForm::of_line(line)));
        open_sources_pane(&mut o);
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(o.frame(&[Key::Enter]), vec![new_portal(line)]);
        o.press(Key::Escape, 1); // out of the form, into the list of sections
                                 // The login works: the form that asked is the one still shown, so it
                                 // shows the source that was kept; the keyboard stays in the list.
        o.sources = Sources::parse(&format!("portal={line}\n"));
        o.active_source = Some(1);
        o.the_app_answers_a_test(Editing::Saved(1));
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(1));
        assert_eq!(o.ctx.memory(|m| m.focused()), None);
    }

    #[test]
    fn a_form_shown_by_the_app_takes_a_tab_that_comes_with_it_from_its_first_control() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.ui.show_source(Editing::New);
        // The Tab comes in the very frame the form is made in: it moves the
        // keyboard from the Xtream radio to the M3U radio.
        assert_eq!(o.frame(&[Key::Tab]), vec![]);
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(o.ui.sources.as_ref().unwrap().kind, SourceKind::M3u);
    }

    #[test]
    fn open_new_list_opens_the_section_on_an_empty_list_with_the_keyboard_in_its_address() {
        let mut o = Overlay::new();
        o.ui.open_new_list();
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.ui.form_address(), Some((SourceKind::M3u, String::new())));
        o.frame_with(vec![typed("x")]);
        assert_eq!(o.ui.form_address(), Some((SourceKind::M3u, "x".to_owned())));
        // The section is the one shown, and the keyboard is in it: `o` is a letter.
        assert_eq!(o.frame(&[Key::O]), vec![]);
    }

    #[test]
    fn the_keyboard_asked_for_in_the_overlays_first_frame_comes_a_frame_later() {
        // The overlay's first frame is a sizing pass in which everything is
        // disabled and drops the focus it is given. Opened by the setup prompt's
        // link, the address field has the keyboard once the overlay shows.
        let ctx = egui::Context::default();
        let settings = Settings::default();
        let view = make_view(&settings);
        let mut ui = SettingsUi::new();
        ui.open_new_list();
        for _ in 0..3 {
            run_frame(&ctx, &[], |ctx| {
                ui.show(ctx, &view);
            });
        }
        run_events(&ctx, vec![typed("x")], |ctx| {
            ui.show(ctx, &view);
        });
        assert_eq!(ui.form_address(), Some((SourceKind::M3u, "x".to_owned())));
    }

    #[test]
    fn the_sources_section_never_makes_the_overlay_bigger_than_its_window() {
        // The smallest window the app allows, and some bigger ones; German has
        // the longest texts. Many sources, the --xtream row, and the question
        // about a get.php link, which has the longest text of all.
        for lang in Lang::ALL {
            for (width, height) in [(640.0, 360.0), (800.0, 450.0), (1280.0, 720.0)] {
                for asking in [false, true] {
                    let mut o = Overlay::in_window(width, height);
                    o.lang = lang;
                    o.creds_from_command_line = true;
                    o.creds = parse_xtream_creds("bob:secret@srv.example.net:8080").ok();
                    o.sources = twelve_sources();
                    if asking {
                        o.ui.sources = Some(SourceForm {
                            name: "Lijst".into(),
                            address: "http://srv.example.net/get.php?username=u&password=p".into(),
                            xtream_offer: Some("u:p@srv.example.net:80".into()),
                            ..SourceForm::blank(Editing::New, SourceKind::M3u, lang)
                        });
                    }
                    open_sources_pane(&mut o);
                    assert_eq!(o.frame(&[]), vec![]);
                    let (screen, overlay) = (o.screen_rect(), o.overlay_rect());
                    let name = format!("{width}x{height}, {lang:?}, asking: {asking}");
                    assert!(
                        screen.contains_rect(overlay),
                        "{name}: {overlay:?} is not inside {screen:?}"
                    );
                    // The pane fits the overlay: it does not widen it.
                    let designed = overlay_width(width) + 2.0 * OVERLAY_FRAME_MARGIN;
                    assert!(
                        overlay.width() <= designed + 0.5,
                        "{name}: {} wide, designed for {designed}",
                        overlay.width()
                    );
                }
            }
        }
    }

    #[test]
    fn tab_scrolls_a_source_below_the_fold_into_view() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.sources = twelve_sources();
        o.active_source = Some(1);
        // Source 1 is a list: the address has the focus.
        open_sources_pane(&mut o);
        // Save, then the twelve rows: the last of them is far below the fold.
        o.press(Key::Tab, 13);
        assert_eq!(o.frame(&[]), vec![]); // the scroll shows in the frame after the Tab
        o.assert_focus_in_view("the last source's row");
        // And it is the last row: Enter chooses it, and the keyboard is on its name.
        o.press(Key::Enter, 1);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(12));
        assert_eq!(o.frame(&[]), vec![]); // the form asks for the scroll ...
        assert_eq!(o.frame(&[]), vec![]); // ... and shows in the next frame
        o.assert_focus_in_view("the name of source 12");
        // The buttons below the rows come into view too: address, Save, twelve
        // rows, then Use this source, New source and Delete.
        o.press(Key::Tab, 17);
        assert_eq!(o.frame(&[]), vec![]);
        o.assert_focus_in_view("Delete");
        assert_eq!(o.frame(&[Key::Enter]), vec![], "the first press only asks");
        o.frame(&[]);
        assert!(
            o.says(t(Lang::En, T::ButtonConfirmDelete)),
            "{:?}",
            o.painted
        );
    }

    #[test]
    fn the_sources_list_adds_no_tab_stop_of_its_own() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.sources = twelve_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        // Backwards from the name is the News row of the list, not something the
        // scroll area put in front of the form.
        tab_back(&mut o); // the name
        tab_back(&mut o);
        let row = o.ctx.memory(|m| m.focused()).expect("a row has the focus");
        assert_eq!(o.ui.list_ids.last(), Some(&row));
    }

    // ---- the sources pane: who owns the verdict, answers, field state ----

    /// What the status line says for a test that found `5` channels.
    const CONNECTED_5: &str = "Connected: 5 channels";

    fn found_5_channels() -> ConnState {
        ConnState::Done(ConnCheck::Ok { channels: Some(5) })
    }

    #[test]
    fn the_verdict_of_a_test_shows_under_the_form_that_asked_for_it() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 3); // user, password, Test
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::TestConnection(
                "bob:secret@srv.example.net:8080".into()
            )]
        );
        // What the app says when the test is over.
        o.conn = found_5_channels();
        o.frame(&[]);
        assert!(o.says(CONNECTED_5), "{:?}", o.painted);
        // New source: a blank form, which no one has tested.
        o.press(Key::Tab, 4); // Save, row 1, row 2, New source
        o.press(Key::Enter, 1);
        o.frame(&[]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::New);
        assert!(!o.says(CONNECTED_5), "{:?}", o.painted);
    }

    #[test]
    fn a_portal_chosen_after_a_test_of_another_shows_no_verdict() {
        let mut o = Overlay::new();
        o.sources = Sources::parse(
            "source=1;xtream;;ann:pw@srv-a.example.net:8080\n\
             source=2;xtream;Tweede;ben:pw@srv-b.example.net:8080\n\
             active=1\n",
        );
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 3); // user, password, Test
        assert!(matches!(
            o.frame(&[Key::Enter]).as_slice(),
            [SettingsAction::TestConnection(_)]
        ));
        o.conn = found_5_channels();
        o.frame(&[]);
        assert!(o.says(CONNECTED_5), "{:?}", o.painted);
        o.press(Key::Tab, 3); // Save, row 1, row 2
        o.press(Key::Enter, 1); // the second portal, which was not tested
        o.frame(&[]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(2));
        assert!(!o.says(CONNECTED_5), "{:?}", o.painted);
        // Back on the first: a new form, and no one has tested it either.
        o.press(Key::Tab, 6); // server, user, password, Test, Save, row 1
        o.press(Key::Enter, 1);
        o.frame(&[]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(1));
        assert!(!o.says(CONNECTED_5), "{:?}", o.painted);
    }

    #[test]
    fn the_form_shown_after_a_save_carries_the_verdict_of_that_save() {
        let line = "bob:secret@srv.example.net:8080";
        let mut o = Overlay::new();
        o.ui.sources = Some(new_portal_form(PortalForm::of_line(line)));
        open_sources_pane(&mut o);
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(o.frame(&[Key::Enter]), vec![new_portal(line)]);
        o.conn = ConnState::Testing;
        o.frame(&[]);
        assert!(o.says("testing..."), "{:?}", o.painted);
        // The login works, and the app keeps the portal as source 1.
        o.sources = Sources::parse(&format!("portal={line}\n"));
        o.active_source = Some(1);
        o.conn = found_5_channels();
        o.the_app_answers_a_test(Editing::Saved(1));
        o.frame(&[]);
        assert_eq!(o.ui.sources.as_ref().unwrap().editing, Editing::Saved(1));
        assert!(o.says(CONNECTED_5), "{:?}", o.painted);
    }

    #[test]
    fn a_late_answer_does_not_replace_the_form_the_user_moved_on_to() {
        let line = "bob:secret@srv.example.net:8080";
        let mut o = Overlay::new();
        o.ui.sources = Some(new_portal_form(PortalForm::of_line(line)));
        open_sources_pane(&mut o);
        o.press(Key::Tab, 4); // user, password, Test, Save
        assert_eq!(o.frame(&[Key::Enter]), vec![new_portal(line)]);
        o.conn = ConnState::Testing;
        // The login is on its way. Meanwhile the user starts another new source.
        o.press(Key::Tab, 1); // New source: no rows yet, Use and Delete are disabled
        o.press(Key::Enter, 1); // its Xtream radio has the keyboard
        o.press(Key::Tab, 2); // the M3U radio, the server field
        assert!(o.type_char(Key::X, 'x').is_empty());
        // The login works, and the app keeps the portal as source 1.
        o.sources = Sources::parse(&format!("portal={line}\n"));
        o.active_source = Some(1);
        o.conn = found_5_channels();
        o.the_app_answers_a_test(Editing::Saved(1));
        assert_eq!(o.frame(&[]), vec![]);
        // The new form is where the user left it, with its character, and the
        // keyboard is still in its field; the list shows the saved portal.
        let host = Some((SourceKind::Xtream, "x".to_owned()));
        assert_eq!(o.ui.form_address(), host);
        assert!(o.says("Portal · Xtream · in use"), "{:?}", o.painted);
        assert!(!o.says(CONNECTED_5), "{:?}", o.painted);
        assert!(o.type_char(Key::Y, 'y').is_empty());
        let host = Some((SourceKind::Xtream, "xy".to_owned()));
        assert_eq!(o.ui.form_address(), host);
    }

    #[test]
    fn an_answer_to_a_save_that_comes_with_the_users_own_move_on_its_way_is_left_alone() {
        let mut o = Overlay::new();
        o.sources = Sources::parse(
            "source=1;xtream;;ann:pw@srv-a.example.net:8080\n\
             source=2;xtream;Tweede;ben:pw@srv-b.example.net:8080\n\
             active=1\n",
        );
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 6); // user, password, Test, Save, row 1, row 2
        o.press(Key::Enter, 1); // the second portal, which is not in use
        o.press(Key::Tab, 4); // server, user, password, Test
        assert!(matches!(
            o.frame(&[Key::Enter]).as_slice(),
            [SettingsAction::TestConnection(_)]
        ));
        o.conn = found_5_channels();
        o.press(Key::Tab, 4); // Save, row 1, row 2, Use this source
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ActivateSource(2)]
        );
        // The app answers the Use at once, and a verdict comes in the same frame.
        o.the_app_answers(Editing::Saved(2));
        o.the_app_answers_a_test(Editing::Saved(2));
        assert_eq!(o.frame(&[]), vec![]);
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.editing, Editing::Saved(2));
        assert!(!form.owns_verdict, "the form of the Use is a new one");
        assert!(!o.says(CONNECTED_5), "{:?}", o.painted);
    }

    #[test]
    fn a_tab_in_the_very_next_frame_after_the_apps_answer_moves_on_from_the_name() {
        let mut o = Overlay::new();
        o.sources = two_sources();
        o.active_source = Some(1);
        open_sources_pane(&mut o); // source 1's server field
                                   // The app answers an action of this frame with the list it kept ...
        o.the_app_answers(Editing::Saved(2));
        // ... and the next frame has the Tab, with no quiet frame between.
        assert_eq!(o.frame(&[Key::Tab]), vec![]);
        assert!(o.type_char(Key::X, 'x').is_empty());
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.name, "Gratis", "the keyboard left the name");
        assert!(
            form.address.ends_with('x'),
            "and is in the address: {form:?}"
        );
    }

    #[test]
    fn ctrl_z_in_one_sources_password_does_not_bring_back_anothers() {
        let mut o = Overlay::new();
        o.sources = Sources::parse(
            "source=1;xtream;;ann:secretA@srv-a.example.net:8080\n\
             source=2;xtream;Tweede;ben:otherB@srv-b.example.net:8080\n\
             active=1\n",
        );
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 2); // user, password
        assert!(o.type_char(Key::X, 'x').is_empty());
        assert_eq!(o.ui.sources.as_ref().unwrap().portal.pass, "secretAx");
        o.press(Key::Tab, 4); // Test, Save, row 1, row 2
        o.press(Key::Enter, 1); // the second portal, the keyboard on its name
        o.press(Key::Tab, 3); // server, user, password
        assert!(o.frame_with(vec![ctrl_z()]).is_empty());
        assert_eq!(o.ui.sources.as_ref().unwrap().portal.pass, "otherB");
    }

    #[test]
    fn a_source_added_with_the_number_of_a_deleted_one_gets_none_of_its_undo_history() {
        let mut o = Overlay::new();
        o.sources = Sources::parse(
            "source=1;xtream;;ann:secretA@srv-a.example.net:8080\n\
             source=2;xtream;Tweede;ben:otherB@srv-b.example.net:8080\n\
             active=1\n",
        );
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 6); // user, password, Test, Save, row 1, row 2
        o.press(Key::Enter, 1); // the second portal, the keyboard on its name
        o.press(Key::Tab, 3); // server, user, password
        assert!(o.type_char(Key::X, 'x').is_empty());
        // The user deletes it, and the app shows the portal in use again ...
        o.sources = Sources::parse("source=1;xtream;;ann:secretA@srv-a.example.net:8080\n");
        o.the_app_answers(Editing::Saved(1));
        assert_eq!(o.frame(&[]), vec![]);
        // ... and later adds another source, which gets the number 2 again.
        o.sources = Sources::parse(
            "source=1;xtream;;ann:secretA@srv-a.example.net:8080\n\
             source=2;xtream;Derde;cor:thirdC@srv-c.example.net:8080\n\
             active=1\n",
        );
        o.press(Key::Tab, 7); // server, user, password, Test, Save, row 1, row 2
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 3); // server, user, password
        assert!(o.frame_with(vec![ctrl_z()]).is_empty());
        assert_eq!(o.ui.sources.as_ref().unwrap().portal.pass, "thirdC");
    }

    #[test]
    fn the_same_field_of_two_forms_has_two_ids() {
        let form = |editing| SourceForm::blank(editing, SourceKind::Xtream, Lang::En);
        let forms = [
            form(Editing::Saved(1)),
            form(Editing::Saved(2)),
            form(Editing::CommandLine),
            form(Editing::New),
            form(Editing::New),
        ];
        let fields = [
            SOURCE_NAME_ID,
            SOURCE_SERVER_ID,
            SOURCE_ADDRESS_ID,
            SOURCE_USER_ID,
            SOURCE_PASS_ID,
        ];
        let mut seen = std::collections::HashSet::new();
        for (i, form) in forms.iter().enumerate() {
            for field in fields {
                assert!(seen.insert(form.field_id(field)), "form {i}, {field}");
            }
        }
        // A form's field is the same in every frame.
        let again = forms[0].clone();
        assert_eq!(
            again.field_id(SOURCE_NAME_ID),
            forms[0].field_id(SOURCE_NAME_ID)
        );
    }

    #[test]
    fn a_default_name_in_any_language_follows_the_kind() {
        let name = |o: &Overlay| o.ui.sources.as_ref().unwrap().name.clone();
        let mut o = Overlay::new();
        o.lang = Lang::Nl;
        open_sources_pane(&mut o); // a new Xtream source, made in Dutch
        assert_eq!(name(&o), "Portaal");
        o.lang = Lang::En; // the language was switched in the meantime
        tab_back(&mut o); // the M3U radio
        assert_eq!(o.frame(&[Key::Space]), vec![]);
        assert_eq!(
            name(&o),
            "M3U list",
            "no name of its own: the default of the kind"
        );
        // The same for a list that turns into an Xtream source.
        let mut o = Overlay::new();
        o.lang = Lang::Nl;
        let link = "http://srv.example.net/get.php?username=u&password=p";
        o.ui.sources = Some(SourceForm {
            address: link.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::Nl)
        });
        open_sources_pane(&mut o);
        assert_eq!(name(&o), "M3U-lijst");
        o.lang = Lang::En;
        o.press(Key::Tab, 1); // Save
        assert_eq!(o.frame(&[Key::Enter]), vec![]); // the question
        o.press(Key::Tab, 1); // "As an Xtream source"
        o.press(Key::Enter, 1);
        assert_eq!(name(&o), "Portal");
    }

    #[test]
    fn the_question_goes_when_the_address_is_edited_and_asks_again_for_the_new_one() {
        let link = "http://srv.example.net/get.php?username=u&password=p";
        let question = t(Lang::En, T::XtreamLinkFound);
        let mut o = Overlay::new();
        o.ui.sources = Some(SourceForm {
            name: "Lijst".into(),
            address: link.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        });
        open_sources_pane(&mut o);
        o.press(Key::Tab, 1); // Save
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        assert!(o.says(question), "{:?}", o.painted);
        // The address is edited: the question about the old one goes.
        tab_back(&mut o); // the address
        o.frame_with(vec![typed("2")]); // password=p2
        o.frame(&[]);
        assert!(!o.says(question), "{:?}", o.painted);
        // Save asks about the new address, which is still a get.php link.
        o.press(Key::Tab, 1);
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        o.frame(&[]);
        assert!(o.says(question), "{:?}", o.painted);
        o.press(Key::Tab, 1); // "As an Xtream source"
        o.press(Key::Enter, 1);
        let form = o.ui.sources.as_ref().unwrap();
        assert_eq!(form.portal.pass, "p2", "the credentials of the new address");
    }

    #[test]
    fn a_saved_get_php_list_is_renamed_without_being_asked_about_its_address_again() {
        let link = "http://srv.example.net/get.php?username=u&password=p";
        let mut o = Overlay::new();
        o.sources = Sources::parse(&format!(
            "source=1;xtream;;bob:secret@srv.example.net:8080\n\
             source=2;m3u;Gratis;{link}\n\
             active=1\n"
        ));
        o.active_source = Some(1);
        open_sources_pane(&mut o);
        o.press(Key::Tab, 6); // user, password, Test, Save, row 1, row 2
        o.press(Key::Enter, 1); // the list, kept as a list; the keyboard on its name
        assert!(o.type_char(Key::X, 'x').is_empty()); // renamed
        o.press(Key::Tab, 2); // address, Save
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SaveSource(SourceDraft {
                editing: Editing::Saved(2),
                kind: SourceKind::M3u,
                name: "Gratisx".into(),
                address: link.into(),
            })]
        );
    }

    #[test]
    fn a_relative_file_path_is_kept_absolute() {
        // `cargo test` runs in rust/, where Cargo.toml is.
        let list = |address: &str| SourceForm {
            name: "Lijst".into(),
            address: address.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        };
        let draft = list(" Cargo.toml ").draft().unwrap();
        let path = std::path::Path::new(&draft.address);
        assert!(path.is_absolute(), "{}", draft.address);
        assert!(path.is_file(), "{}", draft.address);
        assert!(path.ends_with("Cargo.toml"), "{}", draft.address);
        // A path that is absolute already, and a URL, are kept as they are.
        let here = path.to_path_buf().display().to_string();
        assert_eq!(list(&here).draft().unwrap().address, here);
        let url = "https://lists.example/tv.m3u";
        assert_eq!(list(url).draft().unwrap().address, url);
    }

    #[test]
    fn a_path_copied_as_path_in_explorer_loses_its_quotes() {
        // Explorer's "Copy as path" (Ctrl+Shift+C) puts a path in quotes.
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("mijn lijst.m3u");
        std::fs::write(&file, "#EXTM3U\n").unwrap();
        let path = file.display().to_string();
        let list = |address: &str| SourceForm {
            name: "Lijst".into(),
            address: address.into(),
            ..SourceForm::blank(Editing::New, SourceKind::M3u, Lang::En)
        };
        for pasted in [format!("\"{path}\""), format!("  \"{path}\" \r\n")] {
            let draft = list(&pasted).draft();
            assert_eq!(draft.map(|d| d.address), Ok(path.clone()), "{pasted:?}");
        }
        let url = "https://lists.example/tv.m3u";
        let draft = list(&format!("\"{url}\"")).draft();
        assert_eq!(draft.map(|d| d.address), Ok(url.to_owned()));
        // One pair only: what is left of two is no path.
        assert_eq!(
            list(&format!("\"\"{path}\"\"")).draft().err(),
            Some(T::SourceAddressInvalid)
        );
        // A portal's link in quotes still gets the question.
        let mut form = list("\"http://srv.example:8080/get.php?username=u&password=p\"");
        let mut actions = Vec::new();
        form.save(&mut actions, &Sources::default());
        assert_eq!(form.xtream_offer.as_deref(), Some("u:p@srv.example:8080"));
        assert!(actions.is_empty(), "the question comes first");
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
            // The overlay is still there: Esc cancels the dialog, the next one
            // leaves the section, only the third closes.
            assert_eq!(o.frame(&[Key::Escape]), vec![]);
            assert_eq!(o.frame(&[Key::Escape]), vec![]);
            assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
        }
    }

    #[test]
    fn coming_back_to_a_dialog_left_open_puts_the_focus_in_its_first_field() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        // Shift+Tab walks back over the list rows: News, then Adult. Enter on
        // the Adult row enters the section again, with the dialog still open.
        o.frame_with(vec![shift_tab()]);
        o.frame_with(vec![shift_tab()]);
        o.press(Key::Enter, 1);
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
        // Once more, with four digits: the dialog is still there, empty, with the
        // keyboard in its field.
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
        // Again, and this time twice the same: both fields are empty and the
        // keyboard is in the first, the dialog did not close.
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
    fn after_locking_the_focus_is_on_the_button_that_replaced_lock() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.adult_unlocked = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Lock has the focus
        assert_eq!(o.frame(&[Key::Enter]), vec![SettingsAction::Lock]);
        // The app locked: the first button is Unlock now, and the keyboard can
        // carry on from it.
        o.adult_unlocked = false;
        assert_eq!(o.frame(&[]), vec![]);
        o.press(Key::Enter, 1);
        assert!(
            matches!(o.ui.adult, AdultDialog::Unlock { .. }),
            "{:?}",
            o.ui.adult
        );
    }

    #[test]
    fn after_locking_while_blocked_the_focus_goes_to_forgot_pin() {
        // Wrong old PINs in Change PIN can block PIN input while unlocked. Lock
        // then replaces itself by a disabled Unlock, which keeps no focus.
        let mut o = Overlay::new();
        o.has_pin = true;
        o.adult_unlocked = true;
        o.pin_wait_secs = Some(30);
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Lock has the focus
        assert_eq!(o.frame(&[Key::Enter]), vec![SettingsAction::Lock]);
        o.adult_unlocked = false;
        assert_eq!(o.frame(&[]), vec![]);
        o.press(Key::Enter, 1);
        assert!(
            matches!(o.ui.adult, AdultDialog::Forgot { .. }),
            "{:?}",
            o.ui.adult
        );
    }

    #[test]
    fn an_action_leaves_no_stale_note_to_mask_the_apps_own() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.adult_unlocked = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Lock has the focus
        o.press(Key::Tab, 1); // Change PIN
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("12");
        // On to the last field, where Enter is OK: too short, so the dialog stays
        // with a note of its own, and the current PIN is typed again.
        o.press(Key::Enter, 2);
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert_eq!(o.ui.local_note, Some(T::PinFormat));
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        // Sent now, it is the app's to answer: the old note must not stay in
        // front of the one it sets.
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ChangePin {
                old: "1234".into(),
                new: "5678".into()
            }]
        );
        assert_eq!(o.ui.local_note, None);
    }

    #[test]
    fn while_pin_input_is_blocked_the_focus_goes_to_the_one_button_that_works() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.pin_wait_secs = Some(30);
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Unlock and Change PIN are disabled
        o.press(Key::Enter, 1); // Forgot PIN has the focus and opens its dialog
        assert!(
            matches!(o.ui.adult, AdultDialog::Forgot { .. }),
            "{:?}",
            o.ui.adult
        );
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("secret");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ResetPin("secret".into())]
        );
    }

    #[test]
    fn unlock_and_change_pin_cannot_be_reached_while_pin_input_is_blocked() {
        // Shift+Tab from Forgot PIN skips what is disabled: back to the list rows.
        // Enter there opens a section, never a dialog.
        for steps in 1..=2 {
            let mut o = Overlay::new();
            o.has_pin = true;
            o.pin_wait_secs = Some(30);
            o.press(Key::ArrowDown, 3); // Adult
            o.press(Key::Enter, 1); // into it: Forgot PIN has the focus
            for _ in 0..steps {
                o.frame_with(vec![shift_tab()]);
            }
            o.press(Key::Enter, 1);
            assert!(
                matches!(o.ui.adult, AdultDialog::Idle),
                "{steps} step(s) back: {:?}",
                o.ui.adult
            );
        }
    }

    #[test]
    fn enter_in_the_first_pin_field_moves_on_to_the_repeat() {
        let mut o = Overlay::new();
        o.open_adult_dialog(); // no PIN yet: the dialog chooses one
        o.type_text("1234");
        o.press(Key::Enter, 1); // on to the repeat
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]), // in the last field Enter is OK
            vec![SettingsAction::SetPin("1234".into())]
        );
    }

    #[test]
    fn enter_walks_through_the_three_fields_of_changing_the_pin() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Unlock has the focus
        o.press(Key::Tab, 1); // Change PIN
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        o.press(Key::Enter, 1); // the new PIN
        o.type_text("5678");
        o.press(Key::Enter, 1); // its repeat
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
    fn esc_cancels_an_open_pin_dialog_and_what_was_typed_does_not_come_back() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        // No action, and the keyboard stays in the section.
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
        // The first button has the focus: Enter opens a fresh dialog.
        o.press(Key::Enter, 1);
        assert_eq!(first_field(&o.ui), "");
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("1234".into())]
        );
    }

    #[test]
    fn esc_backs_out_one_level_at_a_time_from_an_open_dialog() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        assert_eq!(o.frame(&[Key::Escape]), vec![]); // the dialog
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
        assert_eq!(o.frame(&[Key::Escape]), vec![]); // the section, back to the list
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn esc_in_another_section_leaves_a_dialog_hidden_behind_it_alone() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        o.frame_with(vec![shift_tab()]); // back to the last list row: News
        o.press(Key::Enter, 1); // its section opens, the dialog stays open behind it
        assert!(matches!(o.ui.adult, AdultDialog::Unlock { .. }));
        // Esc is the News section's: back to the list, where `o` closes the overlay.
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert_eq!(o.frame(&[Key::O]), vec![SettingsAction::Close]);
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

    #[test]
    fn the_line_breaks_of_a_pasted_portal_password_do_not_reach_the_pin_reset() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3); // Adult
        o.press(Key::Enter, 1); // into it: Unlock has the focus
        o.press(Key::Tab, 2); // Change PIN, then Forgot PIN?
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.frame_with(vec![egui::Event::Paste("se\r\ncret\n".into())]);
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ResetPin("secret".into())]
        );
    }

    // ---- the PIN dialogs stay open on a PIN that does not go through ----

    #[test]
    fn a_wrong_pin_leaves_the_dialog_open_empty_and_focused_with_the_apps_note() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("1234".into())]
        );
        o.app_turns_the_pin_down(T::PinWrong);
        assert!(matches!(o.ui.adult, AdultDialog::Unlock { .. }));
        assert_eq!(first_field(&o.ui), "");
        assert!(o.says(t(Lang::En, T::PinWrong)), "{:?}", o.painted);
        // The keyboard is in the field: the next try is typed at once.
        o.type_text("4321");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("4321".into())]
        );
    }

    #[test]
    fn a_right_pin_closes_the_dialog_and_the_keyboard_goes_on_from_the_buttons() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("1234".into())]
        );
        // The app unlocked: its note says so, and the first button is Lock now.
        o.adult_unlocked = true;
        o.adult_note = Some(T::Unlocked);
        assert_eq!(o.frame(&[]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.frame(&[Key::Enter]), vec![SettingsAction::Lock]);
    }

    #[test]
    fn a_pin_of_the_wrong_shape_stays_in_the_dialog_with_the_local_note_and_an_empty_field() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("123");
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Unlock { .. }));
        assert_eq!(o.ui.local_note, Some(T::PinFormat));
        assert_eq!(first_field(&o.ui), "");
        assert_eq!(o.frame(&[]), vec![]);
        assert!(o.says(t(Lang::En, T::PinFormat)), "{:?}", o.painted);
        // Typed again, with four digits, it goes to the app and the note is gone.
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::Unlock("1234".into())]
        );
        assert_eq!(o.ui.local_note, None);
    }

    #[test]
    fn two_new_pins_that_differ_clear_both_fields_and_the_keyboard_goes_to_the_first() {
        let mut o = Overlay::new();
        o.open_adult_dialog(); // no PIN yet: the dialog chooses one
        o.type_text("1234");
        o.press(Key::Tab, 1); // the repeat
        o.type_text("1235");
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert_eq!(o.ui.local_note, Some(T::PinMismatch));
        assert_eq!(o.frame(&[]), vec![]);
        match &o.ui.adult {
            AdultDialog::SetPin { first, second } => {
                assert_eq!((first.as_str(), second.as_str()), ("", ""))
            }
            other => panic!("the dialog should stay open: {other:?}"),
        }
        // Both again, from the first field.
        o.type_text("1234");
        o.press(Key::Enter, 1); // on to the repeat
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SetPin("1234".into())]
        );
    }

    #[test]
    fn changing_the_pin_clears_what_is_wrong_and_puts_the_keyboard_on_the_first_of_it() {
        // Where the keyboard lands after the attempt (0 = the current PIN, 1 = the
        // new one) and what is left in the three fields.
        let cases = [
            // The current PIN is not 4 digits: only it goes.
            ("123", "5678", "5678", 0, ["", "5678", "5678"]),
            // The new PIN is not 4 digits: it and its repeat go.
            ("1234", "567", "567", 1, ["1234", "", ""]),
            // The two differ: both go.
            ("1234", "5678", "5679", 1, ["1234", "", ""]),
            // Everything wrong: everything goes, the keyboard on the first.
            ("1", "2", "3", 0, ["", "", ""]),
        ];
        for (old, new, again, lands, left) in cases {
            let mut o = Overlay::new();
            o.has_pin = true;
            o.press(Key::ArrowDown, 3); // Adult
            o.press(Key::Enter, 1);
            o.press(Key::Tab, 1); // Change PIN
            o.press(Key::Enter, 1);
            assert_eq!(o.frame(&[]), vec![]);
            o.type_text(old);
            o.press(Key::Enter, 1);
            o.type_text(new);
            o.press(Key::Enter, 1);
            o.type_text(again);
            assert_eq!(o.frame(&[Key::Enter]), vec![], "{old} {new} {again}");
            assert_eq!(o.frame(&[]), vec![]);
            let fields = match &o.ui.adult {
                AdultDialog::Change { old, first, second } => {
                    [old.clone(), first.clone(), second.clone()]
                }
                other => panic!("{old} {new} {again}: the dialog should stay open: {other:?}"),
            };
            assert_eq!(fields, left.map(String::from), "{old} {new} {again}");
            // A digit typed now lands in the field the keyboard is on.
            o.type_text("9");
            let fields = match &o.ui.adult {
                AdultDialog::Change { old, first, second } => {
                    [old.clone(), first.clone(), second.clone()]
                }
                other => panic!("{other:?}"),
            };
            assert_eq!(fields[lands], "9", "{old} {new} {again}");
        }
    }

    #[test]
    fn a_pin_the_app_turns_down_clears_every_field_of_changing_it() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 1); // Change PIN
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("1234");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ChangePin {
                old: "1234".into(),
                new: "5678".into()
            }]
        );
        // The current PIN was wrong: the app says so and changes nothing.
        o.app_turns_the_pin_down(T::PinWrong);
        match &o.ui.adult {
            AdultDialog::Change { old, first, second } => {
                assert_eq!(
                    (old.as_str(), first.as_str(), second.as_str()),
                    ("", "", "")
                )
            }
            other => panic!("the dialog should stay open: {other:?}"),
        }
        o.type_text("9");
        assert!(matches!(&o.ui.adult, AdultDialog::Change { old, .. } if old == "9"));
        // The right one, once the app says the PIN changed, closes the dialog.
        o.type_text("999");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        o.press(Key::Enter, 1);
        o.type_text("5678");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ChangePin {
                old: "9999".into(),
                new: "5678".into()
            }]
        );
        o.adult_note = Some(T::PinChanged);
        assert_eq!(o.frame(&[]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
    }

    #[test]
    fn a_portal_password_the_app_turns_down_stays_in_the_forgot_pin_dialog() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.press(Key::ArrowDown, 3);
        o.press(Key::Enter, 1);
        o.press(Key::Tab, 2); // Change PIN, then Forgot PIN?
        o.press(Key::Enter, 1);
        assert_eq!(o.frame(&[]), vec![]);
        o.type_text("nope");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ResetPin("nope".into())]
        );
        o.app_turns_the_pin_down(T::WrongPortalPassword);
        assert!(matches!(&o.ui.adult, AdultDialog::Forgot { password } if password.is_empty()));
        assert!(
            o.says(t(Lang::En, T::WrongPortalPassword)),
            "{:?}",
            o.painted
        );
        // The right one clears the PIN, and the dialog goes.
        o.type_text("secret");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::ResetPin("secret".into())]
        );
        o.has_pin = false;
        o.adult_note = Some(T::PinCleared);
        assert_eq!(o.frame(&[]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
    }

    #[test]
    fn choosing_the_first_pin_closes_the_dialog_once_the_app_has_it() {
        let mut o = Overlay::new();
        o.open_adult_dialog();
        o.type_text("1234");
        o.press(Key::Tab, 1);
        o.type_text("1234");
        assert_eq!(
            o.frame(&[Key::Enter]),
            vec![SettingsAction::SetPin("1234".into())]
        );
        // The app stored it and unlocked.
        o.has_pin = true;
        o.adult_unlocked = true;
        o.adult_note = Some(T::Unlocked);
        assert_eq!(o.frame(&[]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
    }

    #[test]
    fn three_wrong_pins_block_the_dialog_which_stays_open_with_its_ok_disabled() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        for _ in 0..3 {
            o.type_text("0000");
            assert_eq!(
                o.frame(&[Key::Enter]),
                vec![SettingsAction::Unlock("0000".into())]
            );
            o.app_turns_the_pin_down(T::PinWrong);
        }
        // The third one blocked PIN input; the dialog is still there, empty.
        o.pin_wait_secs = Some(30);
        assert_eq!(o.frame(&[]), vec![]);
        assert!(matches!(&o.ui.adult, AdultDialog::Unlock { pin } if pin.is_empty()));
        assert!(
            o.says(&tr(Lang::En, T::PinBlocked, &["30"])),
            "{:?}",
            o.painted
        );
        // Four digits and Enter go nowhere while it lasts ...
        o.type_text("1234");
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        // ... and the dialog can still be left with Esc.
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
    }

    #[test]
    fn esc_still_cancels_a_dialog_that_stayed_open_and_takes_its_note_with_it() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        assert_eq!(o.frame(&[Key::Enter]), vec![]); // too short: the dialog stays
        assert_eq!(o.ui.local_note, Some(T::PinFormat));
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
        assert_eq!(o.ui.local_note, None);
        assert_eq!(o.frame(&[]), vec![]);
        // The pane is still the keyboard's: a second Esc leaves it, a third closes.
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn cancel_takes_the_local_note_with_it() {
        let mut o = Overlay::new();
        o.has_pin = true;
        o.open_adult_dialog();
        o.type_text("12");
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert_eq!(o.ui.local_note, Some(T::PinFormat));
        assert_eq!(o.frame(&[]), vec![]); // the keyboard is back in the field
        o.press(Key::Tab, 2); // OK, then Cancel
        assert_eq!(o.frame(&[Key::Enter]), vec![]);
        assert!(matches!(o.ui.adult, AdultDialog::Idle), "{:?}", o.ui.adult);
        assert_eq!(o.ui.local_note, None);
    }

    fn news_rule(channel: &str) -> NewsRule {
        NewsRule {
            channel: channel.into(),
            patterns: "*".into(),
            label: String::new(),
            skip_secs: 0,
        }
    }

    /// 12 rules, as someone who follows many channels has.
    fn twelve_rules() -> Vec<NewsRule> {
        (1..=12)
            .map(|i| news_rule(&format!("Channel {i}")))
            .collect()
    }

    /// The rules `actions` carry, which must be exactly one `SetNewsRules`.
    fn one_set_news_rules(actions: Vec<SettingsAction>) -> Vec<NewsRule> {
        match <[SettingsAction; 1]>::try_from(actions) {
            Ok([SettingsAction::SetNewsRules(rules)]) => rules,
            other => panic!("expected one SetNewsRules, got {other:?}"),
        }
    }

    fn channels(rules: &[NewsRule]) -> Vec<&str> {
        rules.iter().map(|r| r.channel.as_str()).collect()
    }

    #[test]
    fn keys_typed_into_a_news_rule_stay_in_its_field_and_esc_backs_out_one_level() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("")]);
        o.open_news_pane();
        let mut rules = Vec::new();
        for (key, c) in [
            (Key::N, 'n'),
            (Key::O, 'o'),
            (Key::S, 's'),
            (Key::Space, ' '),
            (Key::O, 'o'),
        ] {
            // Each key changes the rule and nothing else: `o` is a letter here,
            // not the key that closes the overlay.
            rules = one_set_news_rules(o.type_char(key, c));
        }
        assert_eq!(channels(&rules), ["nos o"]);
        // Esc leaves the field for the list, and only the next one closes.
        assert_eq!(o.frame(&[Key::Escape]), vec![]);
        assert_eq!(o.frame(&[Key::Escape]), vec![SettingsAction::Close]);
    }

    #[test]
    fn typing_in_the_other_fields_of_a_rule_changes_that_rule_only() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("NPO 1"), news_rule("CNN")]);
        o.open_news_pane();
        o.press(Key::Tab, 1); // the patterns, which hold "*"
        let rules = one_set_news_rules(o.type_char(Key::J, 'j'));
        assert_eq!(rules[0].patterns, "*j");
        o.press(Key::Tab, 1); // the label
        let rules = one_set_news_rules(o.type_char(Key::N, 'N'));
        assert_eq!(rules[0].label, "N");
        assert_eq!(rules[1], news_rule("CNN"));
        assert_eq!(rules[0].channel, "NPO 1");
    }

    #[test]
    fn what_was_typed_stays_while_the_overlay_is_open() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("")]);
        o.open_news_pane();
        o.type_char(Key::A, 'a');
        // To the list, to another section and back: the rule is as it was left,
        // although the app has not handed the new rules back in the view.
        o.frame(&[Key::Escape]);
        o.press(Key::ArrowUp, 1);
        o.press(Key::ArrowDown, 1);
        o.press(Key::Enter, 1);
        let rules = one_set_news_rules(o.type_char(Key::B, 'b'));
        assert_eq!(channels(&rules), ["ab"]);
    }

    #[test]
    fn the_overlay_says_when_the_news_section_is_the_one_shown() {
        let mut o = Overlay::new();
        assert!(!o.ui.shows_news());
        o.press(Key::ArrowDown, 3); // Adult
        assert!(!o.ui.shows_news());
        o.press(Key::ArrowDown, 1); // News
        assert!(o.ui.shows_news());
        o.press(Key::ArrowUp, 1);
        assert!(!o.ui.shows_news());
    }

    #[test]
    fn the_found_lines_stay_blank_until_the_lookup_has_come_in() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("NPO 1"), news_rule("CNN")]);
        // The app looks the channels up only once the section is shown, so the
        // first frame has no lines: it must not say "not found" for them.
        o.open_news_pane();
        let says = |o: &Overlay, text: &str| o.painted.iter().any(|p| p == text);
        assert!(!says(&o, "not found"), "{:?}", o.painted);
        let blank = o.overlay_rect().height();
        // A different number of lines than rules is no answer either.
        o.news_found = vec![None];
        assert_eq!(o.frame(&[]), vec![]);
        assert!(!says(&o, "not found"), "{:?}", o.painted);
        // The lookup comes in: the lines show, in the room that was kept for them.
        o.news_found = vec![Some("NL | NPO1 HD".into()), None];
        assert_eq!(o.frame(&[]), vec![]);
        assert!(says(&o, "found: NL | NPO1 HD") && says(&o, "not found"));
        assert_eq!(o.overlay_rect().height(), blank);
    }

    #[test]
    fn with_no_saved_rules_the_news_section_starts_from_the_defaults() {
        let mut o = Overlay::new();
        assert_eq!(o.settings.news_rules, None);
        o.open_news_pane();
        let rules = one_set_news_rules(o.type_char(Key::X, 'x'));
        assert_eq!(rules.len(), default_rules().len());
        assert_eq!(rules[0].channel, "NPO 1x");
        assert_eq!(rules[1..], default_rules()[1..]);
    }

    #[test]
    fn without_rules_the_news_section_starts_on_the_add_button_and_space_adds_one() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(Vec::new());
        o.open_news_pane();
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules, [news_rule("")]);
    }

    #[test]
    fn a_rule_just_added_takes_the_keyboard_in_its_channel_field() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("CNN")]);
        o.open_news_pane();
        o.press(Key::Tab, 5); // past the rule's five controls: the Add button
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(channels(&rules), ["CNN", ""]);
        assert_eq!(o.frame(&[]), vec![]); // the new rule is drawn and takes the focus
        let rules = one_set_news_rules(o.type_char(Key::C, 'c'));
        assert_eq!(channels(&rules), ["CNN", "c"]);
    }

    #[test]
    fn removing_a_rule_leaves_the_keyboard_on_the_rule_that_took_its_place() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("A"), news_rule("B"), news_rule("C")]);
        o.open_news_pane();
        // B's channel field is the 6th control; its remove button the 10th.
        o.press(Key::Tab, 9);
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(channels(&rules), ["A", "C"]);
        assert_eq!(o.frame(&[]), vec![]);
        let rules = one_set_news_rules(o.type_char(Key::Z, 'z'));
        assert_eq!(channels(&rules), ["A", "Cz"]);
    }

    #[test]
    fn a_click_on_remove_leaves_the_keyboard_alone() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("A"), news_rule("B"), news_rule("C")]);
        o.open_news_pane();
        o.press(Key::Tab, 9); // B's remove button ...
        let remove = o.focused_response().rect;
        o.press(Key::Tab, 1); // ... and on to C's channel field, so Remove has no focus
        let rules = one_set_news_rules(o.click_at(remove.center()));
        assert_eq!(channels(&rules), ["A", "C"]);
        assert_eq!(o.frame(&[]), vec![]);
        // The mouse took the focus away from C's field (a click elsewhere does
        // that) and did not hand it to the rule that moved up: a key typed next
        // goes nowhere, `o` being a letter and not the key that closes.
        assert_eq!(o.ctx.memory(|m| m.focused()), None);
        assert_eq!(o.type_char(Key::O, 'o'), vec![]);
    }

    #[test]
    fn removing_the_last_rule_leaves_the_keyboard_on_the_add_button() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("A")]);
        o.open_news_pane();
        o.press(Key::Tab, 4); // the remove button
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules, []);
        assert_eq!(o.frame(&[]), vec![]);
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules, [news_rule("")]);
    }

    #[test]
    fn restore_defaults_brings_back_the_default_rules() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(Vec::new());
        o.open_news_pane(); // the Add button has the focus
        o.press(Key::Tab, 1); // Restore defaults
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules, default_rules());
    }

    #[test]
    fn each_news_rule_says_what_it_found_and_the_section_names_its_columns() {
        let mut o = Overlay::new();
        o.settings.news_rules = Some(vec![news_rule("NPO 1"), news_rule("Nowhere")]);
        o.news_found = vec![Some("NL | NPO1 HD".into()), None];
        o.open_news_pane();
        for text in [
            "found: NL | NPO1 HD",
            "not found",
            t(Lang::En, T::NewsColChannel),
            t(Lang::En, T::NewsColPatterns),
            t(Lang::En, T::NewsColLabel),
            t(Lang::En, T::NewsColSkip),
            t(Lang::En, T::NewsPatternsHelp),
        ] {
            assert!(
                o.painted.iter().any(|p| p == text),
                "{text:?} in {:?}",
                o.painted
            );
        }
    }

    #[test]
    fn the_overlay_is_as_wide_as_its_window_less_a_margin_within_limits() {
        assert_eq!(overlay_width(640.0), 600.0); // the smallest window the app allows
        assert_eq!(overlay_width(700.0), 660.0);
        assert_eq!(overlay_width(300.0), 480.0); // never narrower than 480
        assert_eq!(overlay_width(1920.0), 760.0); // never wider than 760
    }

    #[test]
    fn the_news_section_never_makes_the_overlay_bigger_than_its_window() {
        // The smallest window the app allows, and some bigger ones; German has
        // the longest texts.
        for (width, height) in [(640.0, 360.0), (800.0, 450.0), (1280.0, 720.0)] {
            for lang in Lang::ALL {
                let mut o = Overlay::in_window(width, height);
                o.lang = lang;
                o.settings.news_rules = Some(twelve_rules());
                o.open_news_pane();
                assert_eq!(o.frame(&[]), vec![]);
                let (screen, overlay) = (o.screen_rect(), o.overlay_rect());
                let name = format!("{width}x{height}, {lang:?}");
                assert!(
                    screen.contains_rect(overlay),
                    "{name}: {overlay:?} is not inside {screen:?}"
                );
                // The rules fit the pane: they do not widen the overlay.
                let designed = overlay_width(width) + 2.0 * OVERLAY_FRAME_MARGIN;
                assert!(
                    overlay.width() <= designed + 0.5,
                    "{name}: {} wide, designed for {designed}",
                    overlay.width()
                );
            }
        }
    }

    #[test]
    fn tab_scrolls_a_rule_below_the_fold_into_view() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        // From the first rule's channel field to the last one's: 11 rules of
        // five controls each.
        o.press(Key::Tab, 55);
        assert_eq!(o.frame(&[]), vec![]); // the scroll shows in the frame after the Tab
        o.assert_focus_in_view("the last rule's channel field");
        // And it is the last rule's channel field.
        let rules = one_set_news_rules(o.type_char(Key::X, 'x'));
        assert_eq!(rules[11].channel, "Channel 12x");
    }

    #[test]
    fn a_rule_added_below_the_fold_is_brought_into_view() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        o.press(Key::Tab, 60); // the Add button, below all 12 rules
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules.len(), 13);
        assert_eq!(o.frame(&[]), vec![]); // the new rule is drawn and takes the focus
        assert_eq!(o.frame(&[]), vec![]); // the list scrolls to it
        o.assert_focus_in_view("the new rule's channel field");
        let rules = one_set_news_rules(o.type_char(Key::X, 'x'));
        assert_eq!(rules[12].channel, "x");
    }

    #[test]
    fn a_rule_removed_with_the_keyboard_below_the_fold_leaves_the_focus_in_view() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        o.press(Key::Tab, 59); // the last rule's remove button
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules.len(), 11);
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.frame(&[]), vec![]);
        o.assert_focus_in_view("the channel field that took the place");
        let rules = one_set_news_rules(o.type_char(Key::X, 'x'));
        assert_eq!(rules[10].channel, "Channel 11x");
    }

    /// What a headless frame draws with `marker_scroll_area`: a button, a scroll
    /// area 100 tall holding `content_height` of stuff scrolled to `offset` (or
    /// ten buttons if `buttons`), its marker, and a button; `keys` are pressed in
    /// it. Returns the view and the rects the marker painted.
    fn marker_frame(
        ctx: &egui::Context,
        content_height: f32,
        offset: f32,
        buttons: bool,
        keys: &[Key],
    ) -> (egui::Rect, Vec<egui::Rect>) {
        let mut view = egui::Rect::NOTHING;
        let events = keys.iter().map(|&key| key_event(key, true)).collect();
        let output = run_events_in(ctx, SCREEN, events, |ctx| {
            egui::Area::new(egui::Id::new("__marker__"))
                .sense(egui::Sense::hover())
                .fixed_pos(egui::pos2(50.0, 50.0))
                .show(ctx, |ui| {
                    let _ = ui.button("before");
                    let scrolled = marker_scroll_area("__marker_rows__")
                        .max_height(100.0)
                        .vertical_scroll_offset(offset)
                        .show(ui, |ui| {
                            if buttons {
                                for i in 0..10 {
                                    let _ = ui.button(format!("row {i}"));
                                }
                            } else {
                                ui.allocate_space(egui::vec2(80.0, content_height));
                            }
                        });
                    view = scrolled.inner_rect;
                    paint_scroll_marker(ui, &scrolled);
                    let _ = ui.button("after");
                });
        });
        (view, painted_markers(&output))
    }

    #[test]
    fn a_scroll_area_with_a_marker_has_no_tab_stops_of_its_own() {
        // 12 buttons, ten of them in a scroll area that cannot show them all: a
        // scroll bar or a drag area would be a 13th stop.
        let ctx = egui::Context::default();
        marker_frame(&ctx, 0.0, 0.0, true, &[]);
        marker_frame(&ctx, 0.0, 0.0, true, &[]);
        let mut stops = std::collections::HashSet::new();
        for _ in 0..30 {
            marker_frame(&ctx, 0.0, 0.0, true, &[Key::Tab]);
            marker_frame(&ctx, 0.0, 0.0, true, &[]); // the focus moves a frame later
            stops.extend(ctx.memory(|m| m.focused()));
        }
        assert_eq!(stops.len(), 12);
    }

    #[test]
    fn the_scroll_marker_shows_which_part_of_the_content_is_in_view() {
        let near = |a: f32, b: f32| (a - b).abs() < 0.01;
        // 300 of content in a view 100 tall: the handle is a third of the view ...
        for (offset, passed) in [(0.0, 0.0), (100.0, 0.5), (200.0, 1.0), (999.0, 1.0)] {
            let ctx = egui::Context::default();
            marker_frame(&ctx, 300.0, offset, false, &[]);
            let (view, bars) = marker_frame(&ctx, 300.0, offset, false, &[]);
            assert_eq!(bars.len(), 1, "offset {offset}");
            let bar = bars[0];
            let length = 100.0 / 3.0;
            assert!(near(bar.height(), length), "offset {offset}: {bar:?}");
            // ... that moves down the view as the content moves up ...
            let top = view.top() + (view.height() - length) * passed;
            assert!(near(bar.top(), top), "offset {offset}: {bar:?} in {view:?}");
            // ... in the room beside the content (80 wide), which is as wide as
            // the view: the area shrinks to fit it, though it was offered more.
            assert!(view.width() > 1000.0);
            assert!(near(bar.right(), view.left() + 80.0 + SCROLL_MARKER_ROOM));
        }
        // Never shorter than a thumb, however much content there is.
        let ctx = egui::Context::default();
        marker_frame(&ctx, 100_000.0, 0.0, false, &[]);
        let (_, bars) = marker_frame(&ctx, 100_000.0, 0.0, false, &[]);
        assert!(near(bars[0].height(), SCROLL_MARKER_MIN_LENGTH), "{bars:?}");
    }

    #[test]
    fn the_scroll_marker_stays_away_when_everything_fits() {
        let ctx = egui::Context::default();
        marker_frame(&ctx, 80.0, 0.0, false, &[]);
        let (_, bars) = marker_frame(&ctx, 80.0, 0.0, false, &[]);
        assert_eq!(bars, []);
    }

    #[test]
    fn the_news_rules_show_where_they_are_scrolled_to() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        assert_eq!(o.frame(&[]), vec![]);
        // One marker, in the pane and not over the rules, at the top of the list.
        assert_eq!(o.markers.len(), 1, "{:?}", o.markers);
        let at_top = o.markers[0];
        let pane_right = o.overlay_rect().right() - OVERLAY_FRAME_MARGIN;
        assert!(
            at_top.right() <= pane_right + 0.5,
            "{at_top:?} in {pane_right}"
        );
        let rules_right = o.focused_response().rect.right();
        assert!(at_top.left() > rules_right, "{at_top:?} over the rules");
        // Tab to the last rule: the list scrolls, and the marker with it.
        o.press(Key::Tab, 55);
        assert_eq!(o.frame(&[]), vec![]);
        assert_eq!(o.markers.len(), 1);
        assert!(o.markers[0].top() > at_top.top() + 50.0, "{:?}", o.markers);
        // Few rules fit: nothing to show.
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(vec![news_rule("A")]);
        o.open_news_pane();
        assert_eq!(o.markers, []);
    }

    #[test]
    fn the_wheel_scrolls_the_news_rules() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        let before = o.focused_response().rect; // the first rule's channel field
        o.frame_with(vec![egui::Event::PointerMoved(before.center())]);
        o.frame_with(vec![egui::Event::Scroll(egui::vec2(0.0, -120.0))]);
        // The wheel's delta is spread over a few frames.
        for _ in 0..10 {
            assert_eq!(o.frame(&[]), vec![]);
        }
        let after = o.focused_response().rect;
        assert!(
            after.top() < before.top() - 100.0,
            "the rules moved from {before:?} to {after:?}"
        );
    }

    #[test]
    fn the_scrolling_list_adds_no_tab_stop_of_its_own() {
        let mut o = Overlay::in_window(640.0, 360.0);
        o.settings.news_rules = Some(twelve_rules());
        o.open_news_pane();
        // Backwards from the first field is the News row of the list, not
        // something the scroll area put in front of the rules. (egui moves the
        // focus back one frame after the key.)
        o.frame_with(vec![shift_tab()]);
        assert_eq!(o.frame(&[]), vec![]);
        let row = o.ctx.memory(|m| m.focused()).expect("a row has the focus");
        assert_eq!(o.ui.list_ids.last(), Some(&row));
        // 12 rules of five controls: the 60th Tab from the first field is the Add
        // button, as it would be without a scroll area and its scroll bar.
        o.press(Key::Tab, 1); // back to the first field
        o.press(Key::Tab, 60);
        let rules = one_set_news_rules(o.frame(&[Key::Space]));
        assert_eq!(rules.len(), 13);
    }
}
