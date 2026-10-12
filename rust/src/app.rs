use crate::args::{parse_xtream_creds, CredsError, XtreamCreds};
use crate::catalog::{fallback_variant, normalize_channel_name, quality_label, quality_rank, zap_position, CatalogStatus, CatalogStore};
use crate::epg::{Epg, EpgEntry};
use crate::favorites::{Entry as FavoriteEntry, Favorites};
use crate::i18n::{Lang, T};
use crate::pin::{PinGate, PinHash};
use crate::playback_health::{PlaybackHealth, ReloadBudget};
use crate::relay::Relay;
use crate::player::{Cmd, Event, FrameFence, PlayerHandle, RgbaFrame};
use crate::presets::Presets;
use crate::portal::xtream::ConnCheck;
use crate::portal::{xtream::XtreamPortal, LiveChannel, Portal};
use crate::search::{self, ItemKind, SearchItem};
use crate::settings::Settings;
use crate::settings_ui::{conn_rich_text, ConnState, SettingsAction, SettingsUi, SettingsView};
use crate::shortcuts;
use crate::storage::Storage;
use crate::zap::ZapPacer;
use egui::{Color32, ColorImage, Key, TextureHandle, TextureOptions};
use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

/// Persisted "what was playing" so startup can resume it. `live` flags whether
/// the stall watchdog should arm on resume (live channels yes, VOD/catch-up no).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct LastWatched {
    url: String,
    live: bool,
    /// Adult content never resumes (every start is locked). `None`: a file
    /// from 0.5, written before this flag existed.
    #[serde(default)]
    adult: Option<bool>,
}

impl LastWatched {
    fn load(path: &std::path::Path) -> Option<Self> {
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()
    }
    fn save(&self, path: &std::path::Path) {
        if let Ok(s) = serde_json::to_string(self) {
            let _ = std::fs::write(path, s);
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProgrammeStatus {
    /// Currently airing.
    Live,
    /// Already aired and the channel exposes catch-up (`tv_archive=1`).
    Catchup,
    /// Not yet aired.
    Future,
    /// Past programme on a channel with no catch-up - shown dim, not playable.
    PastUnavailable,
}

fn programme_status(
    entry: &EpgEntry,
    channel: &LiveChannel,
    now: chrono::DateTime<chrono::Utc>,
) -> ProgrammeStatus {
    if entry.start <= now && now < entry.end {
        ProgrammeStatus::Live
    } else if entry.end <= now {
        if channel.tv_archive == 1 {
            ProgrammeStatus::Catchup
        } else {
            ProgrammeStatus::PastUnavailable
        }
    } else {
        ProgrammeStatus::Future
    }
}

/// Whether a guide pick plays as live TV: through the relay, with the stall
/// watchdog armed and resumed as live. Catch-up plays as VOD.
fn plays_as_live(status: ProgrammeStatus) -> bool {
    matches!(status, ProgrammeStatus::Live)
}

/// What to do with the last-watched stream at startup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResumeDecision {
    Play,
    Skip,
    WaitForCatalog,
}

/// `flag`: the file's adult flag; `catalog_says_adult`: the catalog's answer,
/// `None` while it is not loaded yet. Either one saying adult is enough (the
/// file can be older than the portal's word); with neither knowing, the resume
/// waits for the catalog.
fn resume_decision(flag: Option<bool>, catalog_says_adult: Option<bool>) -> ResumeDecision {
    match (flag, catalog_says_adult) {
        (Some(true), _) | (_, Some(true)) => ResumeDecision::Skip,
        (None, None) => ResumeDecision::WaitForCatalog,
        _ => ResumeDecision::Play,
    }
}

/// What the catalog says about a stream URL: `None` while it is not loaded. A
/// catalog that knows nothing is not one that says "not adult".
fn catalog_says_adult(catalog: &CatalogStore, url: &str) -> Option<bool> {
    catalog.is_loaded().then(|| catalog.is_adult_url(url))
}

/// The record last_watched.json gets when `url` loads, or `None` to leave the
/// file as it is. Its adult flag is the catalog's word when it can give one,
/// else what the caller knows (`hint`: the resume passes the file's own flag
/// on), else unknown, never "not adult" for lack of knowledge: the next start
/// would resume an adult channel (a URL from the command line) without the PIN.
/// A reload of the stream that plays, with nothing new known about it, leaves
/// the file alone: the flag it has must not turn into "unknown".
fn last_watched_record(
    catalog: &CatalogStore,
    url: &str,
    live: bool,
    hint: Option<bool>,
    reload: bool,
) -> Option<LastWatched> {
    let adult = catalog_says_adult(catalog, url).or(hint);
    if adult.is_none() && reload {
        return None;
    }
    Some(LastWatched {
        url: url.to_owned(),
        live,
        adult,
    })
}

/// The favorites that show: adult ones act as if they were not there while
/// locked.
fn shown_favorites(favorites: &Favorites, catalog: &CatalogStore) -> Vec<FavoriteEntry> {
    favorites
        .iter()
        .filter(|e| !catalog.is_hidden_live(e.stream_id, &e.name))
        .cloned()
        .collect()
}

/// The channel a preset recalls: none when the slot is empty, or adult while
/// locked (it then acts as empty).
fn preset_target(presets: &Presets, digit: u8, catalog: &CatalogStore) -> Option<i64> {
    presets
        .get(digit)
        .filter(|p| !catalog.is_hidden_live(p.stream_id, &p.name))
        .map(|p| p.stream_id)
}

/// One line of the help overlay: the key(s) and what they do. Both are static
/// texts (key names or `i18n` lookups), so the rows allocate no strings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HelpRow {
    key: &'static str,
    action: &'static str,
    /// The settings row: drawn as a link that opens them.
    opens_settings: bool,
}

/// The help's rows per section, (key, action), in `lang` and with the zap row
/// as the zap direction is set (reversing flips the arrows and the wheel,
/// never the guide).
fn help_rows(lang: Lang, swap_zap_keys: bool) -> [(T, Vec<HelpRow>); 2] {
    let t = |k: T| crate::i18n::t(lang, k);
    let row = |key: &'static str, action: &'static str| HelpRow {
        key,
        action,
        opens_settings: false,
    };
    let link = |key: &'static str, action: &'static str| HelpRow {
        key,
        action,
        opens_settings: true,
    };
    let zap = if swap_zap_keys {
        T::HelpZapSwapped
    } else {
        T::HelpZap
    };
    [
        (
            T::HelpViewer,
            vec![
                row(t(T::KeyUpDownWheel), t(zap)),
                row(t(T::KeyDigitsTap), t(T::HelpPresetRecall)),
                row(t(T::KeyDigitsHold), t(T::HelpPresetStore)),
                row("n", t(T::HelpNews)),
                row("f", t(T::HelpSearch)),
                row("Shift+F", t(T::HelpFavorites)),
                row("*", t(T::HelpToggleFavorite)),
                row("e", t(T::HelpEpgStrip)),
                row("Shift+E", t(T::HelpEpgGrid)),
                row("a / s", t(T::HelpAudioSubs)),
                row("+ / -", t(T::HelpQuality)),
                row(t(T::KeyLeftRight), t(T::HelpSeek)),
                row(t(T::KeySpace), t(T::HelpPause)),
                link("o", t(T::HelpSettings)),
                row("F11", t(T::HelpFullscreen)),
                row("F5", t(T::HelpRetryPortal)),
                row("d", t(T::HelpDebug)),
                row("g", t(T::HelpOpenGuide)),
                row("t", t(T::HelpBorderless)),
                row("?", t(T::HelpThisHelp)),
                row("Esc", t(T::HelpCloseOverlays)),
            ],
        ),
        (
            T::HelpGuide,
            vec![
                row(t(T::KeyTypeLetters), t(T::HelpGuideFilter)),
                row("Backspace", t(T::HelpGuideBackspace)),
                row(t(T::KeyUpDown), t(T::HelpGuideRows)),
                row(t(T::KeyLeftRight), t(T::HelpGuideColumns)),
                row("PgUp / PgDn", t(T::HelpGuidePage)),
                row("Home / End", t(T::HelpGuideHomeEnd)),
                row("n / p", t(T::HelpGuideTimeMode)),
                row("a / l / t", t(T::HelpGuideChannelMode)),
                row("Enter", t(T::HelpGuidePlay)),
                row(t(T::KeyEscOrG), t(T::HelpGuideClose)),
            ],
        ),
    ]
}

/// Vertical padding inside the help's frame, in points.
const HELP_FRAME_MARGIN: f32 = 14.0;
/// Room the help keeps between its frame and the window's edge.
const HELP_SCREEN_MARGIN: f32 = 12.0;
/// What the help's footer needs below the rows: spacing, separator and one
/// line of 11 pt text, with a little to spare.
const HELP_FOOTER_HEIGHT: f32 = 30.0;
/// The least height the rows keep, however small the window.
const HELP_MIN_ROWS_HEIGHT: f32 = 66.0;
/// How far one Up or Down press scrolls the help: two rows.
const HELP_SCROLL_LINE: f32 = 44.0;

/// Draws one row of the help. The settings row is a link: both texts
/// underlined, and the key, the gap and the action are one click target and one
/// Tab stop. True when that link was clicked (or got Enter or Space).
fn paint_help_row(ui: &mut egui::Ui, row: &HelpRow) -> bool {
    let key_text = egui::RichText::new(row.key)
        .monospace()
        .color(Color32::from_rgb(230, 220, 110))
        .size(12.0);
    let action_text = egui::RichText::new(row.action)
        .color(Color32::from_white_alpha(220))
        .size(12.0);
    if !row.opens_settings {
        ui.label(key_text);
        ui.label(action_text);
        return false;
    }
    // A slot behind the texts, filled once the row's extent is known.
    let background = ui.painter().add(egui::Shape::Noop);
    let key = ui.add(egui::Label::new(key_text.underline()).selectable(false));
    let action = ui.add(egui::Label::new(action_text.underline()).selectable(false));
    let rect = key.rect.union(action.rect);
    let id = egui::Id::new("__help_settings__");
    let response = ui
        .interact(rect, id, egui::Sense::click())
        .on_hover_cursor(egui::CursorIcon::PointingHand);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Link, row.action));
    if response.hovered() {
        let hover = egui::Shape::rect_filled(rect, 3.0, Color32::from_rgb(44, 44, 54));
        ui.painter().set(background, hover);
    }
    if response.has_focus() {
        let ring = egui::Stroke::new(1.0, Color32::from_rgb(120, 180, 240));
        ui.painter().rect_stroke(rect, 3.0, ring);
    }
    response.clicked()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TimeMode {
    /// Each column scrolls to its currently-airing programme on open / mode-switch.
    NowAndNext,
    /// Each column scrolls to ~20:00 (start of prime-time block).
    Primetime,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChannelMode {
    /// All channels including tv_archive=1 archive duplicates (prefixed [TK]).
    All,
    /// Only tv_archive=0 - the live-only channels.
    Live,
    /// Only tv_archive=1 - the catch-up archive channels.
    Catchup,
}

/// Pick a recognisable accent color for a channel based on common Dutch
/// pattern matches. Purely visual - lets the user spot a channel category
/// at a glance without us shipping logos. Falls back to neutral grey.
fn channel_accent_color(name: &str) -> Color32 {
    let n = name.to_uppercase();
    if n.contains("NPO 1") || n.contains("NPO1") {
        Color32::from_rgb(245, 130, 32)
    } else if n.contains("NPO 2") || n.contains("NPO2") {
        Color32::from_rgb(40, 165, 220)
    } else if n.contains("NPO 3") || n.contains("NPO3") {
        Color32::from_rgb(120, 200, 70)
    } else if n.contains("NPO") {
        Color32::from_rgb(220, 110, 30)
    } else if n.contains("RTL 4") || n.contains("RTL4") {
        Color32::from_rgb(220, 30, 30)
    } else if n.contains("RTL 5") || n.contains("RTL5") {
        Color32::from_rgb(180, 50, 50)
    } else if n.contains("RTL Z") || n.contains("RTLZ") {
        Color32::from_rgb(160, 30, 30)
    } else if n.contains("RTL") {
        Color32::from_rgb(200, 40, 40)
    } else if n.contains("SBS") {
        Color32::from_rgb(255, 160, 0)
    } else if n.contains("NET 5") || n.contains("NET5") {
        Color32::from_rgb(180, 90, 180)
    } else if n.contains("VERONICA") {
        Color32::from_rgb(200, 50, 100)
    } else if n.contains("SPORT") || n.contains("ESPN") || n.contains("ZIGGO SPORT") || n.contains("FOX SPORTS") {
        Color32::from_rgb(50, 170, 90)
    } else if n.contains("DISCOVERY") || n.contains("NATIONAL GEOGRAPHIC") || n.contains("HISTORY") || n.contains("ANIMAL PLANET") {
        Color32::from_rgb(40, 140, 140)
    } else if n.contains("FILM") || n.contains("CINEMA") || n.contains("MOVIE") {
        Color32::from_rgb(140, 70, 200)
    } else if n.contains("KIDS") || n.contains("DISNEY") || n.contains("NICKELODEON") || n.contains("CARTOON") {
        Color32::from_rgb(230, 90, 160)
    } else if n.contains("NIEUWS") || n.contains("NEWS") || n.contains("BNR") || n.contains("NOS") {
        Color32::from_rgb(70, 130, 220)
    } else {
        Color32::from_rgb(110, 110, 120)
    }
}

/// Extract a 1-3 character "badge" label from a channel name. For Dutch
/// patterns we prefer the channel number ("NPO 1" -> "1", "RTL 4" -> "4");
/// for others we fall back to the first letter.
fn channel_badge_label(name: &str) -> String {
    let up = name.to_uppercase();
    for prefix in ["NPO ", "NPO", "RTL ", "RTL", "SBS ", "SBS", "NET ", "NET"] {
        if let Some(idx) = up.find(prefix) {
            let rest = &up[idx + prefix.len()..];
            let digits: String = rest
                .chars()
                .skip_while(|c| !c.is_ascii_digit() && *c != 'Z')
                .take_while(|c| c.is_ascii_digit() || *c == 'Z')
                .collect();
            if !digits.is_empty() {
                return digits;
            }
        }
    }
    name.chars()
        .find(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_string())
        .unwrap_or_else(|| "?".into())
}

/// NLZIET-style column EPG guide opened with `g`.
///
/// Layout: top filter/time bar, then N vertical channel columns side-by-side.
/// Each column has its own virtualized programme list. Cursor lives at
/// (selected_col, selected_row_per_channel[chan_id]) so jumping between
/// columns preserves where the user was in each channel.
struct GuideState {
    open: bool,
    /// Free-text filter (typed in the top bar; matches channel name substring).
    filter: String,
    /// Indices into `catalog.live_channels()` for entries matching `filter`.
    /// Rebuilt when filter or catalog changes.
    visible: Vec<usize>,
    visible_filter_snapshot: String,
    /// Index INTO `visible` of the LEFTMOST channel currently rendered.
    column_offset: usize,
    /// Which visible column has cursor focus (0..num_visible_columns).
    selected_col: usize,
    /// Programme-row cursor per channel, keyed by stream_id so it survives
    /// horizontal scrolling and time-mode changes.
    row_per_channel: HashMap<i64, usize>,

    /// EPG cache: stream_id -> Epg. Filled by background tokio tasks; never
    /// evicted during session (~50 KB per channel, bounded by channels visited).
    epg_cache: Arc<Mutex<HashMap<i64, Epg>>>,
    /// Set of stream_ids with an in-flight fetch (prevents duplicate requests
    /// when the user pans back and forth across the same column).
    epg_pending: Arc<Mutex<HashSet<i64>>>,
    /// When did the visible-column SET last change? Debounce so rapid
    /// horizontal panning doesn't fire dozens of HTTP calls.
    visible_set_settled_since: Instant,
    visible_set_settled_snapshot: Vec<i64>,

    time_mode: TimeMode,
    /// While true, every visible column's current-programme row is forced to
    /// the vertical centre each frame (the "now line"). Set on open / time-mode
    /// change / left-right pan; cleared when the user browses with up/down.
    auto_center: bool,
    /// Stream IDs whose last EPG fetch returned an error or returned with
    /// zero entries after our fallback attempt. UI shows a distinct
    /// 'EPG mislukt' message instead of the ambiguous 'geen EPG' that
    /// could mean either failure or successful-but-empty.
    epg_failed: Arc<Mutex<HashSet<i64>>>,

    channel_mode: ChannelMode,
}

impl Default for GuideState {
    fn default() -> Self {
        Self {
            open: false,
            filter: String::new(),
            visible: Vec::new(),
            visible_filter_snapshot: String::new(),
            column_offset: 0,
            selected_col: 0,
            row_per_channel: HashMap::new(),
            epg_cache: Arc::new(Mutex::new(HashMap::new())),
            epg_pending: Arc::new(Mutex::new(HashSet::new())),
            visible_set_settled_since: Instant::now(),
            visible_set_settled_snapshot: Vec::new(),
            time_mode: TimeMode::NowAndNext,
            auto_center: true,
            epg_failed: Arc::new(Mutex::new(HashSet::new())),
            channel_mode: ChannelMode::All,
        }
    }
}

impl GuideState {
    /// Another portal's catalog took over: everything kept per stream id means
    /// other channels now. Fetches of the old portal still in flight finish into
    /// the old maps, which they alone still hold.
    fn forget_portal(&mut self) {
        self.row_per_channel.clear();
        self.epg_cache = Arc::new(Mutex::new(HashMap::new()));
        self.epg_pending = Arc::new(Mutex::new(HashSet::new()));
        self.epg_failed = Arc::new(Mutex::new(HashSet::new()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    /// `--xtream user:pass@host:port` was given with non-placeholder values.
    Configured,
    /// No `--xtream` argument given at all.
    Missing,
    /// `--xtream` was given but with the placeholder template values.
    Placeholder,
}

/// One pickable past news broadcast.
#[derive(Clone)]
struct NewsItem {
    sid: i64, // archive catch-up stream_id, or the live stream_id when `live`
    title: String,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
    tag: String, // NOS / RTL / CNN / BBC / VRT / ZLD
    offset: i64, // catch-up start offset (secs); RTL skips ads
    live: bool,  // 24h news channel with no catch-up - play it live
}

/// A news source for the `n` picker: which catch-up channel, and which
/// programmes on it count as "news".
struct NewsSrc {
    /// Normalized channel-name key (matched exact first, then substring).
    chan: &'static str,
    /// Lowercase title substring required; "" = any programme (news channel).
    title: &'static str,
    /// Title substrings to drop (regional/sport/kids variants).
    excl: &'static [&'static str],
    tag: &'static str,
    offset: i64,
}

/// Modal "latest news" chooser opened with `n` (RTL + NOS combined).
struct NewsPicker {
    items: Vec<NewsItem>,
    selected: usize,
}

/// A finished credentials test, on its way from its background task to the UI.
struct ConnResult {
    check: ConnCheck,
    /// What to keep if the login worked (a Save): the credentials as parsed and
    /// as typed. `None` for a plain test.
    keep: Option<(XtreamCreds, String)>,
}

pub struct TvApp {
    player: PlayerHandle,
    catalog: Arc<CatalogStore>,
    favorites: Favorites,
    storage: Storage,
    portal_state: PortalState,
    /// Portal credentials in use: the settings screen shows them and the PIN
    /// reset checks the password against them.
    creds: Option<XtreamCreds>,

    video_tex: Option<TextureHandle>,
    last_frame_version: u64,

    current_idx: Option<usize>,
    current_name: Option<String>,
    current_stream_id: Option<i64>,

    current_epg: Option<Epg>,
    epg_slot: Arc<Mutex<Option<(i64, Epg)>>>,
    epg_fetch_pending_for: Option<i64>,
    /// Combined NOS/RTL news list built async for the `n` picker.
    news_picker_slot: Arc<Mutex<Option<Vec<NewsItem>>>>,
    /// Open `n` news chooser (modal overlay), if any.
    news_picker: Option<NewsPicker>,

    toast: Option<(String, Instant, Duration)>,

    show_favs: bool,
    show_search: bool,
    show_epg_strip: bool,
    show_epg_grid: bool,
    show_debug: bool,

    search_query: String,
    /// Results for the query (and catalog state) in `search_hits_for`:
    /// searching clones every channel, film and series name, so it runs when
    /// the query changes rather than every frame.
    search_hits: Vec<SearchItem>,
    search_hits_for: Option<(String, bool)>,
    /// Highlighted result (arrow keys); Enter plays it.
    search_sel: usize,

    guide: GuideState,
    /// Tracks borderless+always-on-top toggle (`t` key).
    borderless: bool,
    /// `?` toggles a centered help overlay listing every hotkey.
    show_help: bool,

    // ---- silent-stall watchdog: recover frozen streams by reloading ----
    /// URL of the stream currently loaded, so the watchdog can reload it.
    current_url: Option<String>,
    /// Whether that stream is live TV (else a film, series or catch-up).
    current_live: bool,
    /// Paused with the space bar (films, series and catch-up only).
    paused: bool,
    /// Last FrameBus.version we observed, and when it last advanced.
    last_progress_version: u64,
    last_progress_at: Instant,
    /// When we last auto-reloaded, to rate-limit recovery attempts.
    last_recovery_at: Option<Instant>,
    /// Armed only for live channels (VOD/catch-up can legitimately end).
    stall_watchdog_armed: bool,
    /// Caps the auto-reloads after a live stream ends unexpectedly (eof).
    reload_budget: ReloadBudget,
    /// Local HLS relay for live streams; `None` if it could not start, then
    /// mpv plays the portal directly.
    relay: Option<Relay>,
    /// Paces up/down zapping: quick presses or a held arrow key only move the
    /// target (sid, name, zap index); the channel the user stops on loads.
    zap_pacer: ZapPacer<(i64, String, usize)>,

    // ---- freeze detection: surfaces cache-underrun stalls (paused-for-cache) ----
    /// Per-stream freeze tracker (reset on load; counts only post-first-frame
    /// freezes). Fed by the player's paused-for-cache observation, read by the
    /// debug HUD. Independent of the stall watchdog above: that one *recovers*
    /// (auto-reload after ~10s), this one *measures and shows*.
    health: PlaybackHealth,

    // ---- car-radio number presets on digit keys 0-9 ----
    presets: Presets,
    /// Per-digit press-start time, for long-press(save) vs tap(recall).
    /// None = key currently up.
    digit_down_since: [Option<Instant>; 10],
    /// True once a held digit fired its save, so its release won't also recall.
    digit_long_fired: [bool; 10],

    // ---- first-run portal setup prompt (writes BongoPlayer.ini) ----
    cfg_host: String,
    cfg_user: String,
    cfg_pass: String,

    /// Preferences (settings.json).
    settings: Settings,
    /// The UI language: from the settings, else the Windows language.
    lang: Lang,
    /// First start: the language screen shows, with this one preselected.
    choosing_language: Option<Lang>,
    /// A resume held back until the language is chosen (first start).
    pending_resume: Option<LastWatched>,
    /// The settings overlay (`o`) while it is open.
    settings_ui: Option<SettingsUi>,
    /// The portal credentials test (settings screen and setup prompt).
    conn_state: ConnState,
    conn_slot: Arc<Mutex<Option<ConnResult>>>,
    /// Wrong PIN attempts; blocks PIN input for 30 s after 3 in a row.
    pin_gate: PinGate,
    /// The last PIN outcome, shown in the adult section.
    adult_note: Option<T>,
    /// After a stop, the stopped stream's frames must not show again.
    frame_fence: FrameFence,
}

/// The translated text for a credentials error.
fn creds_error_text(e: &CredsError) -> T {
    match e {
        CredsError::NoAt => T::CredsNoAt,
        CredsError::NoUserPass => T::CredsNoUserPass,
        CredsError::BadPort => T::CredsBadPort,
    }
}

impl TvApp {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        player: PlayerHandle,
        catalog: Arc<CatalogStore>,
        storage: Storage,
        portal_state: PortalState,
        initial_url: Option<String>,
        relay: Option<Relay>,
        creds: Option<XtreamCreds>,
        settings: Settings,
    ) -> Self {
        // Whenever a new mpv frame is ready, ask egui to redraw.
        let ctx = cc.egui_ctx.clone();
        player
            .frames
            .set_new_frame_callback(move || ctx.request_repaint());

        let favorites = Favorites::load(&storage.favorites_path()).unwrap_or_default();
        let presets = Presets::load(&storage.presets_path()).unwrap_or_default();
        // Only kick off a fetch if we actually have a real portal — otherwise the
        // request would hang on a placeholder/missing host until the network timeout
        // and the user would see "loading..." for ~45s before any error appears.
        if portal_state == PortalState::Configured {
            catalog.spawn_fetch();
        }

        let lang = settings.lang().unwrap_or_else(crate::i18n::system_lang);
        let choosing_language = settings.lang().is_none().then_some(lang);
        let mut app = Self {
            player,
            catalog,
            favorites,
            storage,
            portal_state,
            creds,
            video_tex: None,
            last_frame_version: 0,
            current_idx: None,
            current_name: None,
            current_stream_id: None,
            current_epg: None,
            epg_slot: Arc::new(Mutex::new(None)),
            news_picker_slot: Arc::new(Mutex::new(None)),
            news_picker: None,
            epg_fetch_pending_for: None,
            toast: None,
            show_favs: false,
            show_search: false,
            show_epg_strip: false,
            show_epg_grid: false,
            show_debug: false,
            search_query: String::new(),
            search_hits: Vec::new(),
            search_hits_for: None,
            search_sel: 0,
            guide: GuideState::default(),
            borderless: false,
            show_help: false,
            current_url: None,
            current_live: false,
            paused: false,
            last_progress_version: 0,
            last_progress_at: Instant::now(),
            last_recovery_at: None,
            stall_watchdog_armed: false,
            reload_budget: ReloadBudget::new(3, Duration::from_secs(60)),
            relay,
            // Deliberate taps come >= ~0.5 s apart, flipping taps faster.
            zap_pacer: ZapPacer::new(Duration::from_millis(350)),
            health: PlaybackHealth::new(),
            presets,
            digit_down_since: [None; 10],
            digit_long_fired: [false; 10],
            cfg_host: String::new(),
            cfg_user: String::new(),
            cfg_pass: String::new(),
            settings,
            lang,
            choosing_language,
            pending_resume: None,
            settings_ui: None,
            conn_state: ConnState::Idle,
            conn_slot: Arc::new(Mutex::new(None)),
            pin_gate: PinGate::new(),
            adult_note: None,
            frame_fence: FrameFence::default(),
        };
        // Initial stream: an explicit --url / bare URL wins; otherwise resume
        // whatever we were watching last session (persisted by load_url).
        if let Some(url) = initial_url {
            app.load_url(url, false);
        } else if app.portal_state == PortalState::Configured {
            // Only resume when a portal is actually configured. Without creds
            // (e.g. BongoPlayer.ini deleted) drop to the setup prompt instead of
            // silently replaying the last URL (whose credentials are baked in).
            if let Some(lw) = LastWatched::load(&app.storage.last_watched_path()) {
                if app.choosing_language.is_some() {
                    app.pending_resume = Some(lw);
                } else {
                    app.resume(lw);
                }
            }
        }
        app
    }

    /// Play what was playing when the app last closed, unless that was adult
    /// content (every start is locked).
    fn resume(&mut self, lw: LastWatched) {
        let catalog_says = catalog_says_adult(&self.catalog, &lw.url);
        match resume_decision(lw.adult, catalog_says) {
            ResumeDecision::Play => {}
            ResumeDecision::Skip => {
                tracing::info!("resume: last watched was adult content - not resumed");
                return;
            }
            ResumeDecision::WaitForCatalog => {
                self.pending_resume = Some(lw);
                return;
            }
        }
        tracing::info!("resuming last watched ({})", crate::redact::redact(&lw.url));
        self.set_toast(self.t(T::ResumingLast));
        // Up/down then continues from this channel (see zap_delta).
        self.current_stream_id = self.catalog.portal().live_stream_id(&lw.url);
        // The file's flag goes on as the hint: the catalog is mostly not loaded
        // yet, and a normal start must not record "unknown" for the next one.
        self.load_url_with(lw.url, lw.live, lw.adult);
    }

    fn open_settings(&mut self) {
        self.settings_ui = Some(SettingsUi::new());
        // An earlier visit's verdict is stale; a test still running is not.
        if self.conn_state != ConnState::Testing {
            self.conn_state = ConnState::Idle;
        }
        self.adult_note = None;
        tracing::info!("settings: opened");
    }

    /// A failed write keeps the change until the app closes, with a notice.
    fn save_settings(&mut self) {
        if let Err(e) = self.settings.save(&self.storage.settings_path()) {
            tracing::warn!("settings: save failed: {}", e);
            self.set_toast(crate::i18n::t(self.lang, T::SaveFailed));
        }
    }

    fn choose_language(&mut self, l: Lang) {
        self.lang = l;
        self.choosing_language = None;
        self.settings.language = Some(l.code().to_string());
        self.save_settings();
        tracing::info!("settings: language set to {}", l.code());
        if let Some(lw) = self.pending_resume.take() {
            self.resume(lw);
        }
    }

    /// Check a PIN against the saved hash, counting wrong attempts.
    fn pin_attempt(&mut self, pin: &str) -> bool {
        let now = Instant::now();
        if self.pin_gate.wait_left(now).is_some() {
            return false;
        }
        let ok = self.settings.pin.as_ref().is_some_and(|h| h.verify(pin));
        self.pin_gate.record(ok, now);
        if !ok {
            self.adult_note = Some(T::PinWrong);
            tracing::info!("adult: wrong PIN");
            if self.pin_gate.wait_left(now).is_some() {
                tracing::info!("adult: PIN input blocked for 30 s");
            }
        }
        ok
    }

    /// The channel lists changed under whatever points into them: adult content
    /// shown or hidden, or another portal's catalog. The playing channel is
    /// found in the zap list again on the next arrow press, search results are
    /// searched again, and the guide rebuilds its cached channel indices.
    fn forget_list_positions(&mut self) {
        self.current_idx = None;
        self.search_hits_for = None;
        self.guide.visible_filter_snapshot.clear();
    }

    fn set_adult_visible(&mut self, on: bool) {
        self.catalog.set_adult_unlocked(on);
        // The lists changed length: find the playing channel again, search again.
        self.forget_list_positions();
        let note = if on { T::Unlocked } else { T::LockedToast };
        self.adult_note = Some(note);
        self.set_toast(self.t(note));
        tracing::info!("adult: {}", if on { "unlocked" } else { "locked" });
    }

    /// Stop playing: mpv stops, its last picture goes, nothing is current.
    fn stop_playback(&mut self) {
        let _ = self.player.cmd_tx.send(Cmd::Stop);
        self.zap_pacer.cancel();
        self.current_url = None;
        self.current_live = false;
        self.current_idx = None;
        self.current_name = None;
        self.current_stream_id = None;
        self.current_epg = None;
        self.stall_watchdog_armed = false;
        self.frame_fence.stop();
        self.video_tex = None;
        tracing::info!("player: stop");
    }

    /// What plays is adult content, as the catalog in use says.
    fn playing_adult(&self) -> bool {
        self.current_url
            .as_deref()
            .is_some_and(|u| self.catalog.is_adult_url(u))
    }

    fn apply_settings_action(&mut self, action: SettingsAction) {
        match action {
            SettingsAction::Close => {
                self.settings_ui = None;
                tracing::info!("settings: closed");
            }
            SettingsAction::SetLanguage(l) => {
                self.lang = l;
                self.settings.language = Some(l.code().to_string());
                self.save_settings();
                tracing::info!("settings: language set to {}", l.code());
            }
            SettingsAction::SetSwapZap(on) => {
                self.settings.swap_zap_keys = on;
                self.save_settings();
                tracing::info!("settings: zap keys {}", if on { "reversed" } else { "normal" });
            }
            SettingsAction::TestConnection(s) => self.start_conn_check(s, false),
            SettingsAction::SavePortal(s) => self.start_conn_check(s, true),
            SettingsAction::Unlock(pin) => {
                if self.pin_attempt(&pin) {
                    self.set_adult_visible(true);
                }
            }
            SettingsAction::SetPin(pin) => {
                // Only for the very first PIN: otherwise this would bypass it.
                if self.settings.pin.is_none() {
                    match PinHash::new(&pin) {
                        Ok(h) => {
                            self.settings.pin = Some(h);
                            self.set_adult_visible(true);
                            // After the toast above: a failed write must show its own.
                            self.save_settings();
                        }
                        Err(_) => self.adult_note = Some(T::PinFormat),
                    }
                }
            }
            SettingsAction::ChangePin { old, new } => {
                if self.pin_attempt(&old) {
                    match PinHash::new(&new) {
                        Ok(h) => {
                            self.settings.pin = Some(h);
                            self.save_settings();
                            self.adult_note = Some(T::PinChanged);
                            tracing::info!("adult: PIN changed");
                        }
                        Err(_) => self.adult_note = Some(T::PinFormat),
                    }
                }
            }
            SettingsAction::ResetPin(password) => {
                let actual = self
                    .creds
                    .as_ref()
                    .map(|c| c.password.as_str())
                    .unwrap_or("");
                if crate::pin::portal_password_ok(&password, actual) {
                    self.settings.pin = None;
                    self.save_settings();
                    // Whoever proved the portal password need not wait out an
                    // earlier block before choosing the new PIN.
                    self.pin_gate = PinGate::new();
                    self.adult_note = Some(T::PinCleared);
                    tracing::info!("adult: PIN cleared with the portal password");
                } else {
                    self.adult_note = Some(T::WrongPortalPassword);
                }
            }
            SettingsAction::Lock => {
                self.set_adult_visible(false);
                // Adult content playing now stops, and its picture goes.
                if self.playing_adult() {
                    self.stop_playback();
                }
            }
        }
    }

    fn paint_settings(&mut self, ctx: &egui::Context) {
        let Some(overlay) = self.settings_ui.as_mut() else {
            return;
        };
        let view = SettingsView {
            lang: self.lang,
            settings: &self.settings,
            creds: self.creds.as_ref(),
            conn: &self.conn_state,
            adult_unlocked: self.catalog.adult_unlocked(),
            has_pin: self.settings.pin.is_some(),
            pin_wait_secs: self
                .pin_gate
                .wait_left(Instant::now())
                .map(|d| d.as_secs().max(1)),
            adult_note: self.adult_note,
        };
        let actions = overlay.show(ctx, &view);
        for a in actions {
            self.apply_settings_action(a);
        }
    }

    /// A fixed text in the UI language.
    fn t(&self, key: T) -> &'static str {
        crate::i18n::t(self.lang, key)
    }

    /// A text with values ({0}, {1}, ...) in the UI language.
    fn tr(&self, key: T, args: &[&str]) -> String {
        crate::i18n::tr(self.lang, key, args)
    }

    fn set_toast(&mut self, s: impl Into<String>) {
        self.set_toast_for(s, 4);
    }

    /// Toast that lingers for a custom number of seconds. Used by the n/r news
    /// shortcuts so the "what we're watching" label stays up ~30s.
    fn set_toast_for(&mut self, s: impl Into<String>, secs: u64) {
        let s = s.into();
        tracing::debug!("toast: {}", s);
        self.toast = Some((s, Instant::now(), Duration::from_secs(secs)));
    }

    /// Send a stream to the player and remember it as `current_url`. `watchdog`
    /// arms the silent-stall auto-reload (live channels only; VOD/catch-up pass
    /// false so a finished file isn't reloaded on a loop). Centralises the
    /// LoadUrl send so current_url and the watchdog timer stay consistent.
    fn load_url(&mut self, url: String, watchdog: bool) {
        self.load_url_with(url, watchdog, None);
    }

    /// `load_url`, with what the caller already knows about the stream's adult
    /// flag (`adult_hint`) for while the catalog cannot tell yet: the resume
    /// hands the file's own flag on.
    fn load_url_with(&mut self, url: String, watchdog: bool, adult_hint: Option<bool>) {
        // Whatever loads now supersedes a zap still waiting for the user to
        // stop flipping.
        self.zap_pacer.cancel();
        // mpv keeps `pause` across files: a new stream must not start paused.
        if self.paused {
            self.paused = false;
            let _ = self.player.cmd_tx.send(Cmd::Pause(false));
        }
        // Before current_live changes: a reload is this very stream played the
        // same way again.
        let reload =
            self.current_url.as_deref() == Some(url.as_str()) && self.current_live == watchdog;
        self.current_live = watchdog;
        // Live goes through the local relay: one continuous stream for mpv
        // that rides out portal refusals instead of dying on them. The portal
        // URL stays the current/last-watched one (the relay port changes
        // between runs). Catch-up (and HLS VOD) comes from one portal request
        // there instead of mpv's two, each a playlist generation of seconds.
        let hls = url.split('?').next().is_some_and(|p| p.ends_with(".m3u8"));
        let play = match &self.relay {
            Some(relay) if watchdog => relay.local_url(&url),
            Some(relay) if hls => relay.vod_url(&url),
            _ => url.clone(),
        };
        // After a stop: what is on the bus now is the stopped stream's. Read
        // before the load goes out, so no frame of the new stream is counted.
        self.frame_fence.load(self.player.frames.version());
        let _ = self.player.cmd_tx.send(Cmd::LoadUrl(play));
        // Remember as last-watched so the next launch resumes it, unless it is
        // adult content (what is known about that: see last_watched_record).
        if let Some(record) = last_watched_record(&self.catalog, &url, watchdog, adult_hint, reload)
        {
            record.save(&self.storage.last_watched_path());
        }
        self.current_url = Some(url);
        self.stall_watchdog_armed = watchdog;
        // Fresh stream: give it a full window to start before judging it stalled.
        self.last_progress_at = Instant::now();
        // Reset per-stream freeze counters; counting re-arms on the first frame
        // (PlaybackStarted) so startup/zap buffering isn't tallied as a freeze.
        self.health.reset();
    }

    /// Recover a SILENT video stall: frames stop advancing while a live stream
    /// should be playing and mpv emitted no end-file/error. We reload the
    /// current URL - the automatic version of the user's "zap up then back
    /// down" fix. Runs every update() (~30 Hz, even with no frames) so it fires
    /// while the picture is frozen. Uses FrameBus::version() to avoid cloning
    /// the pixel buffer.
    fn check_stall(&mut self) {
        if !self.stall_watchdog_armed || self.zap_pacer.waiting().is_some() {
            return; // nothing to watch, or the user is zapping away anyway
        }
        let Some(url) = self.current_url.clone() else {
            return;
        };
        let v = self.player.frames.version();
        if v != self.last_progress_version {
            self.last_progress_version = v;
            self.last_progress_at = Instant::now();
            return;
        }
        const COOLDOWN: Duration = Duration::from_secs(15);
        if self.last_progress_at.elapsed() < self.health.stall_limit() {
            return;
        }
        if let Some(t) = self.last_recovery_at {
            if t.elapsed() < COOLDOWN {
                return;
            }
        }
        tracing::warn!(
            "video stalled ~{}s with no end-file/error - auto-reloading stream",
            self.last_progress_at.elapsed().as_secs()
        );
        self.set_toast(self.t(T::Reconnecting));
        self.load_url(url, true);
        self.last_recovery_at = Some(Instant::now());
    }

    fn drain_events(&mut self) {
        let mut buf: Vec<Event> = Vec::new();
        {
            let mut rx = self.player.evt_rx.lock();
            while let Ok(evt) = rx.try_recv() {
                buf.push(evt);
            }
        }
        for evt in buf {
            match evt {
                Event::FileLoaded => {}
                Event::PlaybackStarted => {
                    // First frame shown — arm freeze counting. Buffering before
                    // this point (startup/zap) is shown live but not tallied.
                    self.health.mark_started();
                }
                Event::EndOfFile { reason } => {
                    // mpv fires end-file on EVERY channel switch: loadfile stops
                    // the previous stream with reason=stop. Surfacing that as a
                    // toast overwrote the "[TV] <channel>" zap toast (single slot)
                    // and the EPG-enriched line, leaving the user staring at
                    // "ended: <reason>" with no idea what they tuned to.
                    // quit/redirect/eof are equally internal - only a genuine
                    // playback error deserves to interrupt the channel toast.
                    //
                    // A live stream never ends on its own: eof there means the
                    // portal refused a playlist refresh (HTTP 509) and libav
                    // gave up. Reload via the portal URL (fresh redirect)
                    // instead of leaving a frozen picture; the budget stops a
                    // channel that is really down from reloading on a loop.
                    let live = self.stall_watchdog_armed;
                    if reason == "eof" && live && self.zap_pacer.waiting().is_some() {
                        continue; // the user is zapping away anyway
                    }
                    if reason == "eof" && live {
                        if let Some(url) = self.current_url.clone() {
                            if self.reload_budget.try_spend(Instant::now()) {
                                tracing::warn!("live stream ended (eof) - auto-reloading");
                                self.set_toast(self.t(T::Reconnecting));
                                self.load_url(url, true);
                                self.last_recovery_at = Some(Instant::now());
                                continue;
                            }
                            tracing::warn!("live stream keeps ending - auto-reload budget spent");
                        }
                    }
                    // The portal refused this variant to the account: play
                    // another one of the channel rather than an error.
                    if reason == "error" && live {
                        let sid = self
                            .current_url
                            .as_deref()
                            .and_then(|u| self.catalog.portal().live_stream_id(u));
                        if let Some(sid) = sid.filter(|s| self.is_refused(*s)) {
                            if !self.play_fallback_for(sid, self.current_idx) {
                                self.stall_watchdog_armed = false;
                                self.set_toast(self.t(T::ChannelNotInSubscription));
                            }
                            continue;
                        }
                    }
                    // eof/error mean the stream won't resume on its own: disarm
                    // the silent-stall watchdog so it doesn't reload a finished
                    // file on a loop. A plain stop (channel switch) leaves the
                    // watchdog armed for the stream we just loaded.
                    if reason == "eof" || reason == "error" {
                        self.stall_watchdog_armed = false;
                    }
                    if reason == "error" || (reason == "eof" && live) {
                        self.set_toast(self.t(T::StreamErrorRetry));
                    } else {
                        tracing::debug!("mpv end-file ({}) - toast suppressed", reason);
                    }
                }
                Event::Error { msg } => {
                    warn!("player error: {}", msg);
                    self.set_toast(self.tr(T::ErrorMsg, &[&msg]));
                }
                Event::PropertyChanged { name, value } => {
                    if name == "paused-for-cache" {
                        let on = value == "yes";
                        let was = self.health.is_buffering();
                        let prev_count = self.health.freeze_count();
                        let now = Instant::now();
                        self.health.on_buffering_changed(now, on);
                        if on && !was {
                            // elapsed() is Some only for a *counted* freeze;
                            // startup/zap buffering yields None.
                            if self.health.elapsed(now).is_some() {
                                tracing::info!("playback froze (cache underrun)");
                            } else {
                                tracing::debug!("startup/zap buffering");
                            }
                        } else if !on && was {
                            if self.health.freeze_count() > prev_count {
                                tracing::info!(
                                    "playback resumed after {:.1}s - freeze #{}, total {:.1}s",
                                    self.health.last_freeze().unwrap_or_default().as_secs_f64(),
                                    self.health.freeze_count(),
                                    self.health.total_frozen().as_secs_f64(),
                                );
                            } else {
                                tracing::debug!("startup/zap buffering ended");
                            }
                        }
                    }
                }
            }
        }
    }

    fn drain_epg(&mut self) {
        let arrived = self.epg_slot.lock().take();
        if let Some((sid, epg)) = arrived {
            // Gate on current_stream_id (single source of truth) instead of
            // the racy epg_fetch_pending_for. After rapid A->B->C zaps the
            // earlier fetches still arrive; if they don't match the channel
            // we're now watching, discard. If C's fetch is still in flight
            // by the time A's lands, A is correctly dropped.
            if Some(sid) == self.current_stream_id {
                self.epg_fetch_pending_for = None;
                let now = chrono::Utc::now();
                let entries = epg.entries().len();
                if let Some(now_prog) = epg.current_at(now) {
                    let name = self
                        .current_name
                        .clone()
                        .unwrap_or_else(|| "?".into());
                    let start = now_prog.start.with_timezone(&chrono::Local);
                    let end = now_prog.end.with_timezone(&chrono::Local);
                    tracing::info!(
                        "epg landed for sid={} ({} entries) -> enrich toast: {} | {} ({}-{})",
                        sid,
                        entries,
                        name,
                        now_prog.title,
                        start.format("%H:%M"),
                        end.format("%H:%M")
                    );
                    // Reset the 4s toast timer so user sees the enriched
                    // title even if the bare-channel toast had already
                    // ticked away.
                    self.set_toast(format!(
                        "[TV] {}  |  {}  ({}-{})",
                        name,
                        now_prog.title,
                        start.format("%H:%M"),
                        end.format("%H:%M")
                    ));
                } else {
                    tracing::info!(
                        "epg landed for sid={} ({} entries) - no current programme at {}",
                        sid,
                        entries,
                        now.with_timezone(&chrono::Local).format("%H:%M:%S")
                    );
                }
                self.current_epg = Some(epg);
            } else {
                tracing::debug!(
                    "epg arrived for sid={} (now on {:?}) - discarded",
                    sid,
                    self.current_stream_id
                );
            }
        }
    }

    fn kick_epg_fetch(&mut self, stream_id: i64) {
        let portal = self.catalog.portal().clone();
        let slot = self.epg_slot.clone();
        self.epg_fetch_pending_for = Some(stream_id);

        // EPG location is portal-dependent. MOST portals populate EPG on the
        // live stream_id; a few (e.g. hnlol) only populate the tv_archive=1
        // "twin" with the same name. v0.2.0 fetched the live sid directly; the
        // archive-twin redirect added for hnlol then broke normal portals
        // whose twins return empty. So: try the live sid first and fall back
        // to the twin only when the live fetch comes back empty - correct for
        // both portal styles. The result is always stored under the live sid
        // so drain_epg's current_stream_id match still works.
        let channels = self.catalog.live_channels();
        let (name, is_archive) = channels
            .iter()
            .find(|c| c.stream_id == stream_id)
            .map(|c| (c.name.clone(), c.tv_archive == 1))
            .unwrap_or_else(|| (String::new(), false));
        let twin_sid = if !is_archive && !name.is_empty() {
            self.catalog
                .archive_id_by_name(&name)
                .filter(|&t| t != stream_id)
        } else {
            None
        };
        tracing::info!(
            "epg fetch starting for sid={} (twin fallback={:?})",
            stream_id,
            twin_sid
        );
        tokio::spawn(async move {
            // Primary: the live sid itself (what v0.2.0 did, works for normal portals).
            let mut epg = match portal.fetch_epg(stream_id).await {
                Ok(e) => e,
                Err(e) => {
                    tracing::warn!("epg direct fetch failed for sid={}: {}", stream_id, e);
                    Epg::new(Vec::new())
                }
            };
            // Fallback: archive twin, only when the live sid returned nothing.
            if epg.entries().is_empty() {
                if let Some(twin) = twin_sid {
                    tracing::info!(
                        "epg empty on live sid={}, retrying via archive twin {}",
                        stream_id,
                        twin
                    );
                    match portal.fetch_epg(twin).await {
                        Ok(e) if !e.entries().is_empty() => {
                            tracing::info!(
                                "epg twin ok for sid={} ({} entries via {})",
                                stream_id,
                                e.entries().len(),
                                twin
                            );
                            epg = e;
                        }
                        Ok(_) => {
                            tracing::info!("epg twin {} also empty for sid={}", twin, stream_id)
                        }
                        Err(e) => tracing::warn!(
                            "epg twin fetch failed for sid={} (twin {}): {}",
                            stream_id,
                            twin,
                            e
                        ),
                    }
                }
            } else {
                tracing::info!(
                    "epg direct ok for sid={} ({} entries)",
                    stream_id,
                    epg.entries().len()
                );
            }
            *slot.lock() = Some((stream_id, epg));
        });
    }

    fn zap_to(&mut self, sid: i64, name: &str, idx: Option<usize>) {
        // Refused to this account earlier this session: straight to the
        // channel's next best variant instead of failing again.
        if self.is_refused(sid) && self.play_fallback_for(sid, idx) {
            return;
        }
        let url = self.catalog.portal().live_stream_url(sid);
        tracing::info!(
            "zap: -> {} (stream_id={}, idx={:?}, prev={:?})",
            name,
            sid,
            idx,
            self.current_name
        );
        self.load_url(url, true);
        self.current_idx = idx;
        self.current_name = Some(name.to_owned());
        self.current_stream_id = Some(sid);
        self.current_epg = None;
        self.set_toast(format!("[TV] {}", name));
        self.kick_epg_fetch(sid);
    }

    /// Space bar: pause / resume a film, series or catch-up. Live TV keeps
    /// playing: paused, its relay stream would fall behind the live window.
    fn toggle_pause(&mut self) {
        if self.current_live {
            self.set_toast(self.t(T::PauseLiveOnly));
            return;
        }
        self.paused = !self.paused;
        let _ = self.player.cmd_tx.send(Cmd::Pause(self.paused));
        tracing::info!("pause: {} (space)", if self.paused { "on" } else { "off" });
        if self.paused {
            // Stays up while paused; any other toast or a new stream replaces it.
            self.set_toast_for(self.t(T::Paused), 24 * 3600);
        } else {
            self.set_toast(self.t(T::Resumed));
        }
    }

    /// Where the playing channel sits in `zap`, also when it was resumed at
    /// startup rather than zapped to (else arrow up lands on the list's end).
    fn zap_index_of_current(&self, zap: &[LiveChannel]) -> Option<usize> {
        let nav = self.catalog.quality_nav(self.current_stream_id?);
        zap_position(zap, nav.ladder.first()?)
    }

    /// Whether the portal refused live stream `sid` to this account this
    /// session (the relay saw its 407 - RTL 4 UHD, every time).
    fn is_refused(&self, sid: i64) -> bool {
        self.relay
            .as_ref()
            .is_some_and(|r| r.refused(&self.catalog.portal().live_stream_url(sid)))
    }

    /// Play the channel's best variant the portal does not refuse instead of
    /// refused `sid` (RTL 4 UHD -> RTL 4 FHD). False when it has none.
    fn play_fallback_for(&mut self, sid: i64, idx: Option<usize>) -> bool {
        let nav = self.catalog.quality_nav(sid);
        let refused = nav
            .pos
            .and_then(|p| nav.ladder.get(p))
            .map_or("(?)", |c| quality_label(quality_rank(&c.name)));
        let Some(alt) = fallback_variant(&nav.ladder, sid, |c| self.is_refused(c.stream_id))
        else {
            return false;
        };
        tracing::info!("zap: stream {} refused by the portal - playing {}", sid, alt.name);
        self.zap_to(alt.stream_id, &alt.name, idx);
        self.set_toast(self.tr(T::VariantUnavailable, &[&alt.name, refused]));
        true
    }

    /// Up/down zapping. A lone press zaps at once; while the user flips
    /// (quick presses, a held key) only the channel names show and the one
    /// they stop on loads - a portal session per keypress gets throttled.
    fn zap_delta(&mut self, delta: i32, repeat: bool) {
        // Zap over the deduped list so up/down skips HD/UHD/FHD/catch-up
        // duplicates of the same channel (one best-quality entry each).
        let zap = self.catalog.zap_channels();
        if zap.is_empty() {
            if !self.catalog.is_loaded() {
                self.set_toast(self.t(T::CatalogLoading));
            } else {
                self.set_toast(self.t(T::NoLiveChannels));
            }
            return;
        }
        let from = self
            .zap_pacer
            .waiting()
            .map(|t| t.2)
            .or(self.current_idx)
            .or_else(|| self.zap_index_of_current(&zap));
        if let Some(i) = shortcuts::next_live_idx(from, zap.len(), delta) {
            let name = zap[i].name.clone();
            let target = (zap[i].stream_id, name.clone(), i);
            match self.zap_pacer.press(Instant::now(), target, repeat) {
                Some((sid, name, i)) => self.zap_to(sid, &name, Some(i)),
                None => self.set_toast(format!("[TV] {}", name)),
            }
        }
    }

    fn zap_by_id(&mut self, sid: i64) {
        let live = self.catalog.live_channels();
        let Some(ch) = live.iter().find(|c| c.stream_id == sid) else {
            return;
        };
        let name = ch.name.clone();
        // Park the up/down cursor on this channel's entry in the deduped zap
        // list (matched by normalized name) so subsequent zapping continues
        // from the right place; play the exact sid requested.
        let key = normalize_channel_name(&name);
        let idx = self
            .catalog
            .zap_channels()
            .iter()
            .position(|c| normalize_channel_name(&c.name) == key);
        self.zap_to(sid, &name, idx);
    }

    /// Switch the current live channel to a higher (`dir = 1`) or lower
    /// (`dir = -1`) quality variant of the SAME channel. Stops with a toast at
    /// the ends, or when there's no other variant. Keeps `current_idx` so up/
    /// down zapping stays parked on this channel. Runs only on a +/- keypress.
    fn quality_step(&mut self, dir: i32) {
        let Some(sid) = self.current_stream_id else {
            self.set_toast(self.t(T::QualityNoLive));
            return;
        };
        let nav = self.catalog.quality_nav(sid);
        if nav.ladder.len() <= 1 {
            self.set_toast(self.t(T::QualityNoOther));
            return;
        }
        let Some(pos) = nav.pos else {
            self.set_toast(self.t(T::QualityUnknown));
            return;
        };
        let new = pos as i32 + dir;
        if new < 0 {
            self.set_toast(self.t(T::QualityLowest));
            return;
        }
        if new as usize >= nav.ladder.len() {
            self.set_toast(self.t(T::QualityHighest));
            return;
        }
        let target = &nav.ladder[new as usize];
        let (tsid, tname) = (target.stream_id, target.name.clone());
        let rank = quality_rank(&tname);
        self.zap_to(tsid, &tname, self.current_idx);
        // Override zap_to's "[TV] name" toast with explicit quality feedback:
        // the channel is unchanged, the user wants to see the new tier.
        let key = if dir > 0 {
            T::QualityUp
        } else {
            T::QualityDown
        };
        self.set_toast(self.tr(key, &[quality_label(rank)]));
    }

    fn play_movie(&mut self, sid: i64, name: &str) {
        let ext = self.catalog.movie_extension(sid);
        let url = self.catalog.portal().movie_stream_url(sid, &ext);
        self.load_url(url, false);
        self.current_name = Some(name.to_owned());
        self.current_idx = None;
        self.current_stream_id = None;
        self.current_epg = None;
        self.set_toast(format!("[{}] {}", self.t(T::TagMovie), name));
    }

    /// Find and play the most recent NOS Journaal (npo=true) or RTL Nieuws
    /// (npo=false) via catch-up. Resolves the news channel's archive
    /// (tv_archive=1) twin - where both the EPG and the timeshift live -
    /// fetches its day-EPG async, and hands the result to drain_news.
    /// Open the `n` news chooser: fetch NOS Journaal (NPO 1+2) and RTL Nieuws
    /// (RTL 4) catch-up EPG concurrently, build one newest-first list (max 5),
    /// then pop the modal picker. Fully async - the current channel keeps
    /// playing and the toast is just a projection.
    fn open_news_picker(&mut self) {
        const JOURNAAL_EXCL: &[&str] = &["sport", "jeugd", "regio", "makkelijke taal", "gebaren"];
        const SOURCES: &[NewsSrc] = &[
            NewsSrc { chan: "npo1", title: "journaal", excl: JOURNAAL_EXCL, tag: "NOS", offset: 0 },
            NewsSrc { chan: "npo2", title: "journaal", excl: JOURNAAL_EXCL, tag: "NOS", offset: 0 },
            NewsSrc { chan: "rtl4", title: "rtl nieuws", excl: &[], tag: "RTL", offset: 30 },
            NewsSrc { chan: "cnn", title: "", excl: &[], tag: "CNN", offset: 0 },
            NewsSrc { chan: "bbcnews", title: "", excl: &[], tag: "BBC", offset: 0 },
            NewsSrc { chan: "vrt1", title: "journaal", excl: &[], tag: "VRT", offset: 0 },
            NewsSrc { chan: "omroepzeeland", title: "nieuws", excl: &[], tag: "ZLD", offset: 0 },
        ];

        // Resolve each source to its catch-up (tv_archive=1) channel - that's
        // where both the EPG and the timeshift live. Exact normalized match
        // first, then substring (handles "CNN" vs "CNN International").
        let live = self.catalog.live_channels();
        let mut sources: Vec<(i64, bool, &'static NewsSrc)> = Vec::new();
        for src in SOURCES {
            // Prefer the catch-up (tv_archive=1) twin. If there's none - e.g.
            // CNN / BBC, 24h news with no catch-up - fall back to the LIVE
            // channel and play it live.
            let pick = |archive: bool| {
                live.iter()
                    .find(|c| {
                        (c.tv_archive == 1) == archive
                            && normalize_channel_name(&c.name) == src.chan
                    })
                    .or_else(|| {
                        live.iter().find(|c| {
                            (c.tv_archive == 1) == archive
                                && normalize_channel_name(&c.name).contains(src.chan)
                        })
                    })
                    .map(|c| c.stream_id)
            };
            let resolved = pick(true)
                .map(|sid| (sid, true))
                .or_else(|| pick(false).map(|sid| (sid, false)));
            match resolved {
                Some((sid, _)) if sources.iter().any(|(s, _, _)| *s == sid) => {}
                Some((sid, is_archive)) => sources.push((sid, is_archive, src)),
                None => tracing::info!("news source {} ({}) not in catalog", src.tag, src.chan),
            }
        }
        if sources.is_empty() {
            self.set_toast(self.t(T::NoNewsChannels));
            return;
        }
        self.set_toast(self.t(T::FetchingNews));
        let portal = self.catalog.portal().clone();
        let slot = self.news_picker_slot.clone();
        tokio::spawn(async move {
            let now = chrono::Utc::now();
            let handles: Vec<_> = sources
                .into_iter()
                .map(|(sid, is_archive, src)| {
                    let p = portal.clone();
                    (
                        sid,
                        is_archive,
                        src,
                        tokio::spawn(async move { p.fetch_day_epg(sid).await.ok() }),
                    )
                })
                .collect();
            let mut items: Vec<NewsItem> = Vec::new();
            for (sid, is_archive, src, h) in handles {
                let epg = h.await.ok().flatten();
                if !is_archive {
                    // 24h news channel (CNN/BBC): no catch-up - one live item,
                    // titled with whatever it's showing now if EPG is available.
                    let title = epg
                        .as_ref()
                        .and_then(|e| e.current_at(now))
                        .map(|e| e.title.clone())
                        .unwrap_or_else(|| "Live".to_string());
                    items.push(NewsItem {
                        sid,
                        title,
                        start: now,
                        end: now,
                        tag: src.tag.to_string(),
                        offset: 0,
                        live: true,
                    });
                    continue;
                }
                if let Some(epg) = epg {
                    // Per source: matching past entries, newest first, capped at
                    // 3 so a 24h news channel can't flood the combined list.
                    let mut found: Vec<NewsItem> = epg
                        .entries()
                        .iter()
                        .filter(|e| e.start <= now)
                        .filter(|e| {
                            let t = e.title.to_lowercase();
                            (src.title.is_empty() || t.contains(src.title))
                                && !src.excl.iter().any(|x| t.contains(x))
                        })
                        .map(|e| NewsItem {
                            sid,
                            title: e.title.clone(),
                            start: e.start,
                            end: e.end,
                            tag: src.tag.to_string(),
                            offset: src.offset,
                            live: false,
                        })
                        .collect();
                    found.sort_by(|a, b| b.start.cmp(&a.start));
                    found.truncate(3);
                    items.extend(found);
                }
            }
            items.sort_by(|a, b| b.start.cmp(&a.start)); // newest first across sources
            items.truncate(10);
            tracing::info!(
                "news picker {} items: {:?}",
                items.len(),
                items
                    .iter()
                    .map(|i| format!(
                        "{} {}@{}",
                        i.tag,
                        i.title,
                        i.start.with_timezone(&chrono::Local).format("%H:%M")
                    ))
                    .collect::<Vec<_>>()
            );
            *slot.lock() = Some(items);
        });
    }

    /// When the async news list lands, pop the picker (newest pre-selected).
    fn drain_news_picker(&mut self) {
        if let Some(items) = self.news_picker_slot.lock().take() {
            self.news_picker = Some(NewsPicker { items, selected: 0 });
        }
    }

    /// Arrow keys / Enter / Esc for the open news picker.
    fn handle_news_picker_keys(&mut self, ctx: &egui::Context) {
        let (up, down, enter, esc) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowUp),
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::Enter),
                i.key_pressed(Key::Escape),
            )
        });
        if esc {
            self.news_picker = None;
            return;
        }
        let chosen = {
            let Some(p) = self.news_picker.as_mut() else {
                return;
            };
            if up {
                p.selected = p.selected.saturating_sub(1);
            }
            if down && !p.items.is_empty() {
                p.selected = (p.selected + 1).min(p.items.len() - 1);
            }
            if enter {
                p.items.get(p.selected).cloned()
            } else {
                None
            }
        };
        if let Some(item) = chosen {
            self.news_picker = None;
            self.play_news_item(&item);
        }
    }

    /// Play a chosen news bulletin via catch-up (RTL skips 30s of ads).
    fn play_news_item(&mut self, item: &NewsItem) {
        if item.live {
            // 24h news channel (CNN/BBC): no catch-up, just play it live.
            let url = self.catalog.portal().live_stream_url(item.sid);
            self.load_url(url, true);
            self.current_name = Some(format!("{} - {}", item.tag, item.title));
            self.current_idx = None;
            self.current_stream_id = Some(item.sid);
            self.current_epg = None;
            self.set_toast_for(format!("[{}] {}", item.tag, item.title), 30);
            return;
        }
        let off = item.offset;
        let start = item.start + chrono::Duration::seconds(off);
        let dur = ((item.end - start).num_minutes() as u32).max(1);
        let url = self.catalog.portal().catchup_url(item.sid, start, dur);
        let label = format!(
            "[{}] {}  {}",
            self.t(T::TagReplay),
            item.title,
            item.start.with_timezone(&chrono::Local).format("%H:%M")
        );
        tracing::info!("news pick -> {}", label);
        self.load_url(url, false);
        self.current_name = Some(label.clone());
        self.current_idx = None;
        self.current_stream_id = None;
        self.current_epg = None;
        self.set_toast_for(label, 30);
    }

    fn paint_news_picker(&self, ctx: &egui::Context) {
        let Some(picker) = &self.news_picker else {
            return;
        };
        egui::Area::new(egui::Id::new("__news_picker__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_rgb(20, 20, 26))
                    .inner_margin(egui::Margin::symmetric(18.0, 16.0))
                    .show(ui, |ui| {
                        ui.set_width(540.0);
                        ui.label(
                            egui::RichText::new(self.t(T::NewsTitle))
                                .heading()
                                .color(Color32::WHITE),
                        );
                        ui.add_space(10.0);
                        if picker.items.is_empty() {
                            ui.label(
                                egui::RichText::new(self.t(T::NoRecentNews))
                                    .italics()
                                    .color(Color32::from_white_alpha(150)),
                            );
                        }
                        for (i, item) in picker.items.iter().enumerate() {
                            let selected = i == picker.selected;
                            let time = item
                                .start
                                .with_timezone(&chrono::Local)
                                .format("%H:%M")
                                .to_string();
                            let tag = item.tag.as_str();
                            let tag_col = match tag {
                                "RTL" => Color32::from_rgb(225, 80, 80),
                                "NOS" => Color32::from_rgb(245, 150, 40),
                                "CNN" => Color32::from_rgb(200, 50, 50),
                                "BBC" => Color32::from_rgb(190, 30, 40),
                                "VRT" => Color32::from_rgb(80, 160, 220),
                                "ZLD" => Color32::from_rgb(90, 180, 90),
                                _ => Color32::from_rgb(120, 120, 130),
                            };
                            let bg = if selected {
                                Color32::from_rgb(42, 58, 84)
                            } else {
                                Color32::from_rgb(26, 26, 32)
                            };
                            egui::Frame::none()
                                .fill(bg)
                                .rounding(6.0)
                                .inner_margin(egui::Margin::symmetric(12.0, 9.0))
                                .show(ui, |ui| {
                                    ui.horizontal(|ui| {
                                        ui.label(
                                            egui::RichText::new(&time)
                                                .monospace()
                                                .size(17.0)
                                                .color(Color32::from_white_alpha(190)),
                                        );
                                        ui.add_space(8.0);
                                        egui::Frame::none()
                                            .fill(tag_col)
                                            .rounding(4.0)
                                            .inner_margin(egui::Margin::symmetric(5.0, 1.0))
                                            .show(ui, |ui| {
                                                ui.label(
                                                    egui::RichText::new(tag)
                                                        .size(11.0)
                                                        .strong()
                                                        .color(Color32::from_rgb(20, 20, 20)),
                                                );
                                            });
                                        ui.add_space(8.0);
                                        ui.label(
                                            egui::RichText::new(&item.title)
                                                .size(17.0)
                                                .strong()
                                                .color(Color32::WHITE),
                                        );
                                    });
                                });
                            ui.add_space(4.0);
                        }
                        ui.add_space(8.0);
                        ui.label(
                            egui::RichText::new(self.t(T::NewsFooter))
                                .size(12.0)
                                .color(Color32::from_white_alpha(130)),
                        );
                    });
            });
    }

    fn toggle_favorite_current(&mut self) {
        let Some(sid) = self.current_stream_id else {
            self.set_toast(self.t(T::NoChannelToFavorite));
            return;
        };
        let name = self.current_name.clone().unwrap_or_default();
        let was = self.favorites.contains(sid);
        self.favorites.toggle(sid, &name);
        let _ = self.favorites.save(&self.storage.favorites_path());
        if was {
            self.set_toast(self.tr(T::FavoriteRemoved, &[&name]));
        } else {
            self.set_toast(self.tr(T::FavoriteAdded, &[&name]));
        }
    }

    /// Car-radio number presets on digit keys 0-9: hold a digit
    /// (>= LONG_PRESS) to store the current channel in that slot, tap it to
    /// recall the stored channel. Called from handle_keys (viewer only - in
    /// the guide, digits feed the channel filter instead).
    fn handle_number_presets(&mut self, ctx: &egui::Context) {
        const DIGIT_KEYS: [Key; 10] = [
            Key::Num0, Key::Num1, Key::Num2, Key::Num3, Key::Num4,
            Key::Num5, Key::Num6, Key::Num7, Key::Num8, Key::Num9,
        ];
        const LONG_PRESS: Duration = Duration::from_millis(550);
        for (d, key) in DIGIT_KEYS.iter().enumerate() {
            let (pressed, down, released) = ctx.input(|i| {
                (i.key_pressed(*key), i.key_down(*key), i.key_released(*key))
            });
            // Start timing on the first press; ignore OS key-repeat presses so
            // a held key keeps its original down-time.
            if pressed && self.digit_down_since[d].is_none() {
                self.digit_down_since[d] = Some(Instant::now());
                self.digit_long_fired[d] = false;
            }
            // Long-press: fire the save once we cross the threshold while held.
            if down && !self.digit_long_fired[d] {
                if let Some(t0) = self.digit_down_since[d] {
                    if t0.elapsed() >= LONG_PRESS {
                        self.digit_long_fired[d] = true;
                        self.save_preset(d as u8);
                    }
                }
            }
            if released {
                // Tap (released before the long-press fired) -> recall.
                if self.digit_down_since[d].is_some() && !self.digit_long_fired[d] {
                    self.recall_preset(d as u8);
                }
                self.digit_down_since[d] = None;
                self.digit_long_fired[d] = false;
            }
        }
    }

    fn save_preset(&mut self, digit: u8) {
        let Some(sid) = self.current_stream_id else {
            self.set_toast(self.tr(T::PresetNoLive, &[&digit.to_string()]));
            return;
        };
        let name = self.current_name.clone().unwrap_or_default();
        self.presets.set(digit, sid, &name);
        let _ = self.presets.save(&self.storage.presets_path());
        self.set_toast(self.tr(T::PresetSaved, &[&digit.to_string(), &name]));
    }

    fn recall_preset(&mut self, digit: u8) {
        match preset_target(&self.presets, digit, &self.catalog) {
            Some(sid) => self.zap_by_id(sid),
            None => self.set_toast(self.tr(T::PresetEmpty, &[&digit.to_string()])),
        }
    }

    /// First-run portal setup: shown when no credentials are configured.
    /// Collects host/user/pass, tests them, and once they log in writes
    /// BongoPlayer.ini and connects live.
    fn paint_portal_prompt(&mut self, ctx: &egui::Context) {
        let mut submit: Option<String> = None;
        let lang = self.lang;
        egui::Area::new(egui::Id::new("__portal_prompt__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_rgb(22, 22, 28))
                    .inner_margin(egui::Margin::symmetric(20.0, 18.0))
                    .show(ui, |ui| {
                        ui.set_width(430.0);
                        ui.label(
                            egui::RichText::new(crate::i18n::t(lang, T::PortalSetupTitle))
                                .heading()
                                .color(Color32::WHITE),
                        );
                        ui.label(
                            egui::RichText::new(crate::i18n::t(lang, T::PortalSetupIntro))
                                .color(Color32::from_white_alpha(150))
                                .size(12.0),
                        );
                        ui.add_space(12.0);
                        egui::Grid::new("__portal_grid__")
                            .num_columns(2)
                            .spacing([10.0, 8.0])
                            .show(ui, |ui| {
                                ui.label(crate::i18n::t(lang, T::ServerLabel));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.cfg_host)
                                        .hint_text("server.example.com:8080")
                                        .desired_width(250.0),
                                );
                                ui.end_row();
                                ui.label(crate::i18n::t(lang, T::UserLabel));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.cfg_user)
                                        .hint_text(crate::i18n::t(lang, T::UserHint))
                                        .desired_width(250.0),
                                );
                                ui.end_row();
                                ui.label(crate::i18n::t(lang, T::PasswordLabel));
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.cfg_pass)
                                        .password(true)
                                        .desired_width(250.0),
                                );
                                ui.end_row();
                            });
                        ui.add_space(12.0);
                        let ready = !self.cfg_host.trim().is_empty()
                            && !self.cfg_user.trim().is_empty()
                            && !self.cfg_pass.is_empty()
                            && self.conn_state != ConnState::Testing;
                        let clicked = ui
                            .add_enabled(
                                ready,
                                egui::Button::new(crate::i18n::t(lang, T::ConnectAndSave)),
                            )
                            .clicked();
                        if let Some(text) = conn_rich_text(lang, &self.conn_state) {
                            ui.label(text.size(12.0));
                        }
                        let entered = ready && ctx.input(|i| i.key_pressed(Key::Enter));
                        if clicked || entered {
                            submit = Some(format!(
                                "{}:{}@{}",
                                self.cfg_user.trim(),
                                self.cfg_pass,
                                self.cfg_host.trim()
                            ));
                        }
                    });
            });
        if let Some(s) = submit {
            // Saving tests first: only credentials that log in are kept.
            self.start_conn_check(s, true);
        }
    }

    /// Test credentials in the background; with `save`, store and connect
    /// them once the login works. A failed test changes nothing.
    fn start_conn_check(&mut self, creds_str: String, save: bool) {
        let creds = match parse_xtream_creds(&creds_str) {
            Ok(c) => c,
            Err(e) => {
                self.conn_state = ConnState::BadInput(creds_error_text(&e));
                return;
            }
        };
        self.conn_state = ConnState::Testing;
        let slot = self.conn_slot.clone();
        tokio::spawn(async move {
            let portal = XtreamPortal::new(creds);
            let check = portal.check().await;
            let keep = save.then_some((portal.creds, creds_str));
            *slot.lock() = Some(ConnResult { check, keep });
        });
    }

    /// When a credentials test has finished: say how it went and, if it was
    /// to save and the login worked, save and connect. Whether the channel
    /// count came does not matter: a login that works is all a save needs.
    fn drain_conn_check(&mut self) {
        let Some(ConnResult { check, keep }) = self.conn_slot.lock().take() else {
            return;
        };
        tracing::info!("settings: connection check {:?}", check);
        self.conn_state = match (check, keep) {
            (check @ ConnCheck::Ok { .. }, Some((creds, line))) => {
                match self.apply_creds(creds, &line) {
                    Ok(()) => ConnState::Done(check),
                    Err(failed) => failed,
                }
            }
            (check, _) => ConnState::Done(check),
        };
    }

    /// Keep credentials that logged in: write BongoPlayer.ini (`line` is what
    /// goes in it), then rebuild the catalog live and start fetching - no
    /// restart needed. When the ini cannot be written nothing changes and the
    /// failure comes back, to show in place of the test's verdict.
    fn apply_creds(&mut self, creds: XtreamCreds, line: &str) -> Result<(), ConnState> {
        let path = crate::args::ini_path();
        if let Err(e) = crate::args::write_ini_creds(&path, line) {
            let (path, error) = (path.display().to_string(), e.to_string());
            self.set_toast(self.tr(T::CannotWrite, &[&path, &error]));
            return Err(ConnState::WriteFailed { path, error });
        }
        tracing::info!("settings: portal saved to {} - connecting", path.display());
        // The new catalog starts locked, so a save locks like Lock does: asked of
        // the old catalog, the only one that knows this stream, before it goes.
        let playing_adult = self.playing_adult();
        self.creds = Some(creds.clone());
        let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(creds));
        self.catalog = Arc::new(CatalogStore::new(portal));
        // Another portal: its stream ids mean other channels, so the old lists'
        // positions and the guide's per-channel data are of no use. The new
        // catalog starts locked, so the last PIN outcome no longer applies.
        self.forget_list_positions();
        self.guide.forget_portal();
        self.adult_note = None;
        if playing_adult {
            self.stop_playback();
        }
        self.catalog.spawn_fetch();
        self.portal_state = PortalState::Configured;
        self.set_toast(self.t(T::ConnectingPortal));
        Ok(())
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Search box owns input when open; only handle escape/enter externally.
        if self.show_search {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.show_search = false;
                self.search_query.clear();
            }
            return;
        }

        // News picker is modal: arrows / Enter / Esc only.
        if self.news_picker.is_some() {
            self.handle_news_picker_keys(ctx);
            return;
        }

        let (
            down,
            up,
            arrow_repeat,
            space,
            left,
            right,
            scroll,
            f_key,
            e_key,
            shift,
            d,
            f11,
            esc,
            n_key,
            a_key,
            s_key,
            star,
            f5,
            g_key,
            t_key,
            qmark,
            plus,
            minus,
        ) = ctx.input(|i| {
            (
                i.key_pressed(Key::ArrowDown),
                i.key_pressed(Key::ArrowUp),
                i.events.iter().any(|e| {
                    matches!(
                        e,
                        egui::Event::Key {
                            key: Key::ArrowDown | Key::ArrowUp,
                            pressed: true,
                            repeat: true,
                            ..
                        }
                    )
                }),
                i.key_pressed(Key::Space),
                i.key_pressed(Key::ArrowLeft),
                i.key_pressed(Key::ArrowRight),
                i.raw_scroll_delta.y,
                i.key_pressed(Key::F),
                i.key_pressed(Key::E),
                i.modifiers.shift,
                i.key_pressed(Key::D),
                i.key_pressed(Key::F11),
                i.key_pressed(Key::Escape),
                i.key_pressed(Key::N),
                i.key_pressed(Key::A),
                i.key_pressed(Key::S),
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "*")),
                i.key_pressed(Key::F5),
                i.key_pressed(Key::G),
                i.key_pressed(Key::T),
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "+")),
                i.events
                    .iter()
                    .any(|e| matches!(e, egui::Event::Text(t) if t == "-")),
            )
        });

        // Help overlay swallows other input - the user is reading the
        // keymap, not zapping. Only `?` and Esc should be live.
        if self.show_help {
            if qmark || esc {
                self.show_help = false;
            }
            return;
        }
        if qmark {
            self.show_help = true;
            return;
        }
        // Consumed, or the overlay would see this same press and close again.
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::O)) {
            self.open_settings();
            return;
        }

        // Down = next channel, unless the user reversed zapping (wheel too).
        let next = if self.settings.swap_zap_keys { -1 } else { 1 };
        if down {
            self.zap_delta(next, arrow_repeat);
        }
        if up {
            self.zap_delta(-next, arrow_repeat);
        }
        if scroll > 0.5 {
            self.zap_delta(-next, false);
        }
        if scroll < -0.5 {
            self.zap_delta(next, false);
        }

        if space {
            self.toggle_pause();
        }

        if left {
            let _ = self.player.cmd_tx.send(Cmd::SeekRelative(-30.0));
            self.set_toast("<< -30s");
        }
        if right {
            let _ = self.player.cmd_tx.send(Cmd::SeekRelative(30.0));
            self.set_toast(">> +30s");
        }

        // + / - : step to a higher / lower quality variant of the current
        // channel (live only). Read as text events like * and ? above.
        if plus {
            self.quality_step(1);
        }
        if minus {
            self.quality_step(-1);
        }

        if f_key && !shift {
            self.show_search = !self.show_search;
            if self.show_search {
                self.search_query.clear();
            }
        }
        if f_key && shift {
            self.show_favs = !self.show_favs;
        }

        if e_key {
            if shift {
                self.show_epg_grid = !self.show_epg_grid;
            } else {
                self.show_epg_strip = !self.show_epg_strip;
            }
        }

        if d {
            self.show_debug = !self.show_debug;
        }

        // Digit keys 0-9 are car-radio presets: tap = recall, hold = store.
        self.handle_number_presets(ctx);

        if n_key {
            self.open_news_picker();
        }

        if a_key {
            let _ = self.player.cmd_tx.send(Cmd::CycleAudio);
            self.set_toast(self.t(T::CycleAudio));
        }
        if s_key {
            let _ = self.player.cmd_tx.send(Cmd::CycleSubtitle);
            self.set_toast(self.t(T::CycleSubtitle));
        }

        if f11 {
            let want = !ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(want));
        }

        if esc {
            self.show_favs = false;
            self.show_epg_strip = false;
            self.show_epg_grid = false;
            self.show_debug = false;
            self.search_query.clear();
        }

        if star {
            self.toggle_favorite_current();
        }

        if f5 {
            match self.portal_state {
                PortalState::Configured => {
                    self.catalog.spawn_fetch();
                    self.set_toast(self.t(T::RetryingPortal));
                }
                PortalState::Missing | PortalState::Placeholder => {
                    self.set_toast(self.t(T::NoPortalOpenSettings));
                }
            }
        }

        if g_key {
            self.toggle_guide();
        }

        if t_key {
            self.toggle_borderless(ctx);
        }
    }

    fn toggle_borderless(&mut self, ctx: &egui::Context) {
        self.borderless = !self.borderless;
        if self.borderless {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(false));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::AlwaysOnTop,
            ));
            self.set_toast(self.t(T::BorderlessOn));
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Decorations(true));
            ctx.send_viewport_cmd(egui::ViewportCommand::WindowLevel(
                egui::WindowLevel::Normal,
            ));
            self.set_toast(self.t(T::BorderlessOff));
        }
    }

    fn update_video_texture(&mut self, ctx: &egui::Context) {
        let frame: RgbaFrame = self.player.frames.read();
        if frame.w == 0 || frame.h == 0 {
            return;
        }
        // After a stop the last picture must not come back.
        if !self.frame_fence.admit(frame.version) {
            return;
        }
        if frame.version == self.last_frame_version && self.video_tex.is_some() {
            return;
        }
        self.last_frame_version = frame.version;

        // mpv emits rgb0: bytes are R, G, B, 0. egui ColorImage wants RGBA.
        // Just force the 4th byte to 255 (opaque).
        let mut rgba = frame.data.clone();
        for px in rgba.chunks_exact_mut(4) {
            px[3] = 255;
        }
        let img = ColorImage::from_rgba_unmultiplied([frame.w as usize, frame.h as usize], &rgba);
        match self.video_tex.as_mut() {
            Some(t) => t.set(img, TextureOptions::LINEAR),
            None => self.video_tex = Some(ctx.load_texture("video", img, TextureOptions::LINEAR)),
        }
    }

    fn forward_window_size(&self, ctx: &egui::Context) {
        if let Some(rect) = ctx.input(|i| i.viewport().inner_rect) {
            let w = rect.width().max(1.0) as u32;
            let h = rect.height().max(1.0) as u32;
            let _ = self.player.cmd_tx.send(Cmd::SetWindowSize(w, h));
        }
    }

    fn paint_toast(&self, ctx: &egui::Context) {
        let Some((text, t0, dur)) = &self.toast else {
            return;
        };
        if t0.elapsed() > *dur {
            return;
        }
        egui::Area::new(egui::Id::new("__toast__"))
            // Display only: an interactable Area's frame is a focusable widget,
            // an invisible Tab stop in front of whatever has the keyboard.
            .interactable(false)
            .anchor(egui::Align2::LEFT_BOTTOM, [12.0, -12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(Color32::WHITE).heading());
                    });
            });
    }

    fn paint_search(&mut self, ctx: &egui::Context) {
        if !self.show_search {
            return;
        }
        let mut zap_pick: Option<(i64, String, ItemKind)> = None;
        let lang = self.lang;

        egui::Area::new(egui::Id::new("__search__"))
            .anchor(egui::Align2::CENTER_TOP, [0.0, 60.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(220))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        // Arrows pick a result. Taken before the text box sees
                        // them: it would jump its cursor to the start / end,
                        // and what is typed next would land there.
                        let (down, up) = ui.input_mut(|i| {
                            (
                                i.count_and_consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                                i.count_and_consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                            )
                        });
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut self.search_query)
                                .hint_text(crate::i18n::t(lang, T::SearchHint))
                                .desired_width(540.0),
                        );
                        resp.request_focus();

                        let key = (self.search_query.clone(), self.catalog.is_loaded());
                        if self.search_hits_for.as_ref() != Some(&key) {
                            self.search_hits = self.catalog.search(&self.search_query);
                            self.search_hits_for = Some(key);
                            self.search_sel = 0;
                        }
                        let shown = self.search_hits.len().min(12);
                        self.search_sel =
                            search::step(self.search_sel, shown, down as i32 - up as i32);
                        ui.separator();
                        for (i, it) in self.search_hits.iter().take(12).enumerate() {
                            let tag = match it.kind {
                                ItemKind::Live => "[LIVE]".to_string(),
                                ItemKind::Movie => {
                                    format!("[{}]", crate::i18n::t(lang, T::TagMovie))
                                }
                                ItemKind::Series => {
                                    format!("[{}]", crate::i18n::t(lang, T::TagSeries))
                                }
                            };
                            if ui
                                .selectable_label(
                                    i == self.search_sel,
                                    format!("{} {}", tag, it.name),
                                )
                                .clicked()
                            {
                                zap_pick = Some((it.id, it.name.clone(), it.kind));
                            }
                        }
                        if ctx.input(|i| i.key_pressed(Key::Enter)) {
                            if let Some(it) = self.search_hits.get(self.search_sel) {
                                zap_pick = Some((it.id, it.name.clone(), it.kind));
                            }
                        }
                    });
            });

        if let Some((id, name, kind)) = zap_pick {
            self.handle_search_pick(id, &name, kind);
        }
    }

    fn handle_search_pick(&mut self, id: i64, name: &str, kind: ItemKind) {
        match kind {
            ItemKind::Live => {
                self.zap_by_id(id);
            }
            ItemKind::Movie => {
                self.play_movie(id, name);
            }
            ItemKind::Series => {
                self.set_toast(self.tr(T::SeriesNotYet, &[name]));
            }
        }
        self.show_search = false;
        self.search_query.clear();
    }

    fn paint_favs(&mut self, ctx: &egui::Context) {
        if !self.show_favs {
            return;
        }
        let mut zap_id: Option<i64> = None;
        let mut remove_id: Option<i64> = None;
        let entries = shown_favorites(&self.favorites, &self.catalog);
        let lang = self.lang;

        egui::Area::new(egui::Id::new("__favs__"))
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(230))
                    .show(ui, |ui| {
                        ui.set_width(420.0);
                        ui.heading(crate::i18n::t(lang, T::FavoritesTitle));
                        ui.separator();
                        if entries.is_empty() {
                            ui.label(
                                egui::RichText::new(crate::i18n::t(lang, T::FavoritesEmpty))
                                    .italics(),
                            );
                        }
                        for entry in entries {
                            ui.horizontal(|ui| {
                                if ui.selectable_label(false, &entry.name).clicked() {
                                    zap_id = Some(entry.stream_id);
                                }
                                if ui.small_button("x").clicked() {
                                    remove_id = Some(entry.stream_id);
                                }
                            });
                        }
                    });
            });

        if let Some(id) = zap_id {
            self.zap_by_id(id);
            self.show_favs = false;
        }
        if let Some(id) = remove_id {
            self.favorites.remove(id);
            let _ = self.favorites.save(&self.storage.favorites_path());
        }
    }

    fn paint_epg_strip(&self, ctx: &egui::Context) {
        if !self.show_epg_strip {
            return;
        }
        let Some(epg) = &self.current_epg else {
            return;
        };
        let now = chrono::Utc::now();
        let mut text = String::new();
        if let Some(cur) = epg.current_at(now) {
            text.push_str(&format!(
                "> {}  ({}-{})",
                cur.title,
                crate::epg::hhmm(cur.start),
                crate::epg::hhmm(cur.end)
            ));
        }
        if let Some(nxt) = epg.next_at(now) {
            text.push_str(&format!(
                "    ->  {}  {}",
                crate::epg::hhmm(nxt.start),
                nxt.title
            ));
        }
        if text.is_empty() {
            return;
        }

        egui::Area::new(egui::Id::new("__epg_strip__"))
            .interactable(false) // display only, see paint_toast
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -60.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).color(Color32::WHITE));
                    });
            });
    }

    fn paint_epg_grid(&self, ctx: &egui::Context) {
        if !self.show_epg_grid {
            return;
        }
        let Some(epg) = &self.current_epg else {
            return;
        };
        egui::Area::new(egui::Id::new("__epg_grid__"))
            // No Tab stop for the frame (see paint_toast), but not
            // interactable(false): the list scrolls with the mouse wheel, which
            // needs the pointer to reach this layer.
            .sense(egui::Sense::hover())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(230))
                    .show(ui, |ui| {
                        ui.set_width(560.0);
                        ui.heading(self.t(T::EpgTitle));
                        ui.separator();
                        egui::ScrollArea::vertical()
                            .max_height(420.0)
                            .show(ui, |ui| {
                                for e in epg.entries() {
                                    ui.label(format!(
                                        "{}-{}  {}",
                                        crate::epg::hhmm(e.start),
                                        crate::epg::hhmm(e.end),
                                        e.title
                                    ));
                                }
                            });
                    });
            });
    }

    /// Draws the help overlay; true when the user clicked the settings row.
    fn paint_help(&self, ctx: &egui::Context) -> bool {
        if !self.show_help {
            return false;
        }
        let mut open_settings = false;
        let sections = help_rows(self.lang, self.settings.swap_zap_keys);
        let window_height = ctx.screen_rect().height();
        egui::Area::new(egui::Id::new("__help__"))
            // The settings row is the one Tab stop, not the frame (see
            // paint_toast); not interactable(false) either, or the rows could
            // not scroll with the wheel (see paint_epg_grid).
            .sense(egui::Sense::hover())
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_rgb(20, 20, 26))
                    .inner_margin(egui::Margin::symmetric(16.0, HELP_FRAME_MARGIN))
                    .show(ui, |ui| {
                        ui.set_max_width(720.0);
                        // Visible whenever the rows do not all fit.
                        ui.spacing_mut().scroll = egui::style::ScrollStyle::thin();
                        let title =
                            self.tr(T::HelpTitle, &[crate::APP_NAME, env!("CARGO_PKG_VERSION")]);
                        ui.label(egui::RichText::new(title).color(Color32::WHITE).heading());
                        ui.add_space(8.0);
                        // The help never grows taller than its window: the rows
                        // get what is left after the window's edge, the frame's
                        // margins, the title above them and the footer below,
                        // and scroll when they need more.
                        let above = ui.cursor().min.y - ui.min_rect().min.y;
                        let rows_height = (window_height
                            - 2.0 * (HELP_SCREEN_MARGIN + HELP_FRAME_MARGIN)
                            - above
                            - HELP_FOOTER_HEIGHT)
                            .max(HELP_MIN_ROWS_HEIGHT);
                        let scrolled = egui::ScrollArea::vertical()
                            .id_source("__help_rows__")
                            .max_height(rows_height)
                            // Dragging the rows would be one more Tab stop.
                            .drag_to_scroll(false)
                            .show(ui, |ui| {
                                for (si, (section, rows)) in sections.into_iter().enumerate() {
                                    ui.label(
                                        egui::RichText::new(self.t(section))
                                            .color(Color32::from_rgb(120, 180, 240))
                                            .strong()
                                            .size(13.0),
                                    );
                                    ui.add_space(4.0);
                                    egui::Grid::new(("__help_grid__", si))
                                        .num_columns(2)
                                        .spacing([24.0, 4.0])
                                        .show(ui, |ui| {
                                            for row in &rows {
                                                open_settings |= paint_help_row(ui, row);
                                                ui.end_row();
                                            }
                                        });
                                    ui.add_space(10.0);
                                }
                            });
                        // Up/Down and PageUp/PageDown scroll the rows, wherever
                        // the pointer is (the wheel needs it over them). The
                        // viewer's and the guide's keys are all swallowed while
                        // the help is open, so none of these zaps.
                        let page =
                            (scrolled.inner_rect.height() - HELP_SCROLL_LINE).max(HELP_SCROLL_LINE);
                        let step = ctx.input(|i| {
                            let mut step = 0.0;
                            if i.key_pressed(Key::ArrowDown) {
                                step += HELP_SCROLL_LINE;
                            }
                            if i.key_pressed(Key::ArrowUp) {
                                step -= HELP_SCROLL_LINE;
                            }
                            if i.key_pressed(Key::PageDown) {
                                step += page;
                            }
                            if i.key_pressed(Key::PageUp) {
                                step -= page;
                            }
                            step
                        });
                        if step != 0.0 {
                            let furthest =
                                (scrolled.content_size.y - scrolled.inner_rect.height()).max(0.0);
                            let mut state = scrolled.state;
                            state.offset.y = (state.offset.y + step).clamp(0.0, furthest);
                            state.store(ctx, scrolled.id);
                        }
                        ui.separator();
                        ui.label(
                            egui::RichText::new(self.t(T::HelpFooter))
                                .italics()
                                .color(Color32::from_white_alpha(140))
                                .size(11.0),
                        );
                    });
            });
        open_settings
    }

    fn paint_debug_hud(&self, ctx: &egui::Context) {
        if !self.show_debug {
            return;
        }
        egui::Area::new(egui::Id::new("__debug__"))
            .interactable(false) // display only, see paint_toast
            .anchor(egui::Align2::RIGHT_TOP, [-12.0, 12.0])
            .show(ctx, |ui| {
                egui::Frame::popup(&ctx.style())
                    .fill(Color32::from_black_alpha(200))
                    .show(ui, |ui| {
                        let frame = self.player.frames.read();
                        ui.label(format!("frame v{} {}x{}", frame.version, frame.w, frame.h));
                        // playback health: freeze detection (paused-for-cache).
                        // Colour-coded instead of glyphs — egui's default fonts
                        // don't reliably ship emoji.
                        let now = Instant::now();
                        let (txt, col) = if self.health.is_buffering() {
                            match self.health.elapsed(now) {
                                Some(d) => (
                                    self.tr(
                                        T::DebugBuffering,
                                        &[&format!("{:.1}", d.as_secs_f64())],
                                    ),
                                    Color32::from_rgb(255, 170, 60),
                                ),
                                None => (
                                    self.t(T::DebugBufferingStartup).to_string(),
                                    Color32::from_rgb(210, 210, 130),
                                ),
                            }
                        } else {
                            (
                                self.t(T::DebugLive).to_string(),
                                Color32::from_rgb(120, 220, 120),
                            )
                        };
                        ui.label(egui::RichText::new(txt).color(col));
                        let last = self
                            .health
                            .last_freeze()
                            .map(|d| format!("{:.1}s", d.as_secs_f64()))
                            .unwrap_or_else(|| "-".to_string());
                        ui.label(self.tr(
                            T::DebugFreezes,
                            &[
                                &self.health.freeze_count().to_string(),
                                &format!("{:.1}", self.health.total_frozen().as_secs_f64()),
                                &last,
                            ],
                        ));
                        if let Some(n) = &self.current_name {
                            ui.label(self.tr(T::DebugChannel, &[n]));
                        }
                        ui.label(
                            self.tr(T::DebugFavs, &[&self.favorites.iter().count().to_string()]),
                        );
                        ui.label(
                            self.tr(T::DebugCatalog, &[&format!("{:?}", self.catalog.status())]),
                        );
                        ui.label(self.tr(T::DebugPortal, &[&format!("{:?}", self.portal_state)]));
                    });
            });
    }

    fn paint_empty_state(&self, ui: &mut egui::Ui) {
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.25);
            ui.label(
                egui::RichText::new(crate::APP_NAME)
                    .color(Color32::from_white_alpha(180))
                    .size(48.0),
            );
            ui.add_space(20.0);
            if self.portal_state != PortalState::Configured {
                return;
            }
            match self.catalog.status() {
                CatalogStatus::Idle | CatalogStatus::Connecting | CatalogStatus::Fetching => {
                    let key = match self.catalog.status() {
                        CatalogStatus::Connecting => T::Connecting,
                        CatalogStatus::Fetching => T::FetchingCatalog,
                        _ => T::Preparing,
                    };
                    ui.label(
                        egui::RichText::new(self.t(key))
                            .color(Color32::from_white_alpha(140))
                            .size(20.0),
                    );
                    ui.add_space(8.0);
                    ui.spinner();
                }
                CatalogStatus::Loaded => {
                    ui.label(
                        egui::RichText::new(self.t(T::CatalogLoadedHint))
                            .color(Color32::from_white_alpha(140))
                            .size(18.0),
                    );
                }
                CatalogStatus::Failed(e) => {
                    ui.label(
                        egui::RichText::new(self.t(T::PortalFetchFailed))
                            .color(Color32::from_rgb(255, 120, 120))
                            .size(20.0),
                    );
                    ui.add_space(8.0);
                    // The portal's own error text, credentials already masked:
                    // a technical detail, shown as written.
                    ui.label(egui::RichText::new(e).color(Color32::from_white_alpha(140)));
                    ui.add_space(8.0);
                    ui.label(
                        egui::RichText::new(self.t(T::PressF5))
                            .italics()
                            .color(Color32::from_white_alpha(100)),
                    );
                }
            }
        });
    }

    // ------------------------------------------------------------------
    // Guide (`g` key): NLZIET-style vertical-column EPG browser
    // ------------------------------------------------------------------

    fn toggle_guide(&mut self) {
        if !self.catalog.is_loaded() {
            self.set_toast(self.t(T::GuideCatalogLoading));
            return;
        }
        self.guide.open = !self.guide.open;
        if self.guide.open {
            self.guide.filter.clear();
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
            // Each opened guide starts fresh: re-center every column on
            // its current programme.
            self.guide.auto_center = true;
            // visible list (re)builds on next paint via refresh_visible_if_stale
        }
    }

    /// Re-derive `visible` (channel indices matching the filter + channel
    /// mode) when EITHER input changes. Snapshot encodes both so we can
    /// short-circuit per-frame.
    fn refresh_visible_if_stale(&mut self, channels: &[LiveChannel]) {
        let f = self.guide.filter.trim().to_lowercase();
        let mode = self.guide.channel_mode;
        // Mode, filter and list length: unlocking or locking adult content
        // (or a new portal) changes the list under the cached indices.
        let snapshot = match mode {
            ChannelMode::All => format!("A|{}|{}", channels.len(), f),
            ChannelMode::Live => format!("L|{}|{}", channels.len(), f),
            ChannelMode::Catchup => format!("C|{}|{}", channels.len(), f),
        };
        if self.guide.visible_filter_snapshot == snapshot && !self.guide.visible.is_empty() {
            return;
        }
        let mode_pass = move |c: &LiveChannel| -> bool {
            match mode {
                ChannelMode::All => true,
                ChannelMode::Live => c.tv_archive == 0,
                ChannelMode::Catchup => c.tv_archive == 1,
            }
        };
        self.guide.visible = channels
            .iter()
            .enumerate()
            .filter(|(_, c)| mode_pass(c))
            .filter(|(_, c)| f.is_empty() || c.name.to_lowercase().contains(&f))
            .map(|(i, _)| i)
            .collect();
        self.guide.visible_filter_snapshot = snapshot;
        if self.guide.column_offset >= self.guide.visible.len() {
            self.guide.column_offset = 0;
        }
        self.guide.selected_col = 0;
    }

    /// How many columns fit at the current window width. Adaptive.
    fn guide_num_visible_cols(&self, ctx: &egui::Context) -> usize {
        const COL_PX: f32 = 230.0;
        let w = ctx
            .input(|i| i.viewport().inner_rect.map(|r| r.width()))
            .unwrap_or(1280.0);
        let usable = (w - 60.0).max(COL_PX);
        ((usable / COL_PX) as usize).clamp(1, 10)
    }

    fn guide_visible_channel_at<'a>(
        &self,
        visible_col: usize,
        channels: &'a [LiveChannel],
    ) -> Option<&'a LiveChannel> {
        let abs = self.guide.column_offset.checked_add(visible_col)?;
        let ch_idx = *self.guide.visible.get(abs)?;
        channels.get(ch_idx)
    }

    /// Kick EPG fetches for the visible-column set when it's been stable for
    /// 200 ms and we don't already have / aren't already fetching the data.
    /// Naively fetching on every column-offset change would fire 1 HTTP call
    /// per arrow-press while the user is panning - the debounce makes the
    /// guide feel snappy regardless of catalog size.
    fn guide_kick_visible_fetches(&mut self, channels: &[LiveChannel], num_cols: usize) {
        let visible_ids: Vec<i64> = (0..num_cols)
            .filter_map(|col| self.guide_visible_channel_at(col, channels).map(|c| c.stream_id))
            .collect();
        if visible_ids != self.guide.visible_set_settled_snapshot {
            self.guide.visible_set_settled_snapshot = visible_ids;
            self.guide.visible_set_settled_since = Instant::now();
            return;
        }
        if self.guide.visible_set_settled_since.elapsed() < Duration::from_millis(200) {
            return;
        }
        let cache_g = self.guide.epg_cache.lock();
        let pend_g = self.guide.epg_pending.lock();
        let failed_g = self.guide.epg_failed.lock();
        // Skip channels that are cached, pending, OR have already failed -
        // no point hammering the portal with a request that just errored.
        let to_fetch: Vec<i64> = self
            .guide
            .visible_set_settled_snapshot
            .iter()
            .copied()
            .filter(|sid| {
                !cache_g.contains_key(sid)
                    && !pend_g.contains(sid)
                    && !failed_g.contains(sid)
            })
            .collect();
        drop(failed_g);
        drop(pend_g);
        drop(cache_g);
        for sid in to_fetch {
            // Resolve channel name for clearer logs.
            let ch_meta = channels
                .iter()
                .find(|c| c.stream_id == sid)
                .map(|c| (c.name.clone(), c.tv_archive == 1));
            let (name, is_archive) = ch_meta
                .unwrap_or_else(|| ("?".to_owned(), false));
            self.guide.epg_pending.lock().insert(sid);

            // hnlol-style portals only populate EPG against tv_archive=1
            // stream_ids. For a live channel, try the archive twin's
            // stream_id first; cache the result under the live sid so the
            // UI shows it in the live column.
            let fetch_sid = if !is_archive {
                self.catalog
                    .archive_id_by_name(&name)
                    .unwrap_or(sid)
            } else {
                sid
            };
            let via_twin = fetch_sid != sid;
            tracing::info!(
                "guide: epg fetch starting for {} (display sid={}, archive={}, fetch sid={}{})",
                name,
                sid,
                is_archive,
                fetch_sid,
                if via_twin { " [via archive twin]" } else { "" }
            );
            let cache = self.guide.epg_cache.clone();
            let pending = self.guide.epg_pending.clone();
            let failed = self.guide.epg_failed.clone();
            let portal = self.catalog.portal().clone();
            let log_name = name.clone();
            let _ = is_archive; // captured by log line above
            tokio::spawn(async move {
                // PRIMARY = fetch_day_epg (get_simple_data_table) for ALL
                // channels. We need PAST entries above the current
                // programme so each column can scroll the LIVE row to the
                // same vertical middle as every other column - without
                // past entries (get_short_epg only returns now + future)
                // the LIVE row clamps to the top of the viewport and the
                // horizontal alignment across columns breaks.
                let primary = portal.fetch_day_epg(fetch_sid).await;
                let final_epg = match primary {
                    Ok(epg) if !epg.entries().is_empty() => {
                        let first = epg.entries().first().map(|e| {
                            e.start.with_timezone(&chrono::Local).format("%H:%M").to_string()
                        });
                        let last = epg.entries().last().map(|e| {
                            e.end.with_timezone(&chrono::Local).format("%H:%M").to_string()
                        });
                        tracing::info!(
                            "guide: epg ok for {} (sid={}, {} entries, {}..{})",
                            log_name,
                            sid,
                            epg.entries().len(),
                            first.as_deref().unwrap_or("?"),
                            last.as_deref().unwrap_or("?")
                        );
                        Some(epg)
                    }
                    Ok(empty_epg) => {
                        // Day endpoint returned empty. Fall back to the
                        // short endpoint - some portals only expose data
                        // there. Result has no past entries so the LIVE
                        // row won't centre, but it's better than nothing.
                        tracing::info!(
                            "guide: day-EPG empty for {} (sid={}); retrying via fetch_epg",
                            log_name,
                            sid
                        );
                        match portal.fetch_epg(fetch_sid).await {
                            Ok(epg) if !epg.entries().is_empty() => {
                                tracing::info!(
                                    "guide: fallback short-EPG ok for {} (sid={}, {} entries - no past)",
                                    log_name,
                                    sid,
                                    epg.entries().len()
                                );
                                Some(epg)
                            }
                            Ok(_) => {
                                tracing::info!(
                                    "guide: portal has no EPG for {} (sid={}) - empty on both endpoints",
                                    log_name,
                                    sid
                                );
                                Some(empty_epg)
                            }
                            Err(e) => {
                                tracing::warn!(
                                    "guide: fallback fetch_epg failed for {} (sid={}): {}",
                                    log_name,
                                    sid,
                                    e
                                );
                                Some(empty_epg)
                            }
                        }
                    }
                    Err(e) => {
                        tracing::warn!(
                            "guide: day-EPG failed for {} (sid={}): {} - trying short endpoint",
                            log_name,
                            sid,
                            e
                        );
                        match portal.fetch_epg(fetch_sid).await {
                            Ok(epg) if !epg.entries().is_empty() => {
                                tracing::info!(
                                    "guide: rescue short-EPG ok for {} (sid={}, {} entries - no past)",
                                    log_name,
                                    sid,
                                    epg.entries().len()
                                );
                                Some(epg)
                            }
                            _ => {
                                tracing::warn!(
                                    "guide: both endpoints failed for {} (sid={})",
                                    log_name,
                                    sid
                                );
                                failed.lock().insert(sid);
                                None
                            }
                        }
                    }
                };
                if let Some(epg) = final_epg {
                    cache.lock().insert(sid, epg);
                }
                pending.lock().remove(&sid);
            });
        }
    }

    /// Row to scroll-to for the current `time_mode` given a channel's EPG.
    fn guide_target_row(epg: &Epg, mode: TimeMode, now: chrono::DateTime<chrono::Utc>) -> usize {
        let entries = epg.entries();
        if entries.is_empty() {
            return 0;
        }
        match mode {
            TimeMode::NowAndNext => entries
                .iter()
                .position(|e| e.end > now)
                .unwrap_or(entries.len() - 1),
            TimeMode::Primetime => {
                use chrono::TimeZone;
                let target = chrono::Local::now()
                    .date_naive()
                    .and_hms_opt(20, 0, 0)
                    .and_then(|n| chrono::Local.from_local_datetime(&n).single())
                    .map(|l| l.with_timezone(&chrono::Utc))
                    .unwrap_or(now);
                entries
                    .iter()
                    .position(|e| e.start >= target)
                    .unwrap_or(entries.len() - 1)
            }
        }
    }

    /// Update `row_per_channel` for every visible column whose EPG has
    /// landed in cache, setting it to the row that matches the current
    /// `time_mode`. This drives both the cursor position and the scroll
    /// target (paint_guide will center on this row for not-yet-centered
    /// channels). Re-runs every paint so channels whose EPG arrives late
    /// still get their cursor positioned correctly.
    fn guide_sync_target_rows(&mut self, channels: &[LiveChannel], num_cols: usize) {
        // While auto-centring, park every visible column's cursor on its
        // current programme every frame, so the now-line follows time and
        // newly-revealed columns line up. Once the user browses with up/down
        // we stop overriding their cursor.
        if !self.guide.auto_center {
            return;
        }
        let now = chrono::Utc::now();
        let cache = self.guide.epg_cache.lock();
        for col in 0..num_cols {
            let Some(ch) = self.guide_visible_channel_at(col, channels) else { continue; };
            if let Some(epg) = cache.get(&ch.stream_id) {
                let row = Self::guide_target_row(epg, self.guide.time_mode, now);
                self.guide.row_per_channel.insert(ch.stream_id, row);
            }
        }
    }

    fn guide_play(&mut self, channels: &[LiveChannel]) {
        let Some(channel) = self
            .guide_visible_channel_at(self.guide.selected_col, channels)
            .cloned()
        else { return; };
        let row = *self.guide.row_per_channel.get(&channel.stream_id).unwrap_or(&0);
        let maybe_epg = self.guide.epg_cache.lock().get(&channel.stream_id).cloned();
        let Some(epg) = maybe_epg else {
            self.set_toast(self.t(T::GuideEpgNotLoaded));
            return;
        };
        let Some(entry) = epg.entries().get(row).cloned() else { return; };
        let now = chrono::Utc::now();
        let status = programme_status(&entry, &channel, now);
        let (url, tag) = match status {
            ProgrammeStatus::Future => {
                self.set_toast(self.tr(
                    T::GuideNotAired,
                    &[&entry.title, &crate::epg::hhmm(entry.start)],
                ));
                return;
            }
            ProgrammeStatus::PastUnavailable => {
                self.set_toast(self.tr(T::GuideNoCatchup, &[&channel.name]));
                return;
            }
            ProgrammeStatus::Live => {
                let live_sid = if channel.tv_archive == 1 {
                    self.catalog
                        .live_id_by_name(&channel.name)
                        .unwrap_or(channel.stream_id)
                } else {
                    channel.stream_id
                };
                (self.catalog.portal().live_stream_url(live_sid), "LIVE")
            }
            ProgrammeStatus::Catchup => {
                let dur_min = ((entry.end - entry.start).num_minutes() as u32).max(1);
                (
                    self.catalog
                        .portal()
                        .catchup_url(channel.stream_id, entry.start, dur_min),
                    "CATCHUP",
                )
            }
        };
        tracing::info!(
            "guide: [{}] {} | {} ({} -> {})",
            tag,
            channel.name,
            entry.title,
            entry.start.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M"),
            entry.end.with_timezone(&chrono::Local).format("%H:%M")
        );
        self.load_url(url, plays_as_live(status));
        // Up/down continues from this channel, not from the one zapped to before.
        self.current_idx = None;
        self.current_name = Some(format!("{} - {}", channel.name, entry.title));
        self.current_stream_id = Some(channel.stream_id);
        self.current_epg = None;
        let shown_tag = if plays_as_live(status) {
            "LIVE"
        } else {
            self.t(T::TagReplay)
        };
        self.set_toast(format!(
            "[{}] {} | {} @ {}",
            shown_tag,
            channel.name,
            entry.title,
            crate::epg::hhmm(entry.start)
        ));
        self.guide.open = false;
    }

    fn handle_guide_keys(&mut self, ctx: &egui::Context) {
        let (
            esc, g_key, up, down, left, right, pgup, pgdn, home, end, enter, backspace,
            n_key, p_key, a_key, t_key, l_key, qmark,
        ) = ctx.input(|i| (
            i.key_pressed(Key::Escape),
            i.key_pressed(Key::G),
            i.key_pressed(Key::ArrowUp),
            i.key_pressed(Key::ArrowDown),
            i.key_pressed(Key::ArrowLeft),
            i.key_pressed(Key::ArrowRight),
            i.key_pressed(Key::PageUp),
            i.key_pressed(Key::PageDown),
            i.key_pressed(Key::Home),
            i.key_pressed(Key::End),
            i.key_pressed(Key::Enter),
            i.key_pressed(Key::Backspace),
            i.key_pressed(Key::N),
            i.key_pressed(Key::P),
            i.key_pressed(Key::A),
            i.key_pressed(Key::T),
            i.key_pressed(Key::L),
            i.events
                .iter()
                .any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
        ));
        // Help overlay takes priority over guide input.
        if self.show_help {
            if qmark || esc {
                self.show_help = false;
            }
            return;
        }
        if qmark {
            self.show_help = true;
            return;
        }
        if esc || g_key {
            self.guide.open = false;
            return;
        }
        // Typed chars feed the filter, EXCEPT mode hotkeys (n/p time mode,
        // a/l/t channel mode) which we reserve. The filter still accepts
        // any other letter/digit normally.
        let typed: String = ctx.input(|i| {
            i.events
                .iter()
                .filter_map(|e| match e {
                    egui::Event::Text(t)
                        if t != "n"
                            && t != "p"
                            && t != "a"
                            && t != "l"
                            && t != "t"
                            && t != "?" =>
                    {
                        Some(t.clone())
                    }
                    _ => None,
                })
                .collect()
        });
        if !typed.is_empty() {
            self.guide.filter.push_str(&typed);
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
        }
        if backspace && !self.guide.filter.is_empty() {
            self.guide.filter.pop();
            self.guide.column_offset = 0;
            self.guide.selected_col = 0;
        }
        if n_key && self.guide.time_mode != TimeMode::NowAndNext {
            self.guide.time_mode = TimeMode::NowAndNext;
            // New time-mode: re-center every column on the new target.
            self.guide.auto_center = true;
        }
        if p_key && self.guide.time_mode != TimeMode::Primetime {
            self.guide.time_mode = TimeMode::Primetime;
            self.guide.auto_center = true;
        }
        if a_key && self.guide.channel_mode != ChannelMode::All {
            self.guide.channel_mode = ChannelMode::All;
            self.guide.column_offset = 0;
        }
        if l_key && self.guide.channel_mode != ChannelMode::Live {
            self.guide.channel_mode = ChannelMode::Live;
            self.guide.column_offset = 0;
        }
        if t_key && self.guide.channel_mode != ChannelMode::Catchup {
            self.guide.channel_mode = ChannelMode::Catchup;
            self.guide.column_offset = 0;
        }

        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);
        let num_cols = self.guide_num_visible_cols(ctx);
        let n_visible = self.guide.visible.len();

        // Up/down browse programmes - release auto-centring so the user's
        // cursor is respected. Left/right pan channels - keep centring so the
        // now-line stays put as columns scroll in.
        if up || down || pgup || pgdn || home || end {
            self.guide.auto_center = false;
        }
        if left || right {
            self.guide.auto_center = true;
        }

        // Programme cursor (up/down/pgup/pgdn/home/end) acts on the
        // selected column.
        if (up || down || pgup || pgdn || home || end) && n_visible > 0 {
            if let Some(ch) = self.guide_visible_channel_at(self.guide.selected_col, &channels) {
                let sid = ch.stream_id;
                let total = self
                    .guide
                    .epg_cache
                    .lock()
                    .get(&sid)
                    .map(|e| e.entries().len())
                    .unwrap_or(0);
                if total > 0 {
                    let cur = *self.guide.row_per_channel.get(&sid).unwrap_or(&0);
                    let new = if down { (cur + 1).min(total - 1) }
                        else if up { cur.saturating_sub(1) }
                        else if pgdn { (cur + 10).min(total - 1) }
                        else if pgup { cur.saturating_sub(10) }
                        else if home { 0 }
                        else { total - 1 };
                    self.guide.row_per_channel.insert(sid, new);
                }
            }
        }

        // Column selection (left/right) - shifts selected_col within
        // the visible window; when bumping into the edge, scrolls the
        // window via column_offset.
        if left {
            if self.guide.selected_col > 0 {
                self.guide.selected_col -= 1;
            } else if self.guide.column_offset > 0 {
                self.guide.column_offset -= 1;
            }
        }
        if right {
            let last_visible_abs = self.guide.column_offset + self.guide.selected_col + 1;
            if self.guide.selected_col + 1 < num_cols && last_visible_abs < n_visible {
                self.guide.selected_col += 1;
            } else if self.guide.column_offset + num_cols < n_visible {
                self.guide.column_offset += 1;
            }
        }

        if enter {
            self.guide_play(&channels);
        }
    }

    fn paint_guide(&mut self, ctx: &egui::Context) {
        if !self.guide.open { return; }
        let channels = self.catalog.live_channels();
        self.refresh_visible_if_stale(&channels);
        let num_cols = self.guide_num_visible_cols(ctx);
        self.guide_kick_visible_fetches(&channels, num_cols);
        // Park each visible column's cursor on the row that matches the
        // current time-mode (cheap; no-op for already-centered channels).
        self.guide_sync_target_rows(&channels, num_cols);

        let n_visible = self.guide.visible.len();
        let max_offset = n_visible.saturating_sub(num_cols);
        if self.guide.column_offset > max_offset {
            self.guide.column_offset = max_offset;
        }
        if self.guide.selected_col >= num_cols {
            self.guide.selected_col = num_cols.saturating_sub(1);
        }

        let now = chrono::Utc::now();
        let lang = self.lang;
        let time_mode = self.guide.time_mode;
        let chan_mode = self.guide.channel_mode;
        let filter_visible = self.guide.filter.clone();
        let selected_col = self.guide.selected_col;
        let column_offset = self.guide.column_offset;
        let now_playing = self.current_name.clone();

        // Snapshot the visible column slice into owned data BEFORE we
        // hand a &mut Ui into the draw closure. Keeps `self` free so the
        // click-handler post-draw can mutate row_per_channel + dispatch
        // play actions without fighting the borrow checker.
        struct ColData {
            channel: LiveChannel,
            programmes: Vec<EpgEntry>,
            row: usize,
            loading: bool,
            /// True when the last fetch attempt errored (HTTP / parse).
            /// Distinct from `loading=false && programmes empty` which
            /// means the portal genuinely returned no EPG data.
            failed: bool,
        }

        let col_data: Vec<ColData> = {
            let cache = self.guide.epg_cache.lock();
            let pending = self.guide.epg_pending.lock();
            let failed = self.guide.epg_failed.lock();
            (0..num_cols)
                .filter_map(|col| {
                    let ch = self.guide_visible_channel_at(col, &channels)?.clone();
                    let sid = ch.stream_id;
                    let programmes = cache
                        .get(&sid)
                        .map(|e| e.entries().to_vec())
                        .unwrap_or_default();
                    let row = *self.guide.row_per_channel.get(&sid).unwrap_or(&0);
                    let loading = pending.contains(&sid);
                    let did_fail = failed.contains(&sid);
                    Some(ColData {
                        channel: ch,
                        programmes,
                        row,
                        loading,
                        failed: did_fail,
                    })
                })
                .collect()
        };
        // Click intents (col, row) collected during draw, applied after.
        let mut clicked: Option<(usize, usize)> = None;

        // Full-window dark background. CentralPanel replaces the video
        // surface entirely while the guide is open - no bleed-through.
        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(Color32::from_rgb(14, 14, 18)))
            .show(ctx, |ui| {
                // ---------- Top bar ----------
                egui::Frame::none()
                    .inner_margin(egui::Margin::symmetric(16.0, 10.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(
                                egui::RichText::new(crate::i18n::t(lang, T::GuideTitle))
                                    .heading()
                                    .color(Color32::WHITE),
                            );
                            ui.add_space(20.0);

                            // Time-mode pills
                            let mode_pill = |ui: &mut egui::Ui, label: &str, active: bool, accent: Color32| {
                                let bg = if active { accent } else { Color32::from_rgb(32, 32, 38) };
                                let fg = if active { Color32::WHITE } else { Color32::from_white_alpha(180) };
                                egui::Frame::none()
                                    .fill(bg)
                                    .rounding(14.0)
                                    .inner_margin(egui::Margin::symmetric(12.0, 5.0))
                                    .show(ui, |ui| {
                                        ui.label(egui::RichText::new(label).color(fg).size(12.0));
                                    });
                            };
                            mode_pill(ui, crate::i18n::t(lang, T::GuideNowNext), time_mode == TimeMode::NowAndNext, Color32::from_rgb(60, 110, 170));
                            ui.add_space(6.0);
                            mode_pill(ui, crate::i18n::t(lang, T::GuidePrimetime), time_mode == TimeMode::Primetime, Color32::from_rgb(60, 110, 170));

                            ui.add_space(18.0);
                            ui.separator();
                            ui.add_space(6.0);

                            // Channel-mode pills
                            mode_pill(ui, crate::i18n::t(lang, T::GuideAll), chan_mode == ChannelMode::All, Color32::from_rgb(80, 100, 120));
                            ui.add_space(4.0);
                            mode_pill(ui, crate::i18n::t(lang, T::GuideLive), chan_mode == ChannelMode::Live, Color32::from_rgb(70, 130, 70));
                            ui.add_space(4.0);
                            mode_pill(ui, crate::i18n::t(lang, T::GuideCatchup), chan_mode == ChannelMode::Catchup, Color32::from_rgb(110, 140, 200));

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if n_visible > 0 {
                                    ui.label(
                                        egui::RichText::new(crate::i18n::tr(
                                            lang,
                                            T::GuideChannelsRange,
                                            &[
                                                &(column_offset + 1).to_string(),
                                                &(column_offset + col_data.len())
                                                    .min(n_visible)
                                                    .to_string(),
                                                &n_visible.to_string(),
                                            ],
                                        ))
                                        .color(Color32::from_white_alpha(150))
                                        .size(12.0),
                                    );
                                }
                                if let Some(np) = &now_playing {
                                    ui.add_space(20.0);
                                    ui.label(
                                        egui::RichText::new(crate::i18n::tr(
                                            lang,
                                            T::GuidePlaying,
                                            &[np],
                                        ))
                                        .color(Color32::from_rgb(120, 220, 100))
                                        .size(12.0)
                                        .strong(),
                                    );
                                }
                            });
                        });

                        // Filter line (only when active)
                        if !filter_visible.is_empty() {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(crate::i18n::tr(
                                    lang,
                                    T::GuideFilter,
                                    &[&filter_visible],
                                ))
                                .color(Color32::from_rgb(230, 220, 110))
                                .monospace()
                                .size(13.0),
                            );
                        }
                    });

                // Thin separator line between top bar and grid
                let sep_rect = ui.available_rect_before_wrap();
                ui.painter().hline(
                    sep_rect.x_range(),
                    sep_rect.top(),
                    egui::Stroke::new(1.0, Color32::from_rgb(35, 35, 42)),
                );
                ui.add_space(2.0);

                // ---------- Empty-state ----------
                if col_data.is_empty() {
                    ui.add_space(40.0);
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new(match chan_mode {
                                ChannelMode::Catchup => crate::i18n::t(lang, T::GuideNoCatchupMatch),
                                ChannelMode::Live => crate::i18n::t(lang, T::GuideNoLiveMatch),
                                ChannelMode::All => crate::i18n::t(lang, T::GuideNoMatch),
                            })
                            .color(Color32::from_white_alpha(140))
                            .italics()
                            .size(16.0),
                        );
                    });
                    return;
                }

                // ---------- Column grid ----------
                // Leave a bit of room at the bottom for the keybinding hint.
                let grid_bottom_reserve = 26.0;
                let grid_rect = ui.available_rect_before_wrap();
                let grid_height = (grid_rect.height() - grid_bottom_reserve).max(100.0);
                let _ = ui.allocate_ui_with_layout(
                    egui::vec2(grid_rect.width(), grid_height),
                    egui::Layout::top_down(egui::Align::LEFT),
                    |ui| {
                        ui.columns(col_data.len(), |cols| {
                            for (vi, cd) in col_data.iter().enumerate() {
                                let column_ui = &mut cols[vi];
                                let is_selected_col = vi == selected_col;
                                let is_archive = cd.channel.tv_archive == 1;
                                let accent = channel_accent_color(&cd.channel.name);
                                let badge_label = channel_badge_label(&cd.channel.name);

                                // ---- Column header (colored top band + badge + name) ----
                                // 3px coloured accent strip
                                let strip_rect = column_ui.available_rect_before_wrap();
                                let (strip, _) = column_ui.allocate_exact_size(
                                    egui::vec2(strip_rect.width(), 3.0),
                                    egui::Sense::hover(),
                                );
                                column_ui.painter().rect_filled(strip, 0.0, accent);

                                let header_bg = if is_selected_col {
                                    Color32::from_rgb(40, 55, 80)
                                } else {
                                    Color32::from_rgb(24, 24, 30)
                                };
                                egui::Frame::none()
                                    .fill(header_bg)
                                    .inner_margin(egui::Margin::symmetric(6.0, 5.0))
                                    .show(column_ui, |ui| {
                                        ui.horizontal(|ui| {
                                            // Round-ish badge with channel number / initial
                                            egui::Frame::none()
                                                .fill(accent)
                                                .rounding(10.0)
                                                .inner_margin(egui::Margin::symmetric(6.0, 2.0))
                                                .show(ui, |ui| {
                                                    ui.label(
                                                        egui::RichText::new(&badge_label)
                                                            .color(Color32::WHITE)
                                                            .strong()
                                                            .size(11.0),
                                                    );
                                                });
                                            ui.add_space(4.0);
                                            let suffix = if is_archive {
                                                format!("  {}", crate::i18n::t(lang, T::GuideArchiveBadge))
                                            } else {
                                                String::new()
                                            };
                                            ui.add(
                                                egui::Label::new(
                                                    egui::RichText::new(format!(
                                                        "{}{}",
                                                        cd.channel.name, suffix
                                                    ))
                                                    .color(Color32::WHITE)
                                                    .strong()
                                                    .size(12.0),
                                                )
                                                .truncate(true),
                                            );
                                        });
                                    });

                                if cd.programmes.is_empty() {
                                    column_ui.add_space(8.0);
                                    let (msg, col_) = if cd.loading {
                                        (crate::i18n::t(lang, T::GuideEpgLoading), Color32::from_white_alpha(120))
                                    } else if cd.failed {
                                        (
                                            crate::i18n::t(lang, T::GuideEpgFailed),
                                            Color32::from_rgb(220, 130, 110),
                                        )
                                    } else {
                                        (
                                            crate::i18n::t(lang, T::GuideNoEpg),
                                            Color32::from_white_alpha(95),
                                        )
                                    };
                                    column_ui.label(
                                        egui::RichText::new(msg)
                                            .color(col_)
                                            .italics()
                                            .size(11.0),
                                    );
                                    continue;
                                }

                                let cursor = cd.row;
                                let mut clicked_in_col: Option<usize> = None;

                                // egui's ScrollArea offset is unreliable on a
                                // virtualised list, so we DON'T scroll. Instead
                                // we render only the window of programmes that
                                // fills the viewport and use a top spacer to pin
                                // the focus row (current programme, or the cursor
                                // while browsing) to the vertical centre. Every
                                // column does the same -> all the green LIVE rows
                                // line up on one centre line, and the content
                                // never exceeds the viewport so nothing scrolls.
                                // Fixed window: 3 past + current + 4 upcoming,
                                // sized so those 8 rows fill the column height.
                                // A top spacer keeps the current programme at the
                                // same height in every column (the now-line).
                                const ABOVE: usize = 3;
                                const BELOW: usize = 4;
                                let view_h = column_ui.available_height().max(80.0);
                                let row_h = view_h / (ABOVE + BELOW + 1) as f32;
                                let title_sz = (row_h * 0.26).clamp(15.0, 22.0);
                                let time_sz = (row_h * 0.22).clamp(13.0, 18.0);
                                let n = cd.programmes.len();
                                let focus = cd.row.min(n.saturating_sub(1));
                                let start = focus.saturating_sub(ABOVE);
                                let above = focus - start;
                                let pad_top = (ABOVE - above) as f32 * row_h;
                                let end = (focus + BELOW + 1).min(n);
                                egui::ScrollArea::vertical()
                                    .id_source(format!("guide_col_{}", cd.channel.stream_id))
                                    .auto_shrink([false, false])
                                    .show(column_ui, |ui| {
                                        ui.add_space(pad_top);
                                        for p in start..end {
                                            let e = &cd.programmes[p];
                                            let status = programme_status(e, &cd.channel, now);
                                            let local_start = e.start.with_timezone(&chrono::Local);
                                            let time_label = local_start.format("%H:%M").to_string();

                                            let (bg, fg, badge) = match status {
                                                ProgrammeStatus::Live => (
                                                    Color32::from_rgb(28, 58, 30),
                                                    Color32::WHITE,
                                                    Some(("LIVE", Color32::from_rgb(120, 230, 100))),
                                                ),
                                                ProgrammeStatus::Catchup => (
                                                    Color32::from_rgb(18, 18, 22),
                                                    Color32::from_white_alpha(210),
                                                    Some((crate::i18n::t(lang, T::GuideArchiveBadge), Color32::from_rgb(140, 180, 220))),
                                                ),
                                                ProgrammeStatus::Future => (
                                                    Color32::from_rgb(18, 18, 22),
                                                    Color32::WHITE,
                                                    None,
                                                ),
                                                ProgrammeStatus::PastUnavailable => (
                                                    Color32::from_rgb(18, 18, 22),
                                                    Color32::from_white_alpha(95),
                                                    None,
                                                ),
                                            };
                                            let is_selected = p == cursor && is_selected_col;
                                            let row_bg = if is_selected {
                                                // Subtle highlight overlay - blend selection with status colour
                                                match status {
                                                    ProgrammeStatus::Live => Color32::from_rgb(36, 75, 50),
                                                    _ => Color32::from_rgb(40, 55, 80),
                                                }
                                            } else {
                                                bg
                                            };
                                            // Selected row gets a 2px accent-color left border drawn
                                            // by painting a small strip BEFORE the row frame.
                                            let frame_response = egui::Frame::none()
                                                .fill(row_bg)
                                                .inner_margin(egui::Margin {
                                                    left: if is_selected { 6.0 } else { 4.0 },
                                                    right: 4.0,
                                                    top: 1.0,
                                                    bottom: 1.0,
                                                })
                                                .show(ui, |ui| {
                                                    let row_rect = ui.available_rect_before_wrap();
                                                    if is_selected {
                                                        let mut bar = row_rect;
                                                        bar.set_left(row_rect.left() - 4.0);
                                                        bar.set_right(row_rect.left() - 2.0);
                                                        ui.painter().rect_filled(bar, 0.0, accent);
                                                    }
                                                    ui.horizontal(|ui| {
                                                        // Time column - fixed
                                                        // narrow width so titles
                                                        // get the rest of the
                                                        // row to themselves.
                                                        ui.add_sized(
                                                            egui::vec2(52.0, row_h - 6.0),
                                                            egui::Label::new(
                                                                egui::RichText::new(&time_label)
                                                                    .color(Color32::from_white_alpha(165))
                                                                    .monospace()
                                                                    .size(time_sz),
                                                            ),
                                                        );
                                                        // Reserve space for the
                                                        // right-side badge so the
                                                        // title truncates cleanly
                                                        // without overrunning it.
                                                        let badge_w = if badge.is_some() { 52.0 } else { 0.0 };
                                                        let title_w = (ui.available_width()
                                                            - badge_w - 4.0)
                                                            .max(40.0);
                                                        ui.allocate_ui_with_layout(
                                                            egui::vec2(title_w, row_h - 6.0),
                                                            egui::Layout::left_to_right(
                                                                egui::Align::Center,
                                                            ),
                                                            |ui| {
                                                                // Clip so a long
                                                                // wrapped title
                                                                // never bleeds
                                                                // into the next
                                                                // row.
                                                                ui.set_clip_rect(ui.max_rect());
                                                                ui.add(
                                                                    egui::Label::new(
                                                                        egui::RichText::new(&e.title)
                                                                            .color(fg)
                                                                            .size(title_sz)
                                                                            .strong(),
                                                                    )
                                                                    .wrap(true),
                                                                );
                                                            },
                                                        );
                                                        if let Some((b_txt, b_col)) = badge {
                                                            ui.with_layout(
                                                                egui::Layout::right_to_left(egui::Align::Center),
                                                                |ui| {
                                                                    egui::Frame::none()
                                                                        .fill(b_col)
                                                                        .rounding(8.0)
                                                                        .inner_margin(egui::Margin::symmetric(6.0, 1.5))
                                                                        .show(ui, |ui| {
                                                                            ui.label(
                                                                                egui::RichText::new(b_txt)
                                                                                    .color(Color32::from_rgb(20, 20, 20))
                                                                                    .size(9.5)
                                                                                    .strong(),
                                                                            );
                                                                        });
                                                                },
                                                            );
                                                        }
                                                    });
                                                    // 2px progress bar at bottom of LIVE row only.
                                                    if matches!(status, ProgrammeStatus::Live) {
                                                        let total = (e.end - e.start).num_seconds().max(1) as f32;
                                                        let elapsed = (now - e.start).num_seconds().max(0) as f32;
                                                        let progress = (elapsed / total).clamp(0.0, 1.0);
                                                        let bar_rect = ui.available_rect_before_wrap();
                                                        let (full, _) = ui.allocate_exact_size(
                                                            egui::vec2(bar_rect.width(), 2.0),
                                                            egui::Sense::hover(),
                                                        );
                                                        ui.painter().rect_filled(full, 1.0, Color32::from_white_alpha(35));
                                                        let mut done = full;
                                                        done.set_width(full.width() * progress);
                                                        ui.painter().rect_filled(
                                                            done,
                                                            1.0,
                                                            Color32::from_rgb(120, 230, 100),
                                                        );
                                                    }
                                                })
                                                .response;
                                            // Thin divider under each row (NLZIET-style).
                                            ui.painter().hline(
                                                frame_response.rect.x_range(),
                                                frame_response.rect.bottom(),
                                                egui::Stroke::new(
                                                    1.0,
                                                    Color32::from_white_alpha(18),
                                                ),
                                            );
                                            if frame_response.interact(egui::Sense::click()).clicked() {
                                                clicked_in_col = Some(p);
                                            }
                                        }
                                    });
                                if let Some(row) = clicked_in_col {
                                    clicked = Some((vi, row));
                                }
                            }
                        });
                    },
                );

                // ---------- Footer hint ----------
                ui.add_space(4.0);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        egui::RichText::new(crate::i18n::t(lang, T::GuideFooter))
                            .color(Color32::from_white_alpha(95))
                            .size(11.0),
                    );
                });
            });

        // Apply clicks now that the borrow on `self` from .show() is gone.
        if let Some((col, row)) = clicked {
            if let Some(ch) = self.guide_visible_channel_at(col, &channels).cloned() {
                self.guide.selected_col = col;
                self.guide.row_per_channel.insert(ch.stream_id, row);
                self.guide_play(&channels);
            }
        }
    }
}

impl Drop for TvApp {
    fn drop(&mut self) {
        let _ = self.favorites.save(&self.storage.favorites_path());
        let _ = self.player.cmd_tx.send(Cmd::Quit);
    }
}

impl eframe::App for TvApp {
    // Don't persist egui memory across runs - otherwise a stale focus (e.g. the
    // search box that was open at exit) gets restored on startup. Search must
    // only appear on `f`.
    fn persist_egui_memory(&self) -> bool {
        false
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.drain_events();
        self.drain_epg();
        self.drain_news_picker();
        self.drain_conn_check();
        // First start: nothing else until a language is chosen.
        if let Some(selected) = self.choosing_language.as_mut() {
            if let Some(l) = crate::settings_ui::language_screen(ctx, selected) {
                self.choose_language(l);
            }
            ctx.request_repaint_after(Duration::from_millis(33));
            return;
        }
        // A resume waiting for the catalog (a 0.5 last_watched.json).
        if self.choosing_language.is_none()
            && self.pending_resume.is_some()
            && self.catalog.is_loaded()
        {
            if let Some(lw) = self.pending_resume.take() {
                self.resume(lw);
            }
        }
        // The channel the user stopped flipping on.
        if let Some((sid, name, idx)) = self.zap_pacer.due(Instant::now()) {
            self.zap_to(sid, &name, Some(idx));
        }
        self.check_stall();
        self.forward_window_size(ctx);
        self.update_video_texture(ctx);
        // The first-run setup prompt shows while there are no usable credentials.
        let setup_prompt = matches!(
            self.portal_state,
            PortalState::Missing | PortalState::Placeholder
        );
        // Guide owns input while it's open - we don't want `1`/`2`/`3` etc.
        // firing zaps when the user is typing into the channel filter.
        // The settings overlay owns the keyboard while it is open (it reads
        // its keys in paint_settings), and so does the setup prompt: typing a
        // user name must not fire hotkeys.
        if self.settings_ui.is_none() && !setup_prompt {
            if self.guide.open {
                self.handle_guide_keys(ctx);
            } else {
                self.handle_keys(ctx);
            }
        }

        // Guide takes over the full window when open. Skipping the video
        // texture upload in this branch removes ~5 MB of per-frame memcpy
        // work while the user is browsing the schedule.
        if self.guide.open {
            self.paint_guide(ctx);
        } else if setup_prompt {
            // First-run: no credentials yet - show the portal setup prompt
            // over a dark background instead of the video surface.
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::from_rgb(14, 14, 18)))
                .show(ctx, |_ui| {});
            self.paint_portal_prompt(ctx);
        } else {
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::BLACK))
                .show(ctx, |ui| {
                    if let Some(tex) = &self.video_tex {
                        let avail = ui.available_size();
                        ui.add(egui::Image::new(tex).fit_to_exact_size(avail));
                    } else {
                        self.paint_empty_state(ui);
                    }
                });
            // Overlays only make sense in video mode - inside the guide they
            // would compete visually with the schedule grid.
            self.paint_search(ctx);
            self.paint_news_picker(ctx);
            self.paint_favs(ctx);
            self.paint_epg_strip(ctx);
            self.paint_epg_grid(ctx);
            self.paint_debug_hud(ctx);
        }
        // Toast remains visible above everything (covers play-action feedback
        // when the user picks a programme from the guide).
        self.paint_toast(ctx);
        // Help overlay paints last so it sits on top of everything,
        // including the guide. Whether the guide is open is irrelevant
        // for `?` - the help is a global overlay.
        if self.paint_help(ctx) {
            self.show_help = false;
            self.open_settings();
        }
        self.paint_settings(ctx);

        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_live_guide_pick_plays_through_the_relay() {
        assert!(plays_as_live(ProgrammeStatus::Live));
        assert!(!plays_as_live(ProgrammeStatus::Catchup));
    }

    #[test]
    fn an_old_last_watched_file_has_an_unknown_adult_flag() {
        let old: LastWatched =
            serde_json::from_str(r#"{"url":"http://h/live/u/p/1.m3u8","live":true}"#).unwrap();
        assert_eq!(old.adult, None);
        let new: LastWatched =
            serde_json::from_str(r#"{"url":"http://h/live/u/p/1.m3u8","live":true,"adult":false}"#)
                .unwrap();
        assert_eq!(new.adult, Some(false));
    }

    /// A portal that is never asked anything: the catalog only needs one to exist.
    fn idle_portal() -> Arc<dyn Portal> {
        Arc::new(XtreamPortal::new(XtreamCreds {
            username: "u".into(),
            password: "p".into(),
            host: "h".into(),
            port: 80,
        }))
    }

    /// 3 is a normal channel, 901 is adult by the portal's flag, 902 by its name.
    fn adult_catalog() -> crate::portal::Catalog {
        let live = |stream_id: i64, name: &str, is_adult: bool| LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult,
        };
        crate::portal::Catalog {
            live: vec![
                live(3, "TEST 03", false),
                live(901, "24/7 | ADULT A", true),
                live(902, "XXX | ADULT B", false),
            ],
            ..Default::default()
        }
    }

    #[test]
    fn an_unloaded_catalog_says_nothing_about_a_stream_not_even_that_it_is_not_adult() {
        let url = "http://h/live/u/p/901.m3u8";
        assert_eq!(
            catalog_says_adult(&CatalogStore::new(idle_portal()), url),
            None
        );
        let loaded = CatalogStore::from_catalog(idle_portal(), adult_catalog());
        assert_eq!(catalog_says_adult(&loaded, url), Some(true));
        assert_eq!(
            catalog_says_adult(&loaded, "http://h/live/u/p/3.m3u8"),
            Some(false)
        );
    }

    #[test]
    fn last_watched_never_records_not_adult_for_lack_of_knowledge() {
        let loading = CatalogStore::new(idle_portal());
        let loaded = CatalogStore::from_catalog(idle_portal(), adult_catalog());
        let (adult_url, normal_url) = ("http://h/live/u/p/901.m3u8", "http://h/live/u/p/3.m3u8");
        // The flag the file gets: `None` is unknown; the outer `None` leaves the file alone.
        let flag = |catalog: &CatalogStore, url: &str, hint: Option<bool>, reload: bool| {
            last_watched_record(catalog, url, true, hint, reload).map(|r| r.adult)
        };
        // A URL from the command line while the catalog loads: unknown, never "no".
        assert_eq!(flag(&loading, adult_url, None, false), Some(None));
        // The resume hands the file's flag on, so a normal start records what it read.
        assert_eq!(
            flag(&loading, normal_url, Some(false), false),
            Some(Some(false))
        );
        // The loaded catalog's word beats the hint, either way.
        assert_eq!(
            flag(&loaded, adult_url, Some(false), false),
            Some(Some(true))
        );
        assert_eq!(flag(&loaded, normal_url, None, false), Some(Some(false)));
        // A reload of the stream that plays: while the catalog loads nothing new is
        // known, so the file keeps what it has; once the catalog can tell, it is written.
        assert_eq!(flag(&loading, normal_url, None, true), None);
        assert_eq!(flag(&loaded, normal_url, None, true), Some(Some(false)));
        // What else the record holds is the load's own.
        let record = last_watched_record(&loading, normal_url, false, Some(false), false);
        let record = record.expect("a first load is recorded");
        assert_eq!((record.url.as_str(), record.live), (normal_url, false));
    }

    #[test]
    fn locked_favorites_and_presets_pointing_at_adult_content_act_empty() {
        let catalog = CatalogStore::from_catalog(idle_portal(), adult_catalog());
        let mut favorites = Favorites::default();
        favorites.toggle(3, "TEST 03");
        favorites.toggle(901, "24/7 | ADULT A"); // adult by the portal's flag
        favorites.toggle(902, "Renamed"); // adult by its name in the catalog
        favorites.toggle(77, "XXX | Gone"); // not in the catalog: the name says it
        let mut presets = Presets::default();
        presets.set(1, 901, "24/7 | ADULT A");
        presets.set(2, 902, "Renamed");
        presets.set(3, 3, "TEST 03");
        presets.set(4, 77, "XXX | Gone");
        let listed = |catalog: &CatalogStore| {
            let shown = shown_favorites(&favorites, catalog);
            shown.iter().map(|e| e.stream_id).collect::<Vec<_>>()
        };
        let recalled = |catalog: &CatalogStore| {
            (0..=5)
                .map(|digit| preset_target(&presets, digit, catalog))
                .collect::<Vec<_>>()
        };
        // Locked: only the normal channel is listed or recalled; empty slots stay empty.
        assert_eq!(listed(&catalog), [3]);
        assert_eq!(recalled(&catalog), [None, None, None, Some(3), None, None]);
        // Unlocked: all of them.
        catalog.set_adult_unlocked(true);
        assert_eq!(listed(&catalog), [3, 901, 902, 77]);
        assert_eq!(
            recalled(&catalog),
            [None, Some(901), Some(902), Some(3), Some(77), None]
        );
    }

    #[test]
    fn another_portal_leaves_the_guide_nothing_kept_by_the_old_ones_stream_ids() {
        let mut guide = GuideState::default();
        guide.epg_cache.lock().insert(7, Epg::new(Vec::new()));
        guide.epg_pending.lock().insert(8);
        guide.epg_failed.lock().insert(9);
        guide.row_per_channel.insert(7, 3);
        // A fetch of the old portal that is still in flight holds the old maps.
        let (old_cache, old_pending) = (guide.epg_cache.clone(), guide.epg_pending.clone());
        guide.forget_portal();
        assert!(guide.epg_cache.lock().is_empty());
        assert!(guide.epg_pending.lock().is_empty());
        assert!(guide.epg_failed.lock().is_empty());
        assert!(guide.row_per_channel.is_empty());
        // It finishes into the old maps, which the guide no longer reads.
        old_cache.lock().insert(8, Epg::new(Vec::new()));
        old_pending.lock().remove(&8);
        assert!(guide.epg_cache.lock().is_empty());
        assert!(guide.epg_pending.lock().is_empty());
    }

    #[test]
    fn adult_content_never_resumes() {
        use ResumeDecision::*;
        // (the file's flag, the loaded catalog's answer) -> what happens
        let cases = [
            // A file from 0.5 and a catalog still loading: nobody knows.
            (None, None, WaitForCatalog),
            (None, Some(false), Play),
            (None, Some(true), Skip),
            (Some(false), None, Play),
            (Some(false), Some(false), Play),
            // Either one saying adult is enough: the file can be older than the
            // portal's word (a channel flagged since).
            (Some(false), Some(true), Skip),
            (Some(true), None, Skip),
            (Some(true), Some(false), Skip),
            (Some(true), Some(true), Skip),
        ];
        for (flag, catalog, want) in cases {
            assert_eq!(
                resume_decision(flag, catalog),
                want,
                "file {flag:?}, catalog {catalog:?}"
            );
        }
    }

    #[test]
    fn the_help_zap_row_follows_the_zap_direction_and_nothing_else_does() {
        for lang in Lang::ALL {
            let normal = help_rows(lang, false);
            let swapped = help_rows(lang, true);
            let wheel = crate::i18n::t(lang, T::KeyUpDownWheel);
            let zap = |rows: &[(T, Vec<HelpRow>); 2]| {
                rows[0].1.iter().find(|r| r.key == wheel).map(|r| r.action)
            };
            assert_eq!(
                zap(&normal),
                Some(crate::i18n::t(lang, T::HelpZap)),
                "{lang:?}"
            );
            assert_eq!(
                zap(&swapped),
                Some(crate::i18n::t(lang, T::HelpZapSwapped)),
                "{lang:?}"
            );
            assert_ne!(zap(&normal), zap(&swapped), "{lang:?}");
            // Reversing flips the arrows and the wheel; no other row and never the guide's keys.
            let others = |rows: &[(T, Vec<HelpRow>); 2]| {
                rows[0]
                    .1
                    .iter()
                    .filter(|r| r.key != wheel)
                    .copied()
                    .collect::<Vec<_>>()
            };
            assert_eq!(others(&normal), others(&swapped), "{lang:?}");
            assert_eq!(normal[1], swapped[1], "{lang:?}");
        }
    }

    #[test]
    fn exactly_one_help_row_opens_the_settings_and_it_is_the_o_row() {
        for lang in Lang::ALL {
            for swap in [false, true] {
                let sections = help_rows(lang, swap);
                let links: Vec<&HelpRow> = sections
                    .iter()
                    .flat_map(|(_, rows)| rows.iter())
                    .filter(|row| row.opens_settings)
                    .collect();
                assert_eq!(links.len(), 1, "{lang:?}, swapped: {swap}");
                assert_eq!(links[0].key, "o");
                assert_eq!(links[0].action, crate::i18n::t(lang, T::HelpSettings));
            }
        }
    }
}
