use crate::args::{credentials_line, parse_xtream_creds, CredsError, XtreamCreds};
use crate::catalog::GuideChannel;
use crate::catalog::{fallback_variant, normalize_channel_name, quality_label, quality_rank, zap_position, CatalogStatus, CatalogStore};
use crate::epg::{Epg, EpgEntry};
use crate::favorites::{Entry as FavoriteEntry, Favorites};
use crate::guide::fetch::EpgFetcher;
use crate::guide::logos::LogoStore;
use crate::guide::store::{fetch_order, ChannelEpg, EpgStore};
use crate::guide::time::Playable;
use crate::guide_ui::{paint_wait_screen, GuideAction, GuideData, GuideView, WaitInfo};
use crate::i18n::{Lang, T};
use crate::news::NewsRule;
use crate::pin::{PinGate, PinHash};
use crate::playback_health::{PlaybackHealth, ReloadBudget};
use crate::relay::Relay;
use crate::player::{Cmd, Event, FrameFence, PlayerHandle, RgbaFrame};
use crate::presets::Presets;
use crate::portal::xtream::ConnCheck;
use crate::portal::{xtream::XtreamPortal, LiveChannel, Portal};
use crate::search::{self, ItemKind, SearchItem};
use crate::settings::Settings;
use crate::settings_ui::{
    conn_rich_text, marker_scroll_area, paint_scroll_marker, ConnState, SettingsAction, SettingsUi,
    SettingsView,
};
use crate::shortcuts;
use crate::storage::Storage;
use crate::zap::ZapPacer;
use egui::{Color32, ColorImage, Key, TextureHandle, TextureOptions};
use parking_lot::Mutex;
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
/// file as it is. Its adult flag is "adult" when the catalog or the caller says
/// so (`hint`: the resume passes the file's own flag on, a guide pick whether
/// its column is adult, which it is when any variant carries the flag, maybe
/// not this stream). Else it is the catalog's word when it can give one, else
/// the hint, else unknown, never "not adult" for lack of knowledge: the next
/// start would resume an adult channel (a URL from the command line) without
/// the PIN. A reload of the stream that plays, with nothing new known about
/// it, leaves the file alone: the flag it has must not turn into "unknown".
fn last_watched_record(
    catalog: &CatalogStore,
    url: &str,
    live: bool,
    hint: Option<bool>,
    reload: bool,
) -> Option<LastWatched> {
    let catalog_says = catalog_says_adult(catalog, url);
    let adult = if catalog_says == Some(true) || hint == Some(true) {
        Some(true)
    } else {
        catalog_says.or(hint)
    };
    if adult.is_none() && reload {
        return None;
    }
    Some(LastWatched {
        url: url.to_owned(),
        live,
        adult,
    })
}

/// What a resume that skips leaves in last_watched.json. When the loaded
/// catalog is the one that calls the stream adult, and the file did not say so
/// already, the file learns it: the next start then skips at once, without
/// waiting for the catalog. `None`: the file stays as it is.
fn skipped_record(lw: &LastWatched, catalog_says: Option<bool>) -> Option<LastWatched> {
    (catalog_says == Some(true) && lw.adult != Some(true)).then(|| LastWatched {
        adult: Some(true),
        ..lw.clone()
    })
}

/// Whether the catalog's first word on the stream that plays must stop it: the
/// stream came from the resume at start (not from the command line, not picked
/// by the user), the catalog calls it adult, and the app is locked. The file
/// can be older than the portal's flag, and the resume went by the file while
/// the catalog was still loading.
fn stop_resumed_adult(from_resume: bool, adult: bool, unlocked: bool) -> bool {
    from_resume && adult && !unlocked
}

/// The favorites that show: none until the catalog has loaded (until then
/// nothing says which of them are adult, and none could play anyway), then all
/// but the adult ones, which act as if they were not there while locked.
fn shown_favorites(favorites: &Favorites, catalog: &CatalogStore) -> Vec<FavoriteEntry> {
    if !catalog.is_loaded() {
        return Vec::new();
    }
    favorites
        .iter()
        .filter(|e| !catalog.is_hidden_live(e.stream_id))
        .cloned()
        .collect()
}

/// The channel a preset recalls: none when the slot is empty, or adult while
/// locked (it then acts as empty).
fn preset_target(presets: &Presets, digit: u8, catalog: &CatalogStore) -> Option<i64> {
    presets
        .get(digit)
        .filter(|p| !catalog.is_hidden_live(p.stream_id))
        .map(|p| p.stream_id)
}

/// Writes the guide's cache file and says so in the log. Whether it was written.
fn write_epg_cache(path: &std::path::Path, channels: &[crate::guide::cache::ToSave]) -> bool {
    match crate::guide::cache::save(path, channels, chrono::Utc::now()) {
        Ok((n, programmes, bytes)) => {
            tracing::info!(
                "guide: epg cache written ({} channels, {} programmes, {} KB)",
                n,
                programmes,
                bytes / 1024
            );
            true
        }
        Err(e) => {
            tracing::warn!("guide: epg cache not written: {}", e);
            false
        }
    }
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
                row(t(T::KeyLeftRight), t(T::HelpGuideColumns)),
                row(t(T::KeyUpDownWheel), t(T::HelpGuideRows)),
                row("PgUp / PgDn", t(T::HelpGuidePages)),
                row("Home / End", t(T::HelpGuideHomeEnd)),
                row("Enter", t(T::HelpGuidePlay)),
                row("n / p", t(T::HelpGuideTimeMode)),
                row("d", t(T::HelpGuideDate)),
                row("f", t(T::HelpGuideFilter)),
                row("Esc", t(T::HelpGuideEsc)),
                row("g", t(T::HelpGuideClose)),
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

/// A catch-up programme loading behind the wait screen.
struct PendingCatchup {
    channel: GuideChannel,
    title: String,
    start: chrono::DateTime<chrono::Utc>,
    end: chrono::DateTime<chrono::Utc>,
    /// Why it failed before its first frame, once it did.
    failed: Option<String>,
    /// Once mpv has restarted playback: the version of the newest frame on the
    /// bus then, and the time. mpv restarts before it renders anything, so the
    /// wait ends on the first frame newer than that version, or a second after
    /// the restart if none comes.
    started: Option<(u64, Instant)>,
}

/// How long a catch-up wait goes on after mpv's restart when no picture comes.
const WAIT_FOR_PICTURE: Duration = Duration::from_secs(1);

/// The cache file as read on its own thread: for which catalog generation, and
/// what it held.
type CacheRead = (u64, Result<Vec<(i64, ChannelEpg)>, String>);

/// How long the guide's fetches wait for the cache file to be read, should the
/// reading thread hang.
const EPG_CACHE_READ_WAIT: Duration = Duration::from_secs(2);

/// How long a cache write that failed waits before it is tried again.
const EPG_CACHE_RETRY: Duration = Duration::from_secs(60);

/// What the "found" lines of the news settings were worked out for: the rules'
/// channel names and the catalog's state. Typing in a rule's patterns, label or
/// seconds changes none of it.
struct NewsLookup {
    channels: Vec<String>,
    loaded: bool,
    unlocked: bool,
}

impl NewsLookup {
    fn new(rules: &[NewsRule], loaded: bool, unlocked: bool) -> Self {
        Self {
            channels: rules.iter().map(|r| r.channel.clone()).collect(),
            loaded,
            unlocked,
        }
    }

    /// Whether the lines must be worked out again for these rules and this
    /// catalog state. Compared where it lies: nothing is built per frame.
    fn is_stale(&self, rules: &[NewsRule], loaded: bool, unlocked: bool) -> bool {
        self.loaded != loaded
            || self.unlocked != unlocked
            || self.channels.len() != rules.len()
            || self
                .channels
                .iter()
                .zip(rules)
                .any(|(channel, rule)| *channel != rule.channel)
    }
}

/// How many day-EPG fetches the news picker keeps in flight. The portal
/// throttles bursts, so a long rule list must not fire them all at once; the 7
/// default rules are all concurrent.
const NEWS_FETCHES_IN_FLIGHT: usize = 8;

/// Runs `work` once the semaphore has a permit free, and holds the permit until
/// the work is done.
async fn within_limit<T>(
    permits: &tokio::sync::Semaphore,
    work: impl std::future::Future<Output = T>,
) -> T {
    // Only a closed semaphore refuses, and nothing closes this one: then the work
    // just runs.
    let _permit = permits.acquire().await;
    work.await
}

/// Modal "latest news" chooser opened with `n` (RTL + NOS combined).
struct NewsPicker {
    items: Vec<NewsItem>,
    selected: usize,
}

/// What a news list was made for: the catalog it was fetched through (counted
/// from the start, a new one with every portal save) and whether adult content
/// showed then.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct NewsStamp {
    generation: u64,
    unlocked: bool,
}

/// A finished news list, on its way from its background task to the UI.
struct NewsResult {
    items: Vec<NewsItem>,
    stamp: NewsStamp,
}

/// Whether a news list fetched under `fetched` may still be shown under `now`.
/// A list fetched while adult content showed must not pop up after a Lock, and
/// one fetched through the old portal's catalog holds stream ids that mean
/// other channels (or none) in the new one.
fn keep_news_result(fetched: NewsStamp, now: NewsStamp) -> bool {
    fetched == now
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
    /// They came from `--xtream`, which beats the ini on every start: the
    /// settings say that what is saved there only counts without it.
    creds_from_command_line: bool,

    video_tex: Option<TextureHandle>,
    last_frame_version: u64,

    current_idx: Option<usize>,
    current_name: Option<String>,
    current_stream_id: Option<i64>,

    current_epg: Option<Epg>,
    epg_slot: Arc<Mutex<Option<(i64, Epg)>>>,
    epg_fetch_pending_for: Option<i64>,
    /// Combined NOS/RTL news list built async for the `n` picker.
    news_picker_slot: Arc<Mutex<Option<NewsResult>>>,
    /// A news fetch is under way. Its result clears this; `n` does nothing until
    /// then, since a held key repeats and each press would start a batch of EPG
    /// fetches, the burst the portal throttles.
    news_fetching: bool,
    /// Which catalog this is, counted from the start; a portal save makes it a
    /// new one.
    catalog_generation: u64,
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

    /// The guide (`g`) is open.
    guide_open: bool,
    /// The guide's state: it opens as it was left.
    guide_view: GuideView,
    /// The guide's columns, the catalog's channels with catch-up, as the
    /// catalog and the lock stood at `guide_channels_for` (catalog generation,
    /// the catalog's revision, unlocked).
    guide_channels: Vec<GuideChannel>,
    guide_channels_for: Option<(u64, u64, bool)>,
    /// Counts every new `guide_channels`: what the guide works out from the
    /// list is worked out again when it moves.
    guide_channels_revision: u64,
    /// The guide's EPG, fetched in the background once the guide was opened.
    epg_store: EpgStore,
    epg_fetcher: EpgFetcher,
    /// The guide was opened this session: its EPG is fetched from then on.
    guide_fetching: bool,
    logos: LogoStore,
    /// The catalog generation whose EPG cache file was read (or is being read).
    epg_cache_read_for: Option<u64>,
    /// What the cache file held, on its way from the reading thread.
    epg_cache_slot: Arc<Mutex<Option<CacheRead>>>,
    /// Since when EPG has been coming in that the cache file does not hold yet
    /// (a write that failed counts from the moment it was noticed).
    epg_cache_dirty_since: Option<Instant>,
    /// When the guide's channels were last looked through to see whether the EPG
    /// that came in is complete: a walk over the whole list, so once a second
    /// at most.
    epg_cache_checked: Option<Instant>,
    /// The thread writing the cache file, from its start until a frame (or exit)
    /// takes it in once it has finished. A write goes through a temporary file
    /// of one fixed name, so two writers at once could corrupt the file: only
    /// one runs at a time.
    epg_cache_writer: Option<std::thread::JoinHandle<()>>,
    /// Since when this catalog's cache file is being read, until the app has
    /// taken in what it held. The guide's fetches wait for it (2 s at most).
    epg_cache_reading_since: Option<Instant>,
    /// Set by the writer when its write failed, for `note_failed_epg_cache_write`
    /// to find once the writer has finished. A portal save makes a new one, so
    /// the failure of a write for the old portal is no news to the new store.
    epg_cache_write_failed: Arc<std::sync::atomic::AtomicBool>,
    /// No write before this moment: one that failed waits a minute.
    epg_cache_retry_after: Option<Instant>,
    /// When the stream that loads now went to the player; its first frame
    /// clears it. EPG fetching waits meanwhile (10 s at most): zapping first.
    stream_starting_since: Option<Instant>,
    /// A catch-up pick loading behind the wait screen.
    pending_catchup: Option<PendingCatchup>,
    /// Tracks borderless+always-on-top toggle (`t` key).
    borderless: bool,
    /// `?` toggles a centered help overlay listing every hotkey.
    show_help: bool,

    // ---- silent-stall watchdog: recover frozen streams by reloading ----
    /// URL of the stream currently loaded, so the watchdog can reload it.
    current_url: Option<String>,
    /// Whether that stream is live TV (else a film, series or catch-up).
    current_live: bool,
    /// That stream was picked in a guide column that is adult: a variant of the
    /// channel carries the portal's flag, maybe not this one. Lock stops it, and
    /// it is never resumed. Any other load clears it, and so does a stop.
    current_adult_hint: bool,
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
    /// What plays came from the resume at start, and nothing was picked since
    /// (a reload of it counts as the same): the catalog's first word can still
    /// stop it, see `on_catalog_loaded`.
    playing_resumed: bool,
    /// The catalog's first word on the stream that played at start is dealt with.
    catalog_noted: bool,
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
    /// The channel each news rule points at ("gevonden: ..."), for the
    /// settings; worked out again only when `news_found_for` says it is stale
    /// and the News section shows, never per frame.
    news_found: Vec<Option<String>>,
    news_found_for: Option<NewsLookup>,
    /// Settings changed in memory and not written yet: the news rules change
    /// with every key typed, so they are saved when the overlay closes. Cleared
    /// by a write that worked only.
    settings_dirty: bool,
}

/// Spends a key press before anything else reads it in this frame: its `Key`
/// events, whatever the modifiers, and, for a letter, the character it types.
/// What opens a view on a key press must do this when the view reads its keys
/// in the same frame.
fn spend_key(ctx: &egui::Context, key: Key) {
    ctx.input_mut(|i| {
        i.events.retain(|e| match e {
            egui::Event::Key {
                key: event_key,
                pressed: true,
                ..
            } => *event_key != key,
            egui::Event::Text(typed) => !typed.eq_ignore_ascii_case(key.name()),
            _ => true,
        })
    });
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
        creds_from_command_line: bool,
        settings: Settings,
    ) -> Self {
        // Whenever a new mpv frame is ready, ask egui to redraw.
        let ctx = cc.egui_ctx.clone();
        player
            .frames
            .set_new_frame_callback(move || ctx.request_repaint());

        // Only kick off a fetch if we actually have a real portal — otherwise the
        // request would hang on a placeholder/missing host until the network timeout
        // and the user would see "loading..." for ~45s before any error appears.
        if portal_state == PortalState::Configured {
            catalog.spawn_fetch();
        }
        Self::start(
            player,
            catalog,
            storage,
            portal_state,
            initial_url,
            relay,
            creds,
            creds_from_command_line,
            settings,
        )
    }

    /// The rest of `new`, which needs no window: the tests start the app with it.
    #[allow(clippy::too_many_arguments)]
    fn start(
        player: PlayerHandle,
        catalog: Arc<CatalogStore>,
        storage: Storage,
        portal_state: PortalState,
        initial_url: Option<String>,
        relay: Option<Relay>,
        creds: Option<XtreamCreds>,
        creds_from_command_line: bool,
        settings: Settings,
    ) -> Self {
        let favorites = Favorites::load(&storage.favorites_path()).unwrap_or_default();
        let presets = Presets::load(&storage.presets_path()).unwrap_or_default();

        let lang = settings.lang().unwrap_or_else(crate::i18n::system_lang);
        let choosing_language = settings.lang().is_none().then_some(lang);
        let epg_fetcher = EpgFetcher::new(catalog.portal().clone());
        let logos = LogoStore::new(storage.cache_dir().join("logos"));
        let today = crate::guide::time::day_of(chrono::Utc::now());
        let mut app = Self {
            player,
            catalog,
            favorites,
            storage,
            portal_state,
            creds,
            creds_from_command_line,
            video_tex: None,
            last_frame_version: 0,
            current_idx: None,
            current_name: None,
            current_stream_id: None,
            current_epg: None,
            epg_slot: Arc::new(Mutex::new(None)),
            news_picker_slot: Arc::new(Mutex::new(None)),
            news_fetching: false,
            catalog_generation: 0,
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
            guide_open: false,
            guide_view: GuideView::new(today),
            guide_channels: Vec::new(),
            guide_channels_for: None,
            guide_channels_revision: 0,
            epg_store: EpgStore::default(),
            epg_fetcher,
            guide_fetching: false,
            logos,
            epg_cache_read_for: None,
            epg_cache_slot: Arc::new(Mutex::new(None)),
            epg_cache_dirty_since: None,
            epg_cache_checked: None,
            epg_cache_writer: None,
            epg_cache_reading_since: None,
            epg_cache_write_failed: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            epg_cache_retry_after: None,
            stream_starting_since: None,
            pending_catchup: None,
            borderless: false,
            show_help: false,
            current_url: None,
            current_live: false,
            current_adult_hint: false,
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
            playing_resumed: false,
            catalog_noted: false,
            settings_ui: None,
            conn_state: ConnState::Idle,
            conn_slot: Arc::new(Mutex::new(None)),
            pin_gate: PinGate::new(),
            adult_note: None,
            frame_fence: FrameFence::default(),
            news_found: Vec::new(),
            news_found_for: None,
            settings_dirty: false,
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
                if let Some(record) = skipped_record(&lw, catalog_says) {
                    record.save(&self.storage.last_watched_path());
                }
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
        self.playing_resumed = true;
    }

    /// The catalog has loaded, which until now could not say whether the stream
    /// that plays is adult. last_watched.json holds an old word of the file (a
    /// resume) or none (a URL from the command line): it gets the catalog's, once.
    /// A stream the resume played on a file that had it wrong is stopped the way
    /// Lock stops adult playback.
    fn on_catalog_loaded(&mut self) {
        let Some(url) = self.current_url.clone() else {
            return;
        };
        let adult = self.catalog.is_adult_url(&url);
        let record = LastWatched {
            url,
            live: self.current_live,
            adult: Some(adult),
        };
        record.save(&self.storage.last_watched_path());
        if stop_resumed_adult(self.playing_resumed, adult, self.catalog.adult_unlocked()) {
            tracing::info!(
                "resume: the catalog says the resumed stream is adult content - stopped"
            );
            self.stop_playback();
            self.set_toast(self.t(T::LockedToast));
        }
    }

    /// Closes every panel the settings overlay would sit on top of. They are
    /// drawn before it, so Tab with nothing focused, and the Enter or Space
    /// after it, would go to a widget there first (a favorite to zap or
    /// delete) instead of to the overlay, and a click beside the overlay would
    /// play a guide cell (egui 0.27 never lets a guide cell keep the keyboard
    /// focus, so the guide's hazard is the mouse). Panels that only show
    /// something (the toast, the guide strip, the debug HUD) stay.
    fn close_panels_under_settings(&mut self) {
        self.show_favs = false;
        self.show_search = false;
        self.search_query.clear();
        self.show_epg_grid = false;
        self.guide_open = false;
        self.news_picker = None;
    }

    fn open_settings(&mut self) {
        self.close_panels_under_settings();
        self.settings_ui = Some(SettingsUi::new());
        // An earlier visit's verdict is stale; a test still running is not.
        if self.conn_state != ConnState::Testing {
            self.conn_state = ConnState::Idle;
        }
        self.adult_note = None;
        tracing::info!("settings: opened");
    }

    /// Writes every setting, the news rules included. A failed write keeps the
    /// change (and `settings_dirty`) until the app closes, with a notice.
    fn save_settings(&mut self) {
        match self.settings.save(&self.storage.settings_path()) {
            Ok(()) => self.settings_dirty = false,
            Err(e) => {
                tracing::warn!("settings: save failed: {}", e);
                self.set_toast(crate::i18n::t(self.lang, T::SaveFailed));
            }
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
    /// searched again, the guide's channel list is worked out again
    /// (`refresh_guide_channels` notices the lock), and the news settings look
    /// their channels up again (a portal saved while the settings were closed
    /// leaves the lookup's key looking the same).
    fn forget_list_positions(&mut self) {
        self.current_idx = None;
        self.search_hits_for = None;
        self.news_found_for = None;
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
        self.current_adult_hint = false;
        self.current_idx = None;
        self.current_name = None;
        self.current_stream_id = None;
        self.current_epg = None;
        self.playing_resumed = false;
        self.stall_watchdog_armed = false;
        self.stream_starting_since = None;
        self.frame_fence.stop();
        self.video_tex = None;
        tracing::info!("player: stop");
    }

    /// What plays is adult content: as the catalog in use says, or as the guide
    /// column it was picked in says (`current_adult_hint`).
    fn playing_adult(&self) -> bool {
        self.current_url
            .as_deref()
            .is_some_and(|u| self.current_adult_hint || self.catalog.is_adult_url(u))
    }

    fn apply_settings_action(&mut self, action: SettingsAction) {
        // The dialog that sent a PIN reads the app's answer from the note next
        // frame: the note of an earlier attempt must not stand in for it, also
        // when this one is not answered at all (PIN input is blocked).
        if matches!(
            action,
            SettingsAction::Unlock(_)
                | SettingsAction::SetPin(_)
                | SettingsAction::ChangePin { .. }
                | SettingsAction::ResetPin(_)
        ) {
            self.adult_note = None;
        }
        match action {
            SettingsAction::Close => {
                // A write that fails here is tried again when the app closes.
                if self.settings_dirty {
                    self.save_settings();
                }
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
            SettingsAction::SetNewsRules(rules) => {
                self.settings.news_rules = Some(rules);
                self.settings_dirty = true;
            }
        }
    }

    /// Whether `news_found` answers for the rules and the catalog as they are now.
    fn news_found_is_fresh(&self) -> bool {
        let rules = self.settings.news_rules();
        let (loaded, unlocked) = (self.catalog.is_loaded(), self.catalog.adult_unlocked());
        let fresh = |key: &NewsLookup| !key.is_stale(rules, loaded, unlocked);
        self.news_found_for.as_ref().is_some_and(fresh)
    }

    /// The channel each news rule points at, for the settings. A pass over every
    /// channel, so it runs when the rules' channel names, the catalog or the
    /// adult lock changed, not per frame (and `paint_settings` asks for it only
    /// while the News section shows).
    fn refresh_news_found(&mut self) {
        if self.news_found_is_fresh() {
            return;
        }
        let rules = self.settings.news_rules();
        let (loaded, unlocked) = (self.catalog.is_loaded(), self.catalog.adult_unlocked());
        tracing::debug!("settings: news lookup ({} rules)", rules.len());
        self.news_found = self
            .catalog
            .resolve_news(rules)
            .into_iter()
            .map(|found| found.map(|(_, _, name)| name))
            .collect();
        self.news_found_for = Some(NewsLookup::new(rules, loaded, unlocked));
    }

    fn paint_settings(&mut self, ctx: &egui::Context) {
        let Some(overlay) = self.settings_ui.as_ref() else {
            return;
        };
        // The lookup is a pass over every channel: made while its result is on
        // show, not for the other sections.
        if overlay.shows_news() {
            self.refresh_news_found();
        }
        // What the lookup last found is of no use after the rules, the catalog or
        // the lock changed, and the section shows nothing for it then.
        let found_is_fresh = self.news_found_is_fresh();
        let Some(overlay) = self.settings_ui.as_mut() else {
            return;
        };
        let news_found: &[Option<String>] = if found_is_fresh {
            &self.news_found
        } else {
            &[]
        };
        let view = SettingsView {
            lang: self.lang,
            settings: &self.settings,
            creds: self.creds.as_ref(),
            creds_from_command_line: self.creds_from_command_line,
            conn: &self.conn_state,
            adult_unlocked: self.catalog.adult_unlocked(),
            has_pin: self.settings.pin.is_some(),
            pin_wait_secs: self
                .pin_gate
                .wait_left(Instant::now())
                .map(|d| d.as_secs().max(1)),
            adult_note: self.adult_note,
            news_rules: self.settings.news_rules(),
            news_found,
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
    /// flag (`adult_hint`): the resume hands the file's own flag on for while
    /// the catalog cannot tell yet, a guide pick whether its column is adult.
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
        // A stream picked in an adult guide column stays adult when the stall
        // watchdog or an eof reloads it; any other load starts without the mark.
        let adult_hint = if reload && self.current_adult_hint {
            Some(true)
        } else {
            adult_hint
        };
        self.current_adult_hint = adult_hint == Some(true);
        // Another stream is a pick, which the catalog's first word leaves alone.
        if !reload {
            self.playing_resumed = false;
        }
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
        self.stream_starting_since = Some(Instant::now());
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
                    self.stream_starting_since = None;
                    // mpv restarts before it renders the first picture of the new
                    // file, and the bus still holds the last one of the stream
                    // before. A catch-up wait goes on until a newer picture is
                    // there (`end_catchup_wait`); the first restart counts.
                    let frames = &self.player.frames;
                    if let Some(wait) = self.pending_catchup.as_mut() {
                        wait.started
                            .get_or_insert_with(|| (frames.version(), Instant::now()));
                    }
                }
                Event::EndOfFile { reason } => {
                    // Before its first picture a catch-up pick that ends has failed;
                    // the stop of the stream before it ("stop") is no failure.
                    if reason == "error" || reason == "eof" {
                        if let Some(wait) = self.pending_catchup.as_mut() {
                            if wait.failed.is_none() {
                                tracing::warn!(
                                    "catch-up: failed before its first frame ({reason})"
                                );
                                wait.failed =
                                    Some(crate::i18n::t(self.lang, T::WaitStreamEnded).to_string());
                            }
                            continue;
                        }
                    }
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
                    match self.pending_catchup.as_mut() {
                        Some(wait) => {
                            wait.failed.get_or_insert(msg);
                        }
                        None => self.set_toast(self.tr(T::ErrorMsg, &[&msg])),
                    }
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

    /// The guide's EPG of the channel `stream_id` plays, while it is fresh, from
    /// the programme on now, 100 programmes at most: what the short EPG gave the
    /// toast, the strip and the overview (Shift+E). The past is of no use to
    /// them, and the day table holds about 600 programmes from a week back. With
    /// nothing on now or after, the portal is asked as before.
    /// Zapping picks the same variant as the guide's live id, so a lookup by id
    /// finds it; another variant (a quality switch) is fetched as before.
    fn fresh_guide_epg(&self, stream_id: i64) -> Option<Epg> {
        let channel = self
            .guide_channels
            .iter()
            .find(|c| c.live_id == stream_id || c.archive_id == stream_id)?;
        let now = chrono::Utc::now();
        if self.epg_store.needs_fetch(channel.archive_id, now) {
            return None;
        }
        let from_now: Vec<EpgEntry> = self
            .epg_store
            .get(channel.archive_id)?
            .entries()
            .iter()
            .filter(|e| e.end > now)
            .take(100)
            .cloned()
            .collect();
        (!from_now.is_empty()).then(|| Epg::new(from_now))
    }

    fn kick_epg_fetch(&mut self, stream_id: i64) {
        // The guide's store has this channel fresh: no request to the portal.
        if let Some(epg) = self.fresh_guide_epg(stream_id) {
            tracing::info!("epg for sid={} from the guide's store", stream_id);
            self.epg_fetch_pending_for = Some(stream_id);
            *self.epg_slot.lock() = Some((stream_id, epg));
            return;
        }
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

    /// Open the `n` news chooser: per news rule (settings, else the
    /// defaults) find the channel, fetch the day EPGs concurrently, keep the
    /// matching past programmes (3 per rule, 10 in total, newest first), then
    /// pop the modal picker. Fully async - the current channel keeps playing.
    fn open_news_picker(&mut self) {
        if self.news_fetching {
            return;
        }
        let rules = self.settings.news_rules_or_default();
        if rules.is_empty() {
            tracing::info!("news picker: no rules");
            self.set_toast(self.t(T::NoNewsRules));
            return;
        }
        // Each rule finds its catch-up (tv_archive=1) channel - that's where
        // both the EPG and the timeshift live - else the live one. Only channels
        // that show count, so while locked a rule pointing at an adult channel
        // finds nothing.
        let resolved = self.catalog.resolve_news(&rules);
        let mut sources: Vec<(i64, bool, NewsRule)> = Vec::new();
        for (rule, found) in rules.into_iter().zip(resolved) {
            match found {
                Some((sid, _, _)) if sources.iter().any(|(s, _, _)| *s == sid) => {}
                Some((sid, is_archive, _)) => sources.push((sid, is_archive, rule)),
                None => tracing::info!("news source {} not in catalog", rule.channel),
            }
        }
        if sources.is_empty() {
            self.set_toast(self.t(T::NoNewsChannels));
            return;
        }
        self.set_toast(self.t(T::FetchingNews));
        self.news_fetching = true;
        let stamp = self.news_stamp();
        let portal = self.catalog.portal().clone();
        let slot = self.news_picker_slot.clone();
        tokio::spawn(async move {
            let now = chrono::Utc::now();
            let permits = Arc::new(tokio::sync::Semaphore::new(NEWS_FETCHES_IN_FLIGHT));
            let handles: Vec<_> = sources
                .into_iter()
                .map(|(sid, is_archive, rule)| {
                    let (p, permits) = (portal.clone(), permits.clone());
                    (
                        sid,
                        is_archive,
                        rule,
                        tokio::spawn(async move {
                            within_limit(&permits, async { p.fetch_day_epg(sid).await.ok() }).await
                        }),
                    )
                })
                .collect();
            let mut items: Vec<NewsItem> = Vec::new();
            for (sid, is_archive, rule, h) in handles {
                let epg = h.await.ok().flatten();
                let tag = crate::news::rule_label(&rule);
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
                        tag,
                        offset: 0,
                        live: true,
                    });
                    continue;
                }
                if let Some(epg) = epg {
                    // Per rule: matching past entries, newest first, capped at
                    // 3 so a 24h news channel can't flood the combined list.
                    let mut found: Vec<NewsItem> = epg
                        .entries()
                        .iter()
                        .filter(|e| e.start <= now)
                        .filter(|e| crate::news::title_matches(&rule.patterns, &e.title))
                        .map(|e| NewsItem {
                            sid,
                            title: e.title.clone(),
                            start: e.start,
                            end: e.end,
                            tag: tag.clone(),
                            offset: i64::from(rule.skip_secs),
                            live: false,
                        })
                        .collect();
                    found.sort_by_key(|i| std::cmp::Reverse(i.start));
                    found.truncate(3);
                    items.extend(found);
                }
            }
            items.sort_by_key(|i| std::cmp::Reverse(i.start)); // newest first across sources
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
            *slot.lock() = Some(NewsResult { items, stamp });
        });
    }

    /// What a news list fetched now would be made for.
    fn news_stamp(&self) -> NewsStamp {
        NewsStamp {
            generation: self.catalog_generation,
            unlocked: self.catalog.adult_unlocked(),
        }
    }

    /// When the async news list lands, pop the picker (newest pre-selected).
    fn drain_news_picker(&mut self) {
        let Some(result) = self.news_picker_slot.lock().take() else {
            return;
        };
        self.news_fetching = false;
        // Not under the settings overlay either: a panel with a Tab stop of its own
        // must not appear beneath it (see close_panels_under_settings).
        if self.settings_ui.is_some() {
            tracing::info!("news picker: list dropped, the settings are open");
            return;
        }
        if !keep_news_result(result.stamp, self.news_stamp()) {
            tracing::info!("news picker: list dropped, the catalog or the lock changed since");
            return;
        }
        self.news_picker = Some(NewsPicker {
            items: result.items,
            selected: 0,
        });
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
            // Its default click sense makes the frame a Tab stop that shows
            // nothing (see paint_toast): the first Tab goes to the Server field.
            .sense(egui::Sense::hover())
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
                            submit = Some(self.prompt_credentials());
                        }
                    });
            });
        if let Some(s) = submit {
            // Saving tests first: only credentials that log in are kept.
            self.start_conn_check(s, true);
        }
    }

    /// The `user:pass@server` line the setup prompt tests and saves, made of its
    /// three fields as they were typed or pasted (see `credentials_line`).
    fn prompt_credentials(&self) -> String {
        credentials_line(&self.cfg_user, &self.cfg_pass, &self.cfg_host)
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
        self.creds_from_command_line = false;
        let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(creds));
        self.catalog = Arc::new(CatalogStore::new(portal));
        self.catalog_generation += 1;
        // Another portal: its stream ids mean other channels, so the old lists'
        // positions and the guide's per-channel data are of no use. The new
        // catalog starts locked, so the last PIN outcome no longer applies.
        self.forget_list_positions();
        // The old portal's EPG and fetches mean other channels now.
        self.epg_store.clear();
        self.epg_fetcher.reset(self.catalog.portal().clone());
        self.guide_view = GuideView::new(crate::guide::time::day_of(chrono::Utc::now()));
        self.guide_fetching = false;
        // The new portal's file is read when its catalog loads (the generation
        // changed); what the old portal's EPG left to write goes with it. So does
        // a write of the old portal's that fails later: the flag it sets is not
        // the one the new portal's store looks at.
        self.epg_cache_dirty_since = None;
        self.epg_cache_reading_since = None;
        self.epg_cache_retry_after = None;
        self.epg_cache_write_failed = Arc::new(std::sync::atomic::AtomicBool::new(false));
        self.adult_note = None;
        // Whatever waited for the old portal's catalog, or was told by it, means
        // nothing to the new one: a 0.5 resume would play the old portal's URL
        // through it, an old stream id would be another channel.
        self.pending_resume = None;
        self.current_stream_id = None;
        self.playing_resumed = false;
        self.catalog_noted = true;
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
            // The guide that opens reads its keys in this same frame, and its
            // `g` closes it again.
            spend_key(ctx, Key::G);
            self.open_guide();
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

    /// The video texture follows the bus. This runs before the frame is painted,
    /// so the frame that ends a catch-up wait draws the picture that ended it.
    fn update_video_texture(&mut self, ctx: &egui::Context) {
        let picture = self.upload_newest_frame(ctx);
        self.end_catchup_wait(picture, Instant::now());
    }

    /// Puts the newest frame on the bus into the video texture, unless it is the
    /// one there already or the fence holds it back. The version of the frame it
    /// put there, if it did.
    fn upload_newest_frame(&mut self, ctx: &egui::Context) -> Option<u64> {
        let frame: RgbaFrame = self.player.frames.read();
        if frame.w == 0 || frame.h == 0 {
            return None;
        }
        // After a stop the last picture must not come back.
        if !self.frame_fence.admit(frame.version) {
            return None;
        }
        if frame.version == self.last_frame_version && self.video_tex.is_some() {
            return None;
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
        Some(frame.version)
    }

    /// A catch-up wait ends on the first picture newer than mpv's restart (`picture`
    /// is the version of the one that was just put in the texture), or a second
    /// after the restart if none comes: audio only, or a stream without video.
    /// A failed wait stays, for the user to read it.
    fn end_catchup_wait(&mut self, picture: Option<u64>, now: Instant) {
        let Some(wait) = &self.pending_catchup else {
            return;
        };
        let Some((restart, at)) = wait.started else {
            return;
        };
        if wait.failed.is_some() {
            return;
        }
        if picture.is_some_and(|version| version > restart) {
            tracing::info!("catch-up: first frame");
        } else if now.saturating_duration_since(at) >= WAIT_FOR_PICTURE {
            tracing::info!("catch-up: no picture 1 s after the start");
        } else {
            return;
        }
        self.pending_catchup = None;
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
        let loaded = self.catalog.is_loaded();
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
                            let text = if loaded {
                                T::FavoritesEmpty
                            } else {
                                T::CatalogLoading
                            };
                            ui.label(egui::RichText::new(crate::i18n::t(lang, text)).italics());
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
                        // No scroll bar and no drag area: each is a Tab stop, and the
                        // settings row is to be the one. The marker shows where the
                        // rows are scrolled to.
                        let scrolled = marker_scroll_area("__help_rows__")
                            .max_height(rows_height)
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
                        paint_scroll_marker(ui, &scrolled);
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
    // Guide (`g` key): one column per channel with catch-up. guide_ui.rs
    // draws it and hands back what the user did.
    // ------------------------------------------------------------------

    /// The guide's columns again when the catalog (another portal's, or this
    /// one's loaded or fetched again by F5) or the lock changed; three
    /// comparisons otherwise. The key is read before the list: a catalog that
    /// lands in between is noticed next frame.
    fn refresh_guide_channels(&mut self) {
        let key = (
            self.catalog_generation,
            self.catalog.revision(),
            self.catalog.adult_unlocked(),
        );
        if self.guide_channels_for == Some(key) {
            return;
        }
        self.guide_channels = self.catalog.guide_channels();
        self.guide_channels_for = Some(key);
        self.guide_channels_revision += 1;
    }

    /// `g`: the guide opens as it was left (in Nu & straks: on now). From now on
    /// its EPG is fetched in the background.
    fn open_guide(&mut self) {
        if !self.catalog.is_loaded() {
            self.set_toast(self.t(T::GuideCatalogLoading));
            return;
        }
        self.refresh_guide_channels();
        self.guide_view
            .opened(crate::guide::time::day_of(chrono::Utc::now()));
        self.guide_open = true;
        self.guide_fetching = true;
        tracing::info!("guide: opened ({} channels)", self.guide_channels.len());
    }

    /// The guide back from the wait screen, as it was left: not lined up on
    /// now, as `g` does in Nu & straks, since the user picked a programme there.
    fn reopen_guide(&mut self) {
        self.guide_view.reopened();
        self.guide_open = true;
        self.guide_fetching = true;
        tracing::info!("guide: opened ({} channels)", self.guide_channels.len());
    }

    fn close_guide(&mut self) {
        if self.guide_open {
            self.guide_open = false;
            tracing::info!("guide: closed");
        }
    }

    /// Takes the finished EPG fetches in and starts what may start: 2 at a time
    /// with the guide open, 1 with it closed, none while a stream starts or this
    /// catalog's cache file is being read.
    fn tick_epg(&mut self) {
        let now = Instant::now();
        let landed = self.epg_fetcher.drain(now);
        let any = !landed.is_empty();
        for fetched in landed {
            if let Ok(epg) = fetched.result {
                self.epg_store
                    .insert(fetched.archive_id, ChannelEpg::new(epg, chrono::Utc::now()));
                self.epg_cache_dirty_since.get_or_insert_with(Instant::now);
            }
        }
        if self.guide_fetching {
            self.start_epg_fetches(now);
        }
        // After the start, so a fetch that is followed by the next one does not
        // count: this line marks the end of a round (or a rest after a failure),
        // one for all the fetches of it.
        if any && self.epg_fetcher.in_flight().is_empty() {
            tracing::info!(
                "guide: epg store {} channels, {} programmes",
                self.epg_store.len(),
                self.epg_store.programmes()
            );
        }
    }

    /// Starts EPG fetches while fewer run than the limit allows.
    fn start_epg_fetches(&mut self, now: Instant) {
        let starting = self
            .stream_starting_since
            .is_some_and(|t| t.elapsed() < Duration::from_secs(10));
        // What the cache file holds need not be fetched: a guide opened before
        // the read has landed would ask the portal for channels it is about to
        // fill. Not for ever, should the reading thread hang.
        let reading = self
            .epg_cache_reading_since
            .is_some_and(|since| now.saturating_duration_since(since) < EPG_CACHE_READ_WAIT);
        let limit = if starting || reading {
            0
        } else if self.guide_open {
            2
        } else {
            1
        };
        // The columns on screen first (under a filter they are not next to one
        // another), then the rest from the leftmost one on. A channel named twice
        // costs nothing: the fetcher skips what is in flight or fresh. The guide
        // list may have changed since the last frame, hence `get`.
        let (start, count) = self.guide_view.visible_window();
        let channels = &self.guide_channels;
        let on_screen = self
            .guide_view
            .visible_columns()
            .iter()
            .filter_map(|&i| channels.get(i))
            .map(|c| c.archive_id);
        let order = on_screen.chain(
            fetch_order(start..start + count.max(1), channels.len())
                .map(|i| channels[i].archive_id),
        );
        self.epg_fetcher
            .tick(&self.epg_store, order, limit, now, chrono::Utc::now());
    }

    /// The guide's cache file of the portal in use (none without one).
    fn epg_cache_path(&self) -> Option<std::path::PathBuf> {
        let creds = self.creds.as_ref()?;
        let name = crate::guide::cache::file_name(&creds.host, creds.port);
        Some(self.storage.cache_dir().join(name))
    }

    /// What the cache file gets: the guide's channels that have EPG; adult ones
    /// never.
    fn epg_cache_contents(&self) -> Option<(std::path::PathBuf, Vec<crate::guide::cache::ToSave>)> {
        let path = self.epg_cache_path()?;
        let channels = self
            .guide_channels
            .iter()
            .filter(|c| !c.adult)
            .filter_map(|c| {
                Some(crate::guide::cache::ToSave {
                    id: c.archive_id,
                    epg: self.epg_store.get(c.archive_id)?.clone(),
                    archive_days: c.archive_days,
                })
            })
            .collect();
        Some((path, channels))
    }

    /// Reads the cache file once per catalog, on a thread of its own, as soon
    /// as the catalog has loaded, and takes in what it held.
    fn read_epg_cache(&mut self) {
        if self.epg_cache_read_for != Some(self.catalog_generation) && self.catalog.is_loaded() {
            self.epg_cache_read_for = Some(self.catalog_generation);
            if let Some(path) = self.epg_cache_path() {
                let (slot, generation) = (self.epg_cache_slot.clone(), self.catalog_generation);
                self.epg_cache_reading_since = Some(Instant::now());
                std::thread::spawn(move || {
                    let read = crate::guide::cache::load(&path, chrono::Utc::now());
                    *slot.lock() = Some((generation, read));
                });
            }
        }
        let Some((generation, read)) = self.epg_cache_slot.lock().take() else {
            return;
        };
        if generation != self.catalog_generation {
            return;
        }
        // The guide's fetches may start.
        self.epg_cache_reading_since = None;
        match read {
            Ok(channels) => {
                let known: std::collections::HashSet<i64> =
                    self.guide_channels.iter().map(|c| c.archive_id).collect();
                let (mut kept, mut programmes) = (0, 0);
                for (id, epg) in channels {
                    if known.contains(&id) {
                        programmes += epg.entries().len();
                        self.epg_store.insert(id, epg);
                        kept += 1;
                    }
                }
                tracing::info!(
                    "guide: epg cache read ({} channels, {} programmes)",
                    kept,
                    programmes
                );
            }
            Err(e) => tracing::warn!("guide: epg cache unreadable ({}) - building a new one", e),
        }
    }

    /// Writes the cache file, on a thread of its own, once the EPG that came in
    /// is complete (nothing in flight, no guide channel left to fetch), or once
    /// it has been coming in for a minute (a long first round, channels that
    /// keep failing). Not at a pause of the fetching: a stream that starts
    /// stops it for a while, and a file written then would hold a few channels.
    ///
    /// This runs every frame while the store is dirty, so what it costs stays
    /// small. Whether the round is complete is a walk over the guide's channels:
    /// it runs when nothing is in flight, and once a second at most (`now` is
    /// the clock). The minute needs no walk. And one writer at a time: the file
    /// goes through a temporary one of a fixed name, which two writers would
    /// fill together. A write that is due while the writer still runs waits;
    /// the store stays dirty and a later frame writes. A writer that has
    /// finished is taken in first, whether the store is dirty or not: a write
    /// that failed marks it dirty again.
    fn write_epg_cache_when_due(&mut self, now: Instant) {
        if self
            .epg_cache_writer
            .as_ref()
            .is_some_and(|writer| writer.is_finished())
        {
            if let Some(writer) = self.epg_cache_writer.take() {
                let _ = writer.join();
            }
            self.note_failed_epg_cache_write(now);
        }
        let Some(since) = self.epg_cache_dirty_since else {
            return;
        };
        if self.epg_cache_writer.is_some() || self.epg_cache_retry_after.is_some_and(|t| now < t) {
            return;
        }
        if now.saturating_duration_since(since) < Duration::from_secs(60) {
            if !self.epg_fetcher.in_flight().is_empty() {
                return;
            }
            let looked_lately = self
                .epg_cache_checked
                .is_some_and(|t| now.saturating_duration_since(t) < Duration::from_secs(1));
            if looked_lately {
                return;
            }
            self.epg_cache_checked = Some(now);
            let utc_now = chrono::Utc::now();
            let left_to_fetch = self
                .guide_channels
                .iter()
                .any(|c| self.epg_store.needs_fetch(c.archive_id, utc_now));
            if left_to_fetch {
                return;
            }
        }
        if let Some((path, channels)) = self.epg_cache_contents() {
            self.epg_cache_dirty_since = None;
            let failed = self.epg_cache_write_failed.clone();
            self.epg_cache_writer = Some(std::thread::spawn(move || {
                let written = write_epg_cache(&path, &channels);
                failed.store(!written, std::sync::atomic::Ordering::Release);
            }));
        }
    }

    /// A cache write that failed (the writer says so in `epg_cache_write_failed`)
    /// marks the store dirty again, from `now`, and is tried again after a
    /// minute: not at once, since a failure that stays (the file is locked, the
    /// disk is full) would write, fail and log on every frame.
    fn note_failed_epg_cache_write(&mut self, now: Instant) {
        let failed = self
            .epg_cache_write_failed
            .swap(false, std::sync::atomic::Ordering::AcqRel);
        if failed {
            self.epg_cache_dirty_since.get_or_insert(now);
            self.epg_cache_retry_after = Some(now + EPG_CACHE_RETRY);
        }
    }

    /// The guide over the whole window, and what the user did in it. Under the
    /// help it takes no keys; `?` or Esc closes the help.
    fn paint_guide(&mut self, ctx: &egui::Context) {
        let keys = !self.show_help;
        if self.show_help {
            let (question, esc) = ctx.input(|i| {
                (
                    i.events
                        .iter()
                        .any(|e| matches!(e, egui::Event::Text(t) if t == "?")),
                    i.key_pressed(Key::Escape),
                )
            });
            if question || esc {
                self.show_help = false;
            }
        }
        let now = chrono::Utc::now();
        let data = GuideData {
            lang: self.lang,
            channels: &self.guide_channels,
            channels_revision: self.guide_channels_revision,
            store: &self.epg_store,
            fetching: self.epg_fetcher.in_flight(),
            playing: self.current_name.as_deref(),
            now,
            today: crate::guide::time::day_of(now),
            keys,
        };
        let action = self.guide_view.show(ctx, &data, &mut self.logos);
        match action {
            Some(GuideAction::Play {
                channel,
                entry,
                playable,
            }) => self.guide_play(channel, entry, playable),
            Some(GuideAction::Close) => self.close_guide(),
            Some(GuideAction::Help) => self.show_help = true,
            None => {}
        }
    }

    /// A programme picked in the guide: live through the relay, catch-up from
    /// its start on the catch-up twin, or a word on why not.
    fn guide_play(&mut self, channel: GuideChannel, entry: EpgEntry, playable: Playable) {
        let (from, to) = (crate::epg::hhmm(entry.start), crate::epg::hhmm(entry.end));
        match playable {
            Playable::Future => self.set_toast(self.tr(T::GuideStartsAt, &[&from])),
            Playable::Unavailable => self.set_toast(self.t(T::GuideNotReplayable)),
            Playable::Live => {
                tracing::info!(
                    "guide: [LIVE] {} | {} ({} -> {})",
                    channel.name,
                    entry.title,
                    from,
                    to
                );
                let url = self.catalog.portal().live_stream_url(channel.live_id);
                // The column is adult when any variant is flagged: Lock and the
                // resume go by that, not by this stream's own flag alone.
                self.load_url_with(url, true, Some(channel.adult));
                // Up/down continues from this channel.
                self.current_idx = None;
                self.current_stream_id = Some(channel.live_id);
                self.current_name = Some(channel.name.clone());
                // `e` and Shift+E: the store holds the EPG just picked from,
                // with no request. A stale copy is fetched as a zap fetches it.
                self.current_epg = self.fresh_guide_epg(channel.live_id);
                if self.current_epg.is_none() {
                    self.kick_epg_fetch(channel.live_id);
                }
                self.set_toast(self.tr(T::GuideLiveToast, &[&channel.name, &entry.title]));
                self.close_guide();
            }
            Playable::Replay => {
                tracing::info!(
                    "guide: [CATCHUP] {} | {} ({} {} -> {})",
                    channel.name,
                    entry.title,
                    entry.start.with_timezone(&chrono::Local).format("%Y-%m-%d"),
                    from,
                    to
                );
                let minutes = (entry.end - entry.start).num_minutes().max(1) as u32;
                let portal = self.catalog.portal();
                let url = portal.catchup_url(channel.archive_id, entry.start, minutes);
                // The picture of the channel before goes, as on a stop, and none
                // of its frames comes back: the wait screen covers the window
                // until the programme's first one, and a wait that ends without
                // a picture (the 1 s cap) shows the empty state, not that channel.
                self.frame_fence.stop();
                self.video_tex = None;
                self.load_url_with(url, false, Some(channel.adult));
                self.current_idx = None;
                self.current_stream_id = Some(channel.archive_id);
                self.current_name = Some(format!("{} - {}", channel.name, entry.title));
                self.current_epg = None;
                self.close_guide();
                tracing::info!("catch-up: waiting for the first frame");
                self.pending_catchup = Some(PendingCatchup {
                    channel,
                    title: entry.title,
                    start: entry.start,
                    end: entry.end,
                    failed: None,
                    started: None,
                });
            }
        }
    }

    /// The wait screen takes Esc and g only: the load stops and the guide comes
    /// back as it was left.
    fn handle_wait_keys(&mut self, ctx: &egui::Context) {
        let (esc, g) = ctx.input(|i| (i.key_pressed(Key::Escape), i.key_pressed(Key::G)));
        if esc || g {
            self.pending_catchup = None;
            self.stop_playback();
            tracing::info!("catch-up: cancelled");
            // The guide that opens reads its keys in this same frame: its `g`
            // would close it again, its Esc run the Esc chain (the filter, else
            // closing).
            spend_key(ctx, Key::Escape);
            spend_key(ctx, Key::G);
            self.reopen_guide();
        }
    }

    fn paint_wait(&mut self, ctx: &egui::Context) {
        let Some(wait) = &self.pending_catchup else {
            return;
        };
        let info = WaitInfo {
            lang: self.lang,
            channel: &wait.channel,
            title: &wait.title,
            start: wait.start,
            end: wait.end,
            today: crate::guide::time::day_of(chrono::Utc::now()),
            failed: wait.failed.as_deref(),
        };
        paint_wait_screen(ctx, &info, &mut self.logos);
    }
}

impl Drop for TvApp {
    fn drop(&mut self) {
        // The player first: mpv and its audio would run on through the wait for
        // the cache writer and through the write below, tens to a few hundred
        // ms. Nothing below depends on mpv.
        let _ = self.player.cmd_tx.send(Cmd::Quit);
        let _ = self.favorites.save(&self.storage.favorites_path());
        // Rules typed in settings that were still open, or not written yet
        // when they closed.
        if self.settings_dirty {
            let _ = self.settings.save(&self.storage.settings_path());
        }
        // A writer still at work finishes first: two would fill the same
        // temporary file, and a write that exit killed would be lost. One that
        // failed marks the store dirty again, so the write below tries again.
        if let Some(writer) = self.epg_cache_writer.take() {
            let _ = writer.join();
        }
        self.note_failed_epg_cache_write(Instant::now());
        // The guide's EPG that came in since the last write.
        if self.epg_cache_dirty_since.is_some() {
            if let Some((path, channels)) = self.epg_cache_contents() {
                write_epg_cache(&path, &channels);
            }
        }
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
        self.update_ui(ctx);
    }
}

impl TvApp {
    /// One frame of the app: what `update` does, minus the window's `Frame`,
    /// which the tests do not have.
    fn update_ui(&mut self, ctx: &egui::Context) {
        self.drain_events();
        self.drain_epg();
        self.drain_news_picker();
        self.drain_conn_check();
        self.refresh_guide_channels();
        self.tick_epg();
        self.read_epg_cache();
        self.write_epg_cache_when_due(Instant::now());
        self.logos.drain();
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
        if !self.catalog_noted && self.catalog.is_loaded() {
            self.catalog_noted = true;
            self.on_catalog_loaded();
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
        // The open guide reads its own keys in paint_guide, as the settings
        // overlay does in paint_settings (typing in its filter must not fire
        // hotkeys); the setup prompt owns the keyboard. The wait screen of a
        // catch-up pick takes Esc and g only.
        if self.settings_ui.is_none() && !setup_prompt {
            if self.pending_catchup.is_some() {
                self.handle_wait_keys(ctx);
            } else if !self.guide_open {
                self.handle_keys(ctx);
            }
        }

        // The wait screen of a catch-up pick takes over the full window, and so
        // does the guide when open. The video texture was brought up to date
        // above in every case, so the frame that ends a wait draws the picture
        // that ended it.
        if self.pending_catchup.is_some() {
            self.paint_wait(ctx);
        } else if self.guide_open {
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
            // The Enter or Space that clicked the row is spent. The overlay, drawn
            // later in this frame, would take that Enter for "open the section".
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter);
                i.consume_key(egui::Modifiers::NONE, Key::Space);
            });
        }
        self.paint_settings(ctx);

        ctx.request_repaint_after(Duration::from_millis(33));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// 3 is a normal channel, 901 and 902 carry the portal's adult flag (the
    /// name of 902 starts with XXX, which means nothing by itself).
    fn adult_catalog() -> crate::portal::Catalog {
        let live = |stream_id: i64, name: &str, is_adult: bool| LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult,
            stream_icon: None,
        };
        crate::portal::Catalog {
            live: vec![
                live(3, "TEST 03", false),
                live(901, "24/7 | ADULT A", true),
                live(902, "XXX | ADULT B", true),
            ],
            ..Default::default()
        }
    }

    /// The guide's channels: TEST 03 with its catch-up twin 13, and the adult
    /// 901 with its twin 911.
    fn guide_catalog() -> Arc<CatalogStore> {
        flagged_guide_catalog(true, true)
    }

    /// `guide_catalog` with the portal's adult flag on 901 (`live`) and on 911
    /// (`archive`) as given. Its column is adult when either carries it.
    fn flagged_guide_catalog(live: bool, archive: bool) -> Arc<CatalogStore> {
        let ch = |stream_id: i64, name: &str, archive: i32, is_adult: bool| LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: archive,
            tv_archive_duration: 7 * archive,
            is_adult,
            stream_icon: None,
        };
        let catalog = crate::portal::Catalog {
            live: vec![
                ch(3, "TEST 03", 0, false),
                ch(13, "TEST 03 HD", 1, false),
                ch(901, "24/7 | ADULT A", 0, live),
                ch(911, "24/7 | ADULT A HD", 1, archive),
            ],
            ..Default::default()
        };
        Arc::new(CatalogStore::from_catalog(idle_portal(), catalog))
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
        // The loaded catalog's word beats a hint that says "not adult" ...
        assert_eq!(
            flag(&loaded, adult_url, Some(false), false),
            Some(Some(true))
        );
        assert_eq!(flag(&loaded, normal_url, None, false), Some(Some(false)));
        // ... but not one that says "adult": the guide's column is adult when any
        // of its variants carries the flag, and the stream's own may not.
        assert_eq!(
            flag(&loaded, normal_url, Some(true), false),
            Some(Some(true))
        );
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
        favorites.toggle(902, "Renamed"); // flagged in the catalog, saved under another name
        favorites.toggle(77, "XXX | Gone"); // not in the catalog: nothing says it is adult
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
        // Locked: the flagged channels are left out of the list and act empty when
        // recalled; empty slots stay empty. A name starting with XXX decides nothing.
        assert_eq!(listed(&catalog), [3, 77]);
        assert_eq!(
            recalled(&catalog),
            [None, None, None, Some(3), Some(77), None]
        );
        // Unlocked: all of them.
        catalog.set_adult_unlocked(true);
        assert_eq!(listed(&catalog), [3, 901, 902, 77]);
        assert_eq!(
            recalled(&catalog),
            [None, Some(901), Some(902), Some(3), Some(77), None]
        );
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

    /// Starts `tasks` fetches that each take a moment, all through one semaphore
    /// of `limit` permits, and returns the most that were in flight at once.
    async fn most_in_flight(limit: usize, tasks: usize) -> usize {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let permits = Arc::new(tokio::sync::Semaphore::new(limit));
        let (now, most) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
        let handles: Vec<_> = (0..tasks)
            .map(|_| {
                let (permits, now, most) = (permits.clone(), now.clone(), most.clone());
                tokio::spawn(async move {
                    within_limit(&permits, async {
                        let in_flight = now.fetch_add(1, Ordering::SeqCst) + 1;
                        most.fetch_max(in_flight, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(20)).await;
                        now.fetch_sub(1, Ordering::SeqCst);
                    })
                    .await
                })
            })
            .collect();
        for handle in handles {
            handle.await.expect("a fetch task panicked");
        }
        most.load(Ordering::SeqCst)
    }

    #[tokio::test]
    async fn a_long_rule_list_cannot_burst_the_portal() {
        // 20 fetches wanted: 8 at a time ...
        assert_eq!(
            most_in_flight(NEWS_FETCHES_IN_FLIGHT, 20).await,
            NEWS_FETCHES_IN_FLIGHT
        );
        // ... while the 7 default rules all run at once.
        assert_eq!(most_in_flight(NEWS_FETCHES_IN_FLIGHT, 7).await, 7);
    }

    fn news_rule(channel: &str) -> NewsRule {
        NewsRule {
            channel: channel.into(),
            patterns: "*".into(),
            label: String::new(),
            skip_secs: 0,
        }
    }

    #[test]
    fn the_found_lines_are_worked_out_again_only_when_a_channel_or_the_catalog_changes() {
        let rules = vec![news_rule("NPO 1"), news_rule("CNN")];
        let lookup = NewsLookup::new(&rules, true, false);
        let stale = |rules: &[NewsRule], loaded, unlocked| lookup.is_stale(rules, loaded, unlocked);
        assert!(!stale(&rules, true, false), "nothing changed");
        // Typing in the patterns, the label or the seconds never moves a rule to
        // another channel.
        let mut typed = rules.clone();
        typed[0].patterns = "*journaal*; !*sport*".into();
        typed[1].label = "C".into();
        typed[1].skip_secs = 30;
        assert!(!stale(&typed, true, false));
        // A channel name does.
        let mut renamed = rules.clone();
        renamed[1].channel = "CNN International".into();
        assert!(stale(&renamed, true, false));
        let swapped = vec![news_rule("CNN"), news_rule("NPO 1")];
        assert!(stale(&swapped, true, false));
        // So does a rule more or less, which is a line more or less.
        let mut added = rules.clone();
        added.push(news_rule("BBC News"));
        assert!(stale(&added, true, false));
        assert!(stale(&rules[..1], true, false));
        assert!(stale(&[], true, false));
        // The catalog loading, and the adult lock changing which channels show.
        assert!(stale(&rules, false, false));
        assert!(stale(&rules, true, true));
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

    // ---- the app on a headless egui context ----

    /// A key press as egui gets it from the window.
    fn key_press(key: Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The app on a headless egui context, with no window and no mpv behind it.
    /// Frames run through `update_ui`; what the app tells the player arrives in
    /// `cmds`.
    struct Harness {
        app: TvApp,
        ctx: egui::Context,
        cmds: tokio::sync::mpsc::UnboundedReceiver<Cmd>,
        /// Player events for the app, as mpv sends them.
        events: tokio::sync::mpsc::UnboundedSender<Event>,
        /// Every text the last frame painted.
        painted: Vec<String>,
        /// Every texture the last frame drew (the video is one).
        drawn: Vec<egui::TextureId>,
    }

    impl Harness {
        /// The app as `main` starts it, with `root` as its config folder and
        /// two quiet frames drawn, so the widgets exist before the first key.
        fn start(
            root: &std::path::Path,
            catalog: Arc<CatalogStore>,
            state: PortalState,
            initial_url: Option<&str>,
        ) -> Self {
            let (cmd_tx, cmds) = tokio::sync::mpsc::unbounded_channel();
            let (events, evt_rx) = tokio::sync::mpsc::unbounded_channel();
            let player = PlayerHandle {
                cmd_tx,
                evt_rx: Mutex::new(evt_rx),
                frames: Arc::new(crate::player::FrameBus::new()),
            };
            let settings = Settings {
                language: Some("en".into()),
                ..Settings::default()
            };
            let creds = (state == PortalState::Configured).then(|| XtreamCreds {
                username: "u".into(),
                password: "p".into(),
                host: "h".into(),
                port: 80,
            });
            let app = TvApp::start(
                player,
                catalog,
                Storage::with_root(root.to_path_buf()),
                state,
                initial_url.map(str::to_owned),
                None,
                creds,
                false,
                settings,
            );
            let mut harness = Harness {
                app,
                ctx: egui::Context::default(),
                cmds,
                events,
                painted: Vec::new(),
                drawn: Vec::new(),
            };
            harness.frame(&[]);
            harness.frame(&[]);
            // The cache file, which the app reads on a thread of its own once its
            // catalog is loaded, has landed: the guide's fetches wait for it, so a
            // test that counts them starts from there.
            for _ in 0..2000 {
                if harness.app.epg_cache_reading_since.is_none() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(1));
                harness.frame(&[]);
            }
            harness
        }

        /// A picture from the player: `w` x `h` pixels land on the frame bus, as
        /// when mpv renders a frame. The version it gets.
        fn put_frame(&mut self, w: u32, h: u32) -> u64 {
            let frames = &self.app.player.frames;
            frames.write(w, h, vec![0; w as usize * h as usize * 4]);
            frames.version()
        }

        /// One frame in which these events arrive.
        fn frame_with(&mut self, events: Vec<egui::Event>) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                events,
                ..Default::default()
            };
            let app = &mut self.app;
            let output = self.ctx.run(raw, |ctx| app.update_ui(ctx));
            self.painted = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Text(text) => Some(text.galley.text().to_string()),
                    _ => None,
                })
                .collect();
            self.drawn = output
                .shapes
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    // An image is a textured rectangle, or a mesh when it is rotated.
                    egui::Shape::Rect(rect) => Some(rect.fill_texture_id),
                    egui::Shape::Mesh(mesh) => Some(mesh.texture_id),
                    _ => None,
                })
                .collect();
        }

        /// One frame in which these keys are pressed.
        fn frame(&mut self, keys: &[Key]) {
            self.frame_with(keys.iter().map(|&k| key_press(k)).collect());
        }

        /// One frame per key, each followed by a quiet one: the focus egui moves
        /// on a Tab shows in it.
        fn press(&mut self, keys: &[Key]) {
            for &k in keys {
                self.frame(&[k]);
                self.frame(&[]);
            }
        }

        /// Text typed, as the keyboard makes it.
        fn type_text(&mut self, text: &str) {
            for c in text.chars() {
                self.frame_with(vec![egui::Event::Text(c.to_string())]);
            }
        }

        /// What the app told the player since the last call.
        fn sent(&mut self) -> Vec<Cmd> {
            let mut sent = Vec::new();
            while let Ok(cmd) = self.cmds.try_recv() {
                sent.push(cmd);
            }
            sent
        }

        /// Presses Tab until the keyboard is on the widget with this id: whether
        /// it got there within `limit` presses.
        fn tab_to(&mut self, id: egui::Id, limit: usize) -> bool {
            for _ in 0..limit {
                if self.ctx.memory(|m| m.focused()) == Some(id) {
                    return true;
                }
                self.press(&[Key::Tab]);
            }
            self.ctx.memory(|m| m.focused()) == Some(id)
        }
    }

    fn loaded_catalog() -> Arc<CatalogStore> {
        Arc::new(CatalogStore::from_catalog(idle_portal(), adult_catalog()))
    }

    fn zapped(sent: &[Cmd]) -> bool {
        sent.iter().any(|c| matches!(c, Cmd::LoadUrl(_)))
    }

    #[tokio::test]
    async fn the_settings_overlay_owns_tab_and_enter_even_when_the_favorites_list_was_open() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.app.favorites.toggle(3, "TEST 03");
        h.app.show_favs = true;
        h.frame(&[]);
        // o, Tab, Tab, Enter used to zap to the favorite under the overlay.
        h.press(&[Key::O, Key::Tab, Key::Tab, Key::Enter]);
        assert!(h.app.settings_ui.is_some(), "the overlay is up");
        assert!(!h.app.show_favs, "the favorites list is closed");
        assert!(!zapped(&h.sent()), "nothing was zapped");
        assert!(h.app.favorites.contains(3), "and nothing deleted");
    }

    /// An hour-long programme on `now` ("Journaal 0", since 10 minutes), the two
    /// before it and the two after it. Within three hours of local midnight they
    /// are shorter, so that the ones next to the one on now are still on the
    /// day the guide shows: these tests run at any time of day.
    fn programmes_around(now: chrono::DateTime<chrono::Utc>) -> Epg {
        let (from, to) = crate::guide::time::day_bounds(crate::guide::time::day_of(now));
        let hour = chrono::Duration::hours(1).min((now - from).min(to - now) / 3);
        let entries = (-2..3)
            .map(|k: i32| {
                let start = now + hour * k - hour / 6;
                EpgEntry {
                    title: format!("Journaal {k}"),
                    start,
                    end: start + hour,
                    has_archive: None,
                }
            })
            .collect();
        Epg::new(entries)
    }

    /// The guide open on `h`. With `programme`, TEST 03's catch-up twin (13) has
    /// the programmes of `programmes_around`, fetched now.
    fn open_guide(h: &mut Harness, programme: bool) {
        if programme {
            let now = chrono::Utc::now();
            h.app
                .epg_store
                .insert(13, ChannelEpg::new(programmes_around(now), now));
        }
        h.app.open_guide();
        h.frame(&[]);
        h.frame(&[]);
    }

    /// A click with the primary mouse button at `pos`, one frame per step.
    fn click_at(h: &mut Harness, pos: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.frame_with(vec![egui::Event::PointerMoved(pos)]);
        h.frame_with(vec![button(true)]);
        h.frame_with(vec![button(false)]);
    }

    #[tokio::test]
    async fn the_help_link_closes_the_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, false);
        h.app.show_help = true;
        h.frame(&[]);
        let help_row = egui::Id::new("__help_settings__");
        assert!(
            h.tab_to(help_row, 6),
            "the help's settings row can be reached"
        );
        h.frame(&[Key::Enter]);
        assert!(h.app.settings_ui.is_some(), "the settings opened");
        assert!(!h.app.guide_open, "the guide closed");
    }

    #[tokio::test]
    async fn no_guide_cell_lies_under_the_settings_overlay_for_the_mouse_either() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        h.app.open_settings();
        h.frame(&[]);
        // The overlay is a window in the middle; a guide cell spans the whole width,
        // so a click beside the overlay used to play the programme in it.
        click_at(&mut h, egui::pos2(60.0, 350.0));
        assert!(!zapped(&h.sent()), "no guide programme was played");
        assert!(h.app.settings_ui.is_some(), "the overlay is still up");
    }

    #[test]
    fn opening_the_settings_closes_every_panel_a_key_could_reach() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        let app = &mut h.app;
        app.show_favs = true;
        app.show_search = true;
        app.search_query = "ab".into();
        app.show_epg_grid = true;
        app.guide_open = true;
        app.news_picker = Some(NewsPicker {
            items: Vec::new(),
            selected: 0,
        });
        // These only show something, nothing in them takes a key.
        app.show_epg_strip = true;
        app.show_debug = true;
        app.open_settings();
        assert!(app.settings_ui.is_some());
        assert!(!app.show_favs && !app.show_search && !app.show_epg_grid);
        assert!(!app.guide_open && app.news_picker.is_none());
        assert!(app.search_query.is_empty());
        assert!(app.show_epg_strip && app.show_debug);
    }

    #[tokio::test]
    async fn g_opens_the_guide_and_stays_open_and_the_next_g_closes_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // The press that opens the guide must not close it again in its own frame.
        h.press(&[Key::G]);
        assert!(h.app.guide_open, "g opens the guide");
        h.frame(&[]);
        assert!(h.app.guide_open, "and it stays open");
        h.press(&[Key::G]);
        assert!(!h.app.guide_open, "the next g closes it");
    }

    #[tokio::test]
    async fn enter_in_the_guide_plays_the_programme_on_now_live() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        assert!(h.app.guide_open);
        assert_eq!(h.app.guide_channels.len(), 1, "locked: no adult column");
        h.sent();
        h.press(&[Key::Enter]);
        let sent = h.sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Cmd::LoadUrl(u) if *u == live_url(3))),
            "{sent:?}"
        );
        assert!(!h.app.guide_open, "the guide closes");
        assert_eq!(h.app.current_stream_id, Some(3));
        // Live plays as live, never straight to mpv: through the relay, with the
        // stall watchdog watching.
        assert!(h.app.current_live);
        assert!(h.app.stall_watchdog_armed);
    }

    #[tokio::test]
    async fn after_a_live_pick_in_the_guide_e_shows_the_programme_from_the_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        h.press(&[Key::Enter]);
        assert_eq!(h.app.current_stream_id, Some(3), "it plays live");
        // What `e` and Shift+E show: the store's EPG of the channel, from the
        // programme on now.
        let titles = toast_titles(&h);
        assert_eq!(titles, ["Journaal 0", "Journaal 1", "Journaal 2"]);
        // The store had it, so nothing was asked and the pick's toast stays.
        h.frame(&[]);
        let toast = h.app.toast.as_ref().map(|(t, _, _)| t.as_str());
        assert_eq!(toast, Some("[LIVE] TEST 03 · Journaal 0"));
    }

    #[tokio::test]
    async fn after_a_live_pick_on_a_stale_column_the_portal_is_asked_as_on_a_zap() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let mut h = zap_app(dir.path(), vec![programme_at(now, "Portal", -10, 50)]);
        // TEST 03's column, fetched 7 hours ago: stale, though the guide shows it.
        let old = now - chrono::Duration::hours(7);
        h.app
            .epg_store
            .insert(13, ChannelEpg::new(programmes_around(now), old));
        open_guide(&mut h, false);
        h.press(&[Key::Enter]);
        assert_eq!(h.app.current_stream_id, Some(3), "it plays live");
        wait_until("the portal's answer", || {
            h.frame(&[]);
            h.app.current_epg.is_some()
        })
        .await;
        assert_eq!(toast_titles(&h), ["Portal"], "the portal's answer");
    }

    #[tokio::test]
    async fn a_past_programme_plays_from_its_start_on_the_catch_up_twin() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        // The programme before the one on now, as the EPG holds it, and the URL
        // that plays it from its start for as long as it ran.
        let picked = h
            .app
            .epg_store
            .get(13)
            .and_then(|c| c.entries().iter().find(|e| e.title == "Journaal -1"))
            .cloned()
            .expect("the programme before the one on now");
        let minutes = (picked.end - picked.start).num_minutes().max(1) as u32;
        let from_its_start = h
            .app
            .catalog
            .portal()
            .catchup_url(13, picked.start, minutes);
        h.sent();
        h.press(&[Key::ArrowUp, Key::Enter]);
        let sent = h.sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Cmd::LoadUrl(u) if *u == from_its_start)),
            "{sent:?} should hold {from_its_start}"
        );
        // Catch-up plays as video on demand: not as live, no stall watchdog.
        assert!(!h.app.current_live);
        assert!(!h.app.stall_watchdog_armed);
    }

    #[tokio::test]
    async fn a_programme_still_to_come_plays_nothing_and_says_when_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        h.sent();
        h.press(&[Key::ArrowDown, Key::Enter]);
        assert!(!zapped(&h.sent()));
        let toast = h
            .app
            .toast
            .as_ref()
            .map(|(t, _, _)| t.clone())
            .unwrap_or_default();
        assert!(toast.starts_with("starts at"), "{toast}");
        assert!(h.app.guide_open, "the guide stays");
    }

    #[tokio::test]
    async fn locking_with_the_guide_open_drops_the_adult_column_and_keeps_the_page_valid() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.app.set_adult_visible(true);
        open_guide(&mut h, true);
        assert_eq!(h.app.guide_channels.len(), 2);
        h.press(&[Key::ArrowRight]);
        h.app.set_adult_visible(false);
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.app.guide_channels.len(), 1);
        assert!(
            !h.painted.iter().any(|t| t.contains("ADULT")),
            "{:?}",
            h.painted
        );
        h.sent();
        h.press(&[Key::Enter]);
        assert!(zapped(&h.sent()), "the cursor is back on TEST 03");
    }

    #[tokio::test]
    async fn unlocking_with_the_guide_open_adds_the_adult_column_within_a_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        assert_eq!(h.app.guide_channels.len(), 1, "locked: no adult column");
        assert!(
            !h.painted.iter().any(|t| t.contains("ADULT")),
            "{:?}",
            h.painted
        );
        h.app.set_adult_visible(true);
        h.frame(&[]);
        assert_eq!(h.app.guide_channels.len(), 2, "unlocked: the adult column");
        assert!(
            h.painted.iter().any(|t| t.contains("ADULT")),
            "and it is drawn: {:?}",
            h.painted
        );
    }

    #[tokio::test]
    async fn a_guide_pick_in_an_adult_column_is_adult_whichever_variant_carries_the_flag() {
        // Only 901 flagged: a catch-up pick plays the unflagged twin 911. Only 911
        // flagged: a live pick plays the unflagged 901.
        for live_flagged in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let catalog = flagged_guide_catalog(live_flagged, !live_flagged);
            let mut h = Harness::start(dir.path(), catalog, PortalState::Configured, None);
            h.app.set_adult_visible(true);
            let now = chrono::Utc::now();
            h.app
                .epg_store
                .insert(911, ChannelEpg::new(programmes_around(now), now));
            open_guide(&mut h, false);
            // ADULT A's column: the programme before the one on now, or that one.
            if live_flagged {
                h.press(&[Key::ArrowRight, Key::ArrowUp, Key::Enter]);
                show_first_picture(&mut h);
            } else {
                h.press(&[Key::ArrowRight, Key::Enter]);
            }
            let played = h.app.current_url.clone().unwrap_or_default();
            let id = if live_flagged { "/911.m3u8" } else { "/901.m3u8" };
            assert!(played.ends_with(id), "{played}");
            if !live_flagged {
                // The stall watchdog's reload is the same stream: still adult.
                h.app.load_url(played.clone(), true);
            }
            let adult = read_last_watched(dir.path()).adult;
            assert_eq!(adult, Some(true), "{played} is never resumed");
            h.sent();
            h.app.apply_settings_action(SettingsAction::Lock);
            assert!(stopped(&h.sent()), "Lock stops {played}");
            assert_eq!(h.app.current_url, None);
        }
    }

    #[tokio::test]
    async fn after_an_adult_guide_pick_the_next_load_is_judged_on_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = flagged_guide_catalog(false, true);
        let mut h = Harness::start(dir.path(), catalog, PortalState::Configured, None);
        h.app.set_adult_visible(true);
        let now = chrono::Utc::now();
        h.app
            .epg_store
            .insert(911, ChannelEpg::new(programmes_around(now), now));
        open_guide(&mut h, false);
        h.press(&[Key::ArrowRight, Key::Enter]);
        assert_eq!(h.app.current_url, Some(live_url(901)));
        // A zap to TEST 03 afterwards: an ordinary channel.
        h.app.zap_by_id(3);
        assert_eq!(read_last_watched(dir.path()).adult, Some(false));
        h.sent();
        h.app.apply_settings_action(SettingsAction::Lock);
        assert!(!stopped(&h.sent()), "Lock leaves TEST 03 playing");
    }

    #[tokio::test]
    async fn the_guide_follows_a_catalog_that_f5_fetches_again() {
        let dir = tempfile::tempdir().unwrap();
        let portal = FakePortal::new(vec![archive_channel(601, "NL | NPO1 HD")], Vec::new());
        portal.release_catalog();
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        catalog.spawn_fetch();
        wait_until("the catalog loads", || catalog.is_loaded()).await;
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        assert_eq!(h.app.guide_channels.len(), 1);
        // The portal lists another channel now. F5 fetches the catalog again into
        // the same store, which stays loaded all the while.
        portal.set_live(vec![
            archive_channel(601, "NL | NPO1 HD"),
            archive_channel(602, "NL | NPO2 HD"),
        ]);
        h.press(&[Key::F5]);
        wait_until("the re-fetch lands", || catalog.live_channels().len() == 2).await;
        h.frame(&[]);
        assert_eq!(
            h.app.guide_channels.len(),
            2,
            "the columns follow the new catalog"
        );
        open_guide(&mut h, false);
        assert!(
            h.painted.iter().any(|t| t.contains("NPO2")),
            "and the new column is drawn: {:?}",
            h.painted
        );
    }

    // ---- the wait screen of a catch-up pick ----

    /// The programme before the one on now, picked in the open guide.
    fn pick_catch_up(h: &mut Harness) {
        open_guide(h, true);
        h.press(&[Key::ArrowUp, Key::Enter]);
    }

    /// mpv restarts playback, and then renders the first picture of the
    /// programme: the way a catch-up wait ends. Checks that it does.
    fn show_first_picture(h: &mut Harness) {
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        h.put_frame(4, 2);
        h.frame(&[]);
        assert!(
            h.app.pending_catchup.is_none(),
            "the first picture ends the wait"
        );
    }

    #[tokio::test]
    async fn a_catch_up_pick_shows_the_wait_screen_until_the_first_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // The channel watched before the pick: its last picture is on screen.
        h.put_frame(4, 2);
        h.frame(&[]);
        pick_catch_up(&mut h);
        assert!(h.app.pending_catchup.is_some());
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t == "loading the programme…"),
            "{:?}",
            h.painted
        );
        assert!(h.painted.iter().any(|t| t == "Journaal -1"));
        // The file opening is not the first frame: the stream is still starting.
        h.events.send(Event::FileLoaded).unwrap();
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_some());
        assert!(h.app.stream_starting_since.is_some());
        // Nor is mpv's restart, which comes before it renders anything: the
        // picture on the bus is still the one of the channel before.
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        assert!(
            h.app.stream_starting_since.is_none(),
            "the EPG fetching, held back while the stream started, goes on"
        );
        assert!(
            h.app.pending_catchup.is_some(),
            "the wait goes on after the restart"
        );
        assert!(h.painted.iter().any(|t| t == "loading the programme…"));
        assert!(h.app.video_tex.is_none(), "the old picture is gone");
        // The first picture of the programme ends it, and in that very frame
        // nothing but that picture is drawn.
        h.put_frame(8, 4);
        h.frame(&[]);
        assert!(
            h.app.pending_catchup.is_none(),
            "a new picture ends the wait"
        );
        assert!(
            !h.painted.iter().any(|t| t == "loading the programme…"),
            "{:?}",
            h.painted
        );
        let video = h.app.video_tex.as_ref().expect("the new picture");
        assert_eq!(video.size(), [8, 4], "its texture holds the new picture");
        assert!(h.drawn.contains(&video.id()), "and it is what is drawn");
    }

    #[tokio::test]
    async fn a_picture_that_was_there_when_the_restart_came_does_not_end_the_wait() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        // Rendered before the app took the event in: the stream before the pick, or
        // the new one a moment ahead of its event. Neither is a picture that is new.
        let before = h.put_frame(4, 2);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        assert_eq!(h.app.last_frame_version, before, "it was taken in ...");
        assert!(
            h.app.pending_catchup.is_some(),
            "... and is not newer than the restart"
        );
        h.put_frame(4, 2);
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_none(), "the next one is");
    }

    #[tokio::test]
    async fn the_wait_ends_a_second_after_the_restart_when_no_picture_comes() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_some(), "a picture can still come");
        // The restart's instant moved into the past instead of a sleep.
        let ago = |ms| Instant::now().checked_sub(Duration::from_millis(ms));
        let wait = h.app.pending_catchup.as_mut().unwrap();
        let version = wait.started.expect("the restart is noted").0;
        wait.started = Some((version, ago(700).unwrap()));
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_some(), "not before the second");
        let log = LogCapture::start();
        let wait = h.app.pending_catchup.as_mut().unwrap();
        wait.started = Some((version, ago(1100).unwrap()));
        h.frame(&[]);
        assert!(
            h.app.pending_catchup.is_none(),
            "audio only, or no video: the wait ends anyway"
        );
        let text = log.text();
        assert!(
            text.contains("catch-up: no picture 1 s after the start"),
            "{text}"
        );
        assert!(!text.contains("catch-up: first frame"), "{text}");
    }

    #[tokio::test]
    async fn a_wait_that_ends_without_a_picture_never_shows_the_channel_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // The channel watched before the pick: its last picture is on screen.
        h.put_frame(4, 2);
        h.frame(&[]);
        assert!(h.app.video_tex.is_some(), "a live picture");
        pick_catch_up(&mut h);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        // A second after the restart no picture has come: audio only.
        let ago = Instant::now().checked_sub(Duration::from_millis(1100)).unwrap();
        let wait = h.app.pending_catchup.as_mut().unwrap();
        let version = wait.started.expect("the restart is noted").0;
        wait.started = Some((version, ago));
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_none(), "the cap ends the wait");
        h.frame(&[]);
        assert!(h.app.video_tex.is_none(), "the old picture is gone");
        assert!(
            h.painted.iter().any(|t| t == crate::APP_NAME),
            "the empty state shows: {:?}",
            h.painted
        );
    }

    #[tokio::test]
    async fn esc_between_the_restart_and_the_first_picture_still_brings_the_guide_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        h.sent();
        h.press(&[Key::Escape]);
        let sent = h.sent();
        assert!(sent.iter().any(|c| matches!(c, Cmd::Stop)), "{sent:?}");
        assert!(h.app.pending_catchup.is_none());
        assert!(h.app.guide_open, "back in the guide");
    }

    #[tokio::test]
    async fn a_failure_between_the_restart_and_the_first_picture_shows_on_the_wait_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
        h.frame(&[]);
        let said = "Catch-up failed: the stream stopped";
        assert!(h.painted.iter().any(|t| t == said), "{:?}", h.painted);
        assert!(h.app.toast.is_none(), "and no toast says it");
        // A picture that was on its way, or a second without one, does not wipe
        // what the user has to read.
        h.put_frame(4, 2);
        h.frame(&[]);
        let wait = h.app.pending_catchup.as_mut().expect("the failure stays");
        let version = wait.started.expect("the restart is noted").0;
        wait.started = Some((
            version,
            Instant::now().checked_sub(Duration::from_secs(2)).unwrap(),
        ));
        h.frame(&[]);
        assert!(h.painted.iter().any(|t| t == said), "{:?}", h.painted);
        h.press(&[Key::Escape]);
        assert!(h.app.pending_catchup.is_none());
        assert!(h.app.guide_open, "Esc brings the guide back from it");
    }

    #[tokio::test]
    async fn after_a_cancel_the_next_pick_waits_for_its_own_picture_not_the_splash() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.press(&[Key::Escape]);
        assert!(h.app.video_tex.is_none(), "a stop takes the picture away");
        // A frame mpv rendered while the stop was on its way: the fence keeps it
        // off screen, and it is no picture of the next programme either.
        h.put_frame(4, 2);
        h.press(&[Key::ArrowUp, Key::Enter]);
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_some(), "the wait goes on");
        assert!(h.painted.iter().any(|t| t == "loading the programme…"));
        assert!(
            !h.painted.iter().any(|t| t == crate::APP_NAME),
            "not the splash: {:?}",
            h.painted
        );
        assert!(h.app.video_tex.is_none());
        // Its own first picture.
        h.put_frame(8, 4);
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_none());
        let video = h.app.video_tex.as_ref().expect("its picture");
        assert_eq!(video.size(), [8, 4]);
        assert!(h.drawn.contains(&video.id()));
    }

    #[tokio::test]
    async fn esc_on_the_wait_screen_stops_the_load_and_brings_the_guide_back() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        assert!(h.app.stream_starting_since.is_some(), "the load is on");
        h.sent();
        h.press(&[Key::Escape]);
        let sent = h.sent();
        assert!(sent.iter().any(|c| matches!(c, Cmd::Stop)), "{sent:?}");
        assert!(h.app.pending_catchup.is_none());
        assert!(
            h.app.stream_starting_since.is_none(),
            "the stop ends the load, and the EPG fetching goes on"
        );
        assert!(h.app.guide_open, "back in the guide");
    }

    #[tokio::test]
    async fn the_guide_comes_back_from_the_wait_screen_on_the_programme_picked() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // In Nu & straks, where a plain `g` lines up on now again.
        pick_catch_up(&mut h);
        let picked = h.sent().into_iter().find_map(|c| match c {
            Cmd::LoadUrl(u) => Some(u),
            _ => None,
        });
        let picked = picked.expect("the pick loads");
        h.press(&[Key::Escape]);
        assert!(h.app.guide_open, "back in the guide");
        h.sent();
        h.press(&[Key::Enter]);
        let sent = h.sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Cmd::LoadUrl(u) if *u == picked)),
            "{sent:?} should pick {picked} again"
        );
    }

    #[test]
    fn spending_a_key_takes_its_presses_and_the_letter_it_types_and_nothing_else() {
        let key_event = |key, pressed, modifiers| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let text = |s: &str| egui::Event::Text(s.into());
        let none = egui::Modifiers::NONE;
        let release = key_event(Key::G, false, none);
        let events = vec![
            key_event(Key::G, true, none),
            key_event(Key::G, true, egui::Modifiers::SHIFT),
            text("g"),
            text("G"),
            release.clone(),
            key_event(Key::H, true, none),
            text("h"),
        ];
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            events,
            ..Default::default()
        };
        let mut left = Vec::new();
        let _ = ctx.run(raw, |ctx| {
            spend_key(ctx, Key::G);
            left = ctx.input(|i| i.events.clone());
        });
        // The release, and another key with its letter, stay.
        assert_eq!(left, [release, key_event(Key::H, true, none), text("h")]);
    }

    // The guide that comes back reads its keys in the frame of the key that
    // brought it: a `g` there closes it again, an Esc runs its own chain (the
    // filter, else closing). These press the key as a keyboard makes it.

    #[tokio::test]
    async fn g_on_the_wait_screen_brings_the_guide_back_and_it_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.sent();
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        assert!(
            h.app.guide_open,
            "the guide is back in the frame of the key"
        );
        assert!(h.app.pending_catchup.is_none());
        assert!(h.sent().iter().any(|c| matches!(c, Cmd::Stop)));
        h.frame(&[]);
        assert!(h.app.guide_open, "and it stays");
        h.press(&[Key::G]);
        assert!(!h.app.guide_open, "the next g closes it, as ever");
    }

    #[tokio::test]
    async fn shift_g_on_the_wait_screen_is_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        let shift_g = egui::Event::Key {
            key: Key::G,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        h.frame_with(vec![shift_g, egui::Event::Text("G".into())]);
        assert!(h.app.guide_open, "whatever the modifiers");
        h.frame(&[]);
        assert!(h.app.guide_open, "and it stays");
    }

    #[tokio::test]
    async fn esc_on_the_wait_screen_brings_the_guide_back_and_it_stays() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.frame(&[Key::Escape]);
        assert!(
            h.app.guide_open,
            "the guide is back in the frame of the key"
        );
        assert!(h.app.pending_catchup.is_none());
        h.frame(&[]);
        assert!(h.app.guide_open, "and it stays");
        h.press(&[Key::Escape]);
        assert!(!h.app.guide_open, "the next Esc closes it, as ever");
    }

    #[tokio::test]
    async fn a_stream_that_fails_before_its_first_frame_says_so_on_the_wait_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        // The stream before it stopping is no failure.
        h.events
            .send(Event::EndOfFile {
                reason: "stop".into(),
            })
            .unwrap();
        h.frame(&[]);
        assert!(h.app.pending_catchup.as_ref().unwrap().failed.is_none());
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t.starts_with("Catch-up failed")),
            "{:?}",
            h.painted
        );
        h.press(&[Key::Escape]);
        assert!(h.app.guide_open);
    }

    #[tokio::test]
    async fn an_eof_before_the_first_frame_is_a_failure_too() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.sent();
        h.events
            .send(Event::EndOfFile {
                reason: "eof".into(),
            })
            .unwrap();
        h.frame(&[]);
        h.frame(&[]);
        let said = "Catch-up failed: the stream stopped";
        assert!(h.painted.iter().any(|t| t == said), "{:?}", h.painted);
        assert!(!zapped(&h.sent()), "a catch-up that ended is not reloaded");
        assert!(h.app.toast.is_none(), "and no toast says it");
    }

    #[tokio::test]
    async fn a_player_error_before_the_first_frame_goes_to_the_wait_screen_not_to_a_toast() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        let error = |msg: &str| Event::Error { msg: msg.into() };
        h.events.send(error("boom")).unwrap();
        h.frame(&[]);
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t == "Catch-up failed: boom"),
            "{:?}",
            h.painted
        );
        assert!(h.app.toast.is_none(), "no toast over the wait screen");
        // The first reason stays: what comes after it explains nothing better.
        h.events.send(error("later")).unwrap();
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
        let wait = h.app.pending_catchup.as_ref().unwrap();
        assert_eq!(wait.failed.as_deref(), Some("boom"));
    }

    #[tokio::test]
    async fn after_the_first_frame_the_wait_screen_leaves_errors_to_the_toast() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        show_first_picture(&mut h);
        h.events.send(Event::Error { msg: "boom".into() }).unwrap();
        h.frame(&[]);
        let toast = |h: &Harness| h.app.toast.as_ref().map(|(t, _, _)| t.clone());
        assert_eq!(toast(&h).as_deref(), Some("error: boom"));
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
        let retry = "stream error - zap up or down to retry";
        assert_eq!(toast(&h).as_deref(), Some(retry));
        assert!(h.app.pending_catchup.is_none());
    }

    #[tokio::test]
    async fn the_wait_screen_takes_no_other_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        h.sent();
        h.press(&[Key::ArrowDown, Key::Enter, Key::O]);
        assert!(!zapped(&h.sent()));
        assert!(h.app.settings_ui.is_none());
        assert!(h.app.pending_catchup.is_some());
    }

    #[tokio::test]
    async fn the_wait_screen_logs_what_the_regression_scenarios_look_for() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let log = LogCapture::start();
        // Cancelled, failed, and shown at last.
        pick_catch_up(&mut h);
        h.press(&[Key::Escape]);
        h.press(&[Key::ArrowUp, Key::Enter]);
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.press(&[Key::Escape]);
        h.press(&[Key::ArrowUp, Key::Enter]);
        show_first_picture(&mut h);
        let text = log.text();
        let count = |what: &str| text.lines().filter(|l| l.contains(what)).count();
        assert_eq!(count("catch-up: waiting for the first frame"), 3, "{text}");
        assert_eq!(count("catch-up: cancelled"), 2, "{text}");
        assert_eq!(
            count("catch-up: failed before its first frame (error)"),
            1,
            "{text}"
        );
        assert_eq!(count("catch-up: first frame"), 1, "{text}");
    }

    #[test]
    fn enter_on_the_help_row_opens_the_settings_in_their_list_not_in_a_section() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.frame_with(vec![egui::Event::Text("?".into())]);
        h.frame(&[]);
        assert!(h.app.show_help);
        // The `o` row is the help's one Tab stop.
        h.press(&[Key::Tab]);
        h.frame(&[Key::Enter]);
        assert!(h.app.settings_ui.is_some() && !h.app.show_help);
        // In the list `o` closes the overlay. Had the same Enter opened the Language
        // pane, `o` would be a letter there and the overlay would stay.
        h.press(&[Key::O]);
        assert!(h.app.settings_ui.is_none());
    }

    #[test]
    fn the_first_tab_in_the_setup_prompt_lands_in_the_server_field() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        h.press(&[Key::Tab]);
        h.type_text("host");
        assert_eq!(h.app.cfg_host, "host");
    }

    // ---- a portal that answers from memory ----

    /// Answers from memory. Its catalog arrives only after `release_catalog`,
    /// its day EPGs only after `release_epg`, so a test decides when the app
    /// learns what; `epg_fetches` counts the day-EPG requests.
    struct FakePortal {
        urls: XtreamPortal,
        /// What the catalog lists; `set_live` changes it for the next fetch.
        live: Mutex<Vec<LiveChannel>>,
        day_epg: Vec<EpgEntry>,
        /// What the short EPG answers (the zap toast's own fetch): nothing, unless
        /// `set_short_epg` says otherwise.
        short_epg: Mutex<Vec<EpgEntry>>,
        catalog_gate: tokio::sync::Semaphore,
        epg_gate: tokio::sync::Semaphore,
        epg_fetches: std::sync::atomic::AtomicUsize,
    }

    impl FakePortal {
        fn new(live: Vec<LiveChannel>, day_epg: Vec<EpgEntry>) -> Arc<Self> {
            Arc::new(FakePortal {
                urls: XtreamPortal::new(XtreamCreds {
                    username: "u".into(),
                    password: "p".into(),
                    host: "h".into(),
                    port: 80,
                }),
                live: Mutex::new(live),
                day_epg,
                short_epg: Mutex::new(Vec::new()),
                catalog_gate: tokio::sync::Semaphore::new(0),
                epg_gate: tokio::sync::Semaphore::new(0),
                epg_fetches: std::sync::atomic::AtomicUsize::new(0),
            })
        }

        /// The portal lists these channels from the next catalog fetch on.
        fn set_live(&self, live: Vec<LiveChannel>) {
            *self.live.lock() = live;
        }

        /// The portal answers these programmes to every short EPG.
        fn set_short_epg(&self, programmes: Vec<EpgEntry>) {
            *self.short_epg.lock() = programmes;
        }

        fn release_catalog(&self) {
            self.catalog_gate.add_permits(1 << 20);
        }

        fn release_epg(&self) {
            self.epg_gate.add_permits(1 << 20);
        }

        fn epg_fetches(&self) -> usize {
            self.epg_fetches.load(std::sync::atomic::Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl Portal for FakePortal {
        async fn fetch_catalog(
            &self,
        ) -> Result<crate::portal::Catalog, crate::portal::PortalError> {
            let _wait = self.catalog_gate.acquire().await;
            Ok(crate::portal::Catalog {
                live: self.live.lock().clone(),
                ..Default::default()
            })
        }
        async fn fetch_epg(&self, _: i64) -> Result<Epg, crate::portal::PortalError> {
            Ok(Epg::new(self.short_epg.lock().clone()))
        }
        async fn fetch_day_epg(&self, _: i64) -> Result<Epg, crate::portal::PortalError> {
            self.epg_fetches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let _wait = self.epg_gate.acquire().await;
            Ok(Epg::new(self.day_epg.clone()))
        }
        async fn fetch_series_episodes(
            &self,
            _: i64,
        ) -> Result<Vec<crate::portal::Episode>, crate::portal::PortalError> {
            Ok(Vec::new())
        }
        fn live_stream_url(&self, id: i64) -> String {
            self.urls.live_stream_url(id)
        }
        fn live_stream_id(&self, url: &str) -> Option<i64> {
            self.urls.live_stream_id(url)
        }
        fn movie_stream_url(&self, id: i64, ext: &str) -> String {
            self.urls.movie_stream_url(id, ext)
        }
        fn series_stream_url(&self, id: &str, ext: &str) -> String {
            self.urls.series_stream_url(id, ext)
        }
        fn catchup_url(&self, id: i64, start: chrono::DateTime<chrono::Utc>, min: u32) -> String {
            self.urls.catchup_url(id, start, min)
        }
    }

    /// Waits (for 2 s at most) until `done` holds: the tests wait for tasks that
    /// run in the background.
    async fn wait_until(what: &str, mut done: impl FnMut() -> bool) {
        for _ in 0..400 {
            if done() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("timed out waiting for {what}");
    }

    fn live_url(id: i64) -> String {
        format!("http://h:80/live/u/p/{id}.m3u8")
    }

    fn write_last_watched(root: &std::path::Path, url: &str, live: bool, adult: Option<bool>) {
        let record = LastWatched {
            url: url.into(),
            live,
            adult,
        };
        record.save(&Storage::with_root(root.to_path_buf()).last_watched_path());
    }

    fn read_last_watched(root: &std::path::Path) -> LastWatched {
        LastWatched::load(&Storage::with_root(root.to_path_buf()).last_watched_path())
            .expect("last_watched.json was written")
    }

    // ---- the favorites wait for the catalog ----

    #[test]
    fn the_favorites_list_shows_nothing_until_the_catalog_has_loaded() {
        let mut favorites = Favorites::default();
        favorites.toggle(3, "TEST 03");
        favorites.toggle(901, "Adult A"); // flagged, whatever the name says
        let unloaded = CatalogStore::new(idle_portal());
        // Until then nothing says which favorites are adult: none shows.
        assert!(shown_favorites(&favorites, &unloaded).is_empty());
        // Loaded: the normal ones, and the flagged one only when unlocked.
        let loaded = CatalogStore::from_catalog(idle_portal(), adult_catalog());
        let ids = |catalog: &CatalogStore| {
            let shown = shown_favorites(&favorites, catalog);
            shown.iter().map(|e| e.stream_id).collect::<Vec<_>>()
        };
        assert_eq!(ids(&loaded), [3]);
        loaded.set_adult_unlocked(true);
        assert_eq!(ids(&loaded), [3, 901]);
    }

    #[tokio::test]
    async fn the_favorites_panel_says_the_catalog_is_loading_and_names_nothing_until_it_has() {
        let dir = tempfile::tempdir().unwrap();
        let portal = FakePortal::new(adult_catalog().live, Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        h.app.favorites.toggle(3, "TEST 03");
        h.app.favorites.toggle(901, "Adult A");
        h.app.show_favs = true;
        // An area shows from its second frame on.
        h.frame(&[]);
        h.frame(&[]);
        let says = |h: &Harness, text: &str| h.painted.iter().any(|p| p == text);
        assert!(says(&h, "loading catalog..."), "{:?}", h.painted);
        assert!(
            !says(&h, "TEST 03") && !says(&h, "Adult A"),
            "{:?}",
            h.painted
        );
        catalog.spawn_fetch();
        portal.release_catalog();
        wait_until("the catalog", || catalog.is_loaded()).await;
        h.frame(&[]);
        h.frame(&[]);
        assert!(says(&h, "TEST 03"), "{:?}", h.painted);
        assert!(!says(&h, "Adult A") && !says(&h, "loading catalog..."));
    }

    // ---- the catalog's first word on what was resumed ----

    #[test]
    fn a_resume_that_skips_on_the_catalogs_word_teaches_the_file() {
        let lw = |adult| LastWatched {
            url: live_url(901),
            live: true,
            adult,
        };
        let learned = |adult, says| skipped_record(&lw(adult), says).map(|r| r.adult);
        // The catalog calls it adult and the file did not know: the file learns it.
        assert_eq!(learned(Some(false), Some(true)), Some(Some(true)));
        assert_eq!(learned(None, Some(true)), Some(Some(true)));
        // Nothing to write when the file said so already, or the catalog did not.
        assert_eq!(learned(Some(true), Some(true)), None);
        assert_eq!(learned(Some(true), None), None);
        assert_eq!(learned(Some(false), Some(false)), None);
        assert_eq!(learned(None, None), None);
        // The rest of the record stays as it was.
        let record = skipped_record(&lw(Some(false)), Some(true)).unwrap();
        assert_eq!(
            (record.url.as_str(), record.live),
            (live_url(901).as_str(), true)
        );
    }

    #[test]
    fn only_a_resumed_adult_stream_on_a_locked_app_is_stopped_by_the_catalogs_word() {
        // (from the resume, adult, unlocked) -> stop?
        let cases = [
            (true, true, false, true),
            // A URL from the command line, or a pick of the user: left playing.
            (false, true, false, false),
            // The PIN opened adult content for this session.
            (true, true, true, false),
            // Not adult: nothing to stop.
            (true, false, false, false),
            (false, false, false, false),
            (true, false, true, false),
            (false, true, true, false),
        ];
        for (from_resume, adult, unlocked, stop) in cases {
            assert_eq!(
                stop_resumed_adult(from_resume, adult, unlocked),
                stop,
                "resumed {from_resume}, adult {adult}, unlocked {unlocked}"
            );
        }
    }

    /// An app started on a resume of channel 901 (flagged in the catalog) from a
    /// last_watched.json that says `file_says`, with its catalog still to come:
    /// the catalog, the portal that releases it, and the app.
    fn resumed_901(
        root: &std::path::Path,
        file_says: Option<bool>,
    ) -> (Arc<CatalogStore>, Arc<FakePortal>, Harness) {
        write_last_watched(root, &live_url(901), true, file_says);
        let portal = FakePortal::new(adult_catalog().live, Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(root, catalog.clone(), PortalState::Configured, None);
        assert!(
            zapped(&h.sent()),
            "the resume plays: the file says {file_says:?}"
        );
        (catalog, portal, h)
    }

    /// The catalog arrives and the app draws a frame in which it notices.
    async fn load_catalog(catalog: &Arc<CatalogStore>, portal: &FakePortal, h: &mut Harness) {
        catalog.spawn_fetch();
        portal.release_catalog();
        wait_until("the catalog", || catalog.is_loaded()).await;
        h.frame(&[]);
    }

    fn stopped(sent: &[Cmd]) -> bool {
        sent.iter().any(|c| matches!(c, Cmd::Stop))
    }

    #[tokio::test]
    async fn a_resumed_stream_the_catalog_calls_adult_stops_and_the_next_start_skips_it() {
        let dir = tempfile::tempdir().unwrap();
        // The file, from an earlier session, says "not adult": the portal has
        // flagged the channel since.
        let (catalog, portal, mut h) = resumed_901(dir.path(), Some(false));
        load_catalog(&catalog, &portal, &mut h).await;
        // Stopped as Lock stops adult playback, with its toast ...
        assert!(stopped(&h.sent()));
        assert_eq!(h.app.current_url, None);
        assert_eq!(h.app.toast.as_ref().map(|t| t.0.as_str()), Some("Locked"));
        // ... and the file knows now, so the next start does not resume it.
        assert_eq!(read_last_watched(dir.path()).adult, Some(true));
        let next_catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut next = Harness::start(dir.path(), next_catalog, PortalState::Configured, None);
        assert!(!zapped(&next.sent()));
        assert_eq!(next.app.current_url, None);
    }

    #[tokio::test]
    async fn a_stale_file_that_says_nothing_either_way_is_healed_and_stopped_too() {
        let dir = tempfile::tempdir().unwrap();
        // A 0.5 file with no flag waits for the catalog and then skips; nothing plays.
        write_last_watched(dir.path(), &live_url(901), true, None);
        let portal = FakePortal::new(adult_catalog().live, Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        assert!(!zapped(&h.sent()));
        load_catalog(&catalog, &portal, &mut h).await;
        h.frame(&[]);
        assert!(!zapped(&h.sent()), "still not resumed");
        assert_eq!(read_last_watched(dir.path()).adult, Some(true));
    }

    #[tokio::test]
    async fn a_url_from_the_command_line_keeps_playing_and_its_flag_is_healed() {
        let dir = tempfile::tempdir().unwrap();
        let portal = FakePortal::new(adult_catalog().live, Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let url = live_url(901);
        let mut h = Harness::start(
            dir.path(),
            catalog.clone(),
            PortalState::Configured,
            Some(&url),
        );
        // Before the catalog the file can only say "unknown".
        assert_eq!(read_last_watched(dir.path()).adult, None);
        load_catalog(&catalog, &portal, &mut h).await;
        assert!(!stopped(&h.sent()), "the user's own URL keeps playing");
        assert_eq!(h.app.current_url.as_deref(), Some(url.as_str()));
        // But the next plain start must not resume it without the PIN.
        assert_eq!(read_last_watched(dir.path()).adult, Some(true));
    }

    #[tokio::test]
    async fn a_resumed_stream_keeps_playing_when_the_app_is_unlocked_by_then() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, portal, mut h) = resumed_901(dir.path(), Some(false));
        catalog.set_adult_unlocked(true);
        load_catalog(&catalog, &portal, &mut h).await;
        assert!(!stopped(&h.sent()));
        assert!(h.app.current_url.is_some());
        assert_eq!(read_last_watched(dir.path()).adult, Some(true));
    }

    #[tokio::test]
    async fn a_pick_since_the_start_is_left_playing() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, portal, mut h) = resumed_901(dir.path(), Some(false));
        // The user picked another stream before the catalog came in.
        h.app.load_url(live_url(902), true);
        h.sent();
        load_catalog(&catalog, &portal, &mut h).await;
        assert!(!stopped(&h.sent()));
        assert_eq!(h.app.current_url, Some(live_url(902)));
    }

    #[tokio::test]
    async fn a_reload_of_the_resumed_stream_is_still_the_resumed_stream() {
        let dir = tempfile::tempdir().unwrap();
        let (catalog, portal, mut h) = resumed_901(dir.path(), Some(false));
        // The stall watchdog (or an eof) loads the same stream again.
        h.app.load_url(live_url(901), true);
        h.sent();
        load_catalog(&catalog, &portal, &mut h).await;
        assert!(stopped(&h.sent()));
    }

    #[tokio::test]
    async fn a_resume_that_skips_on_the_loaded_catalogs_word_writes_that_into_the_file() {
        let dir = tempfile::tempdir().unwrap();
        write_last_watched(dir.path(), &live_url(901), true, Some(false));
        let loaded = Arc::new(CatalogStore::from_catalog(idle_portal(), adult_catalog()));
        let mut h = Harness::start(dir.path(), loaded, PortalState::Configured, None);
        assert!(!zapped(&h.sent()));
        assert_eq!(read_last_watched(dir.path()).adult, Some(true));
    }

    #[tokio::test]
    async fn a_normal_resume_is_not_touched_by_the_catalogs_word() {
        let dir = tempfile::tempdir().unwrap();
        write_last_watched(dir.path(), &live_url(3), true, Some(false));
        let portal = FakePortal::new(adult_catalog().live, Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        assert!(zapped(&h.sent()));
        load_catalog(&catalog, &portal, &mut h).await;
        assert!(!stopped(&h.sent()));
        assert_eq!(h.app.current_url, Some(live_url(3)));
        assert_eq!(read_last_watched(dir.path()).adult, Some(false));
    }

    // ---- a portal save forgets the old portal ----

    /// Tests that point `BONGO_INI` somewhere hold this: the variable is the
    /// whole process's.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[tokio::test]
    async fn a_portal_save_forgets_what_waited_for_or_came_from_the_old_portal() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        // A 0.5 resume still waiting for the catalog; a stream of the old portal
        // that came from a resume; the old portal's id of the channel it plays.
        h.app.pending_resume = Some(LastWatched {
            url: live_url(3),
            live: true,
            adult: None,
        });
        h.app.current_stream_id = Some(3);
        h.app.playing_resumed = true;
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        };
        let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
        std::env::remove_var("BONGO_INI");
        assert!(saved.is_ok());
        assert!(
            h.app.pending_resume.is_none(),
            "the old portal's URL must not play"
        );
        assert_eq!(h.app.current_stream_id, None);
        assert!(!h.app.playing_resumed);
    }

    #[tokio::test]
    async fn a_portal_save_empties_the_guide_of_the_old_portals_epg() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        assert_eq!(h.app.epg_store.len(), 1);
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        };
        let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
        std::env::remove_var("BONGO_INI");
        assert!(saved.is_ok());
        assert!(
            h.app.epg_store.is_empty(),
            "the old portal's ids mean other channels now"
        );
        assert!(h.app.epg_fetcher.in_flight().is_empty());
    }

    // ---- the guide's EPG fetching ----

    /// An app on a catalog of three channels with catch-up, whose portal holds
    /// every day EPG back: a fetch that starts stays in flight.
    fn epg_app(root: &std::path::Path) -> Harness {
        let live = vec![
            archive_channel(601, "NL | NPO1 HD"),
            archive_channel(602, "NL | NPO2 HD"),
            archive_channel(603, "NL | NPO3 HD"),
        ];
        let portal = FakePortal::new(live.clone(), Vec::new());
        let catalog = CatalogStore::from_catalog(
            portal,
            crate::portal::Catalog {
                live,
                ..Default::default()
            },
        );
        Harness::start(root, Arc::new(catalog), PortalState::Configured, None)
    }

    #[tokio::test]
    async fn the_guide_fetches_two_epgs_at_a_time_while_open_and_one_while_closed() {
        for open in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = epg_app(dir.path());
            h.app.open_guide();
            if !open {
                h.app.close_guide();
            }
            h.frame(&[]);
            let at_once = h.app.epg_fetcher.in_flight().len();
            assert_eq!(at_once, if open { 2 } else { 1 }, "guide open: {open}");
        }
    }

    #[tokio::test]
    async fn epg_fetching_waits_while_a_stream_starts_ten_seconds_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = epg_app(dir.path());
        h.app.open_guide();
        h.app.close_guide();
        // A stream goes to the player: zapping first, the EPG waits.
        h.app.load_url(live_url(601), true);
        assert!(h.app.stream_starting_since.is_some());
        h.frame(&[]);
        h.frame(&[]);
        assert!(h.app.epg_fetcher.in_flight().is_empty());
        // Not for ever: a stream without a frame for 10 s no longer holds it up.
        h.app.stream_starting_since = Instant::now().checked_sub(Duration::from_secs(11));
        h.frame(&[]);
        assert_eq!(h.app.epg_fetcher.in_flight().len(), 1);
        // A stop ends the wait too.
        h.app.stop_playback();
        assert!(h.app.stream_starting_since.is_none());
    }

    #[tokio::test]
    async fn a_locked_guide_fetches_no_adult_epg() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, false);
        let fetching: Vec<i64> = h.app.epg_fetcher.in_flight().iter().copied().collect();
        assert_eq!(fetching, [13], "TEST 03's catch-up twin, not the adult 911");
    }

    /// A log sink the test can read back.
    #[derive(Clone, Default)]
    struct LogBuffer(Arc<Mutex<Vec<u8>>>);

    impl std::io::Write for LogBuffer {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.0.lock().extend_from_slice(bytes);
            Ok(bytes.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl LogBuffer {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock()).into_owned()
        }
    }

    /// What this thread logs from `start` until the capture is dropped: the one
    /// way the tests read the log back. Lines of other threads are not captured,
    /// the subscriber being this thread's.
    ///
    /// The second dispatcher is there for the tests that run beside this one.
    /// With a single dispatcher registered, tracing works a log line's interest
    /// out from whoever hits the line first, and another test's thread, which
    /// has no subscriber, would silence the line for this one for good.
    struct LogCapture {
        buffer: LogBuffer,
        _second: tracing::Dispatch,
        _logging: tracing::subscriber::DefaultGuard,
    }

    impl LogCapture {
        fn start() -> Self {
            let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            let buffer = LogBuffer::default();
            let sink = buffer.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .finish();
            Self {
                buffer,
                _second: second,
                _logging: tracing::subscriber::set_default(subscriber),
            }
        }

        fn text(&self) -> String {
            self.buffer.text()
        }
    }

    #[tokio::test]
    async fn the_epg_store_is_logged_once_per_round_not_once_per_fetch() {
        let dir = tempfile::tempdir().unwrap();
        let live: Vec<LiveChannel> = (601..=604)
            .map(|id| archive_channel(id, &format!("NL | NPO{} HD", id - 600)))
            .collect();
        let portal = FakePortal::new(live.clone(), Vec::new());
        portal.release_epg();
        let catalog = crate::portal::Catalog {
            live,
            ..Default::default()
        };
        let catalog = Arc::new(CatalogStore::from_catalog(portal, catalog));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Configured, None);
        let log = LogCapture::start();
        // The guide was opened and is closed again: one fetch at a time, four
        // fetches in a row.
        h.app.open_guide();
        h.app.close_guide();
        wait_until("every channel has its EPG", || {
            h.frame(&[]);
            h.app.epg_store.len() == 4 && h.app.epg_fetcher.in_flight().is_empty()
        })
        .await;
        let text = log.text();
        let lines = text.lines().filter(|l| l.contains("guide: epg store"));
        assert_eq!(lines.count(), 1, "one line for the round:\n{text}");
    }

    /// An app on a catalog of ten channels with catch-up (ids 601 to 610) whose
    /// guide is open and filtered on "zebra", which is in the names of the third
    /// and the ninth. No EPG fetch has started yet: the test lets it start.
    fn zebra_app(root: &std::path::Path) -> Harness {
        let names = [
            "ALFA",
            "BRAVO",
            "CHARLIE ZEBRA",
            "DELTA",
            "ECHO",
            "FOXTROT",
            "GOLF",
            "HOTEL",
            "INDIA ZEBRA",
            "JULIET",
        ];
        let live: Vec<LiveChannel> = (601..)
            .zip(names)
            .map(|(id, name)| archive_channel(id, &format!("NL | {name}")))
            .collect();
        let portal = FakePortal::new(live.clone(), Vec::new());
        let catalog = crate::portal::Catalog {
            live,
            ..Default::default()
        };
        let catalog = Arc::new(CatalogStore::from_catalog(portal, catalog));
        let mut h = Harness::start(root, catalog, PortalState::Configured, None);
        h.app.refresh_guide_channels();
        h.app.guide_open = true;
        h.frame(&[]);
        h.press(&[Key::F]);
        h.type_text("zebra");
        h.frame(&[]);
        assert_eq!(h.app.guide_view.visible_columns(), [2, 8], "the two shown");
        assert!(h.app.epg_fetcher.in_flight().is_empty());
        h
    }

    fn fetching_now(h: &Harness) -> Vec<i64> {
        let mut ids: Vec<i64> = h.app.epg_fetcher.in_flight().iter().copied().collect();
        ids.sort_unstable();
        ids
    }

    #[tokio::test]
    async fn under_a_filter_the_epg_of_the_columns_on_screen_is_fetched_first() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = zebra_app(dir.path());
        // The fetching starts: the two columns on screen go first, not the
        // channel beside the leftmost one.
        h.app.guide_fetching = true;
        h.frame(&[]);
        assert_eq!(fetching_now(&h), [603, 609]);
    }

    #[tokio::test]
    async fn a_guide_list_that_shrank_since_the_last_frame_does_not_break_the_fetch_order() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = zebra_app(dir.path());
        // The lock took channels out of the list after the last frame drew.
        h.app.guide_channels.truncate(4);
        h.app.guide_fetching = true;
        h.frame(&[]);
        assert_eq!(
            fetching_now(&h),
            [603, 604],
            "the one left on screen, then on"
        );
    }

    #[tokio::test]
    async fn a_portal_save_drops_the_fetches_and_columns_of_the_old_portal() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = epg_app(dir.path());
        h.app.open_guide();
        h.frame(&[]);
        assert_eq!(h.app.epg_fetcher.in_flight().len(), 2);
        assert_eq!(h.app.guide_channels.len(), 3);
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        };
        let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
        std::env::remove_var("BONGO_INI");
        assert!(saved.is_ok());
        assert!(h.app.epg_fetcher.in_flight().is_empty());
        assert!(
            !h.app.guide_fetching,
            "no fetching until the guide opens again"
        );
        h.frame(&[]);
        assert!(
            h.app.guide_channels.is_empty(),
            "the new catalog has not loaded"
        );
    }

    // ---- the guide's EPG cache file, and the zap toast from the store ----

    /// EPG from now for 30 hours: fresh in the store.
    fn fresh_epg(title: &str) -> Epg {
        let now = chrono::Utc::now();
        Epg::new(vec![EpgEntry {
            title: title.into(),
            start: now - chrono::Duration::minutes(5),
            end: now + chrono::Duration::hours(30),
            has_archive: None,
        }])
    }

    #[tokio::test]
    async fn the_epg_cache_file_leaves_adult_channels_out_and_the_next_start_reads_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.app.set_adult_visible(true);
        h.frame(&[]);
        let now = chrono::Utc::now();
        h.app
            .epg_store
            .insert(13, ChannelEpg::new(fresh_epg("Nu"), now));
        h.app
            .epg_store
            .insert(911, ChannelEpg::new(fresh_epg("Adult"), now));
        let (path, channels) = h.app.epg_cache_contents().unwrap();
        assert_eq!(path, dir.path().join("cache").join("epg-h_80.json"));
        write_epg_cache(&path, &channels);
        let ids: Vec<i64> = crate::guide::cache::load(&path, chrono::Utc::now())
            .unwrap()
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert_eq!(ids, [13], "adult EPG never goes to disk");
        drop(h);
        let mut next = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        wait_until("the cache file is read", || {
            next.frame(&[]);
            next.app.epg_store.get(13).is_some()
        })
        .await;
    }

    #[tokio::test]
    async fn the_cache_file_is_written_when_a_fetch_round_is_done() {
        let dir = tempfile::tempdir().unwrap();
        let live = vec![
            LiveChannel {
                stream_id: 3,
                name: "TEST 03".into(),
                category_id: None,
                epg_channel_id: None,
                tv_archive: 0,
                tv_archive_duration: 0,
                is_adult: false,
                stream_icon: None,
            },
            archive_channel(13, "TEST 03 HD"),
        ];
        let portal = FakePortal::new(live, fresh_epg("Nu").entries().to_vec());
        portal.release_catalog();
        portal.release_epg();
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        catalog.spawn_fetch();
        wait_until("the catalog loads", || catalog.is_loaded()).await;
        let mut h = Harness::start(dir.path(), catalog, PortalState::Configured, None);
        h.app.open_guide();
        let path = dir.path().join("cache").join("epg-h_80.json");
        wait_until("the cache file is written", || {
            h.frame(&[]);
            path.exists()
        })
        .await;
        assert_eq!(portal.epg_fetches(), 1);
    }

    #[tokio::test]
    async fn a_zap_takes_now_and_next_from_the_guides_store_when_it_is_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.frame(&[]);
        h.app.epg_store.insert(
            13,
            ChannelEpg::new(fresh_epg("Journaal"), chrono::Utc::now()),
        );
        h.app.zap_by_id(3);
        h.frame(&[]);
        let title = h
            .app
            .current_epg
            .as_ref()
            .and_then(|e| e.entries().first().map(|x| x.title.clone()));
        assert_eq!(title.as_deref(), Some("Journaal"), "no portal asked");
    }

    /// Where the tests' portal ("h", port 80) keeps its cache file.
    fn cache_path(root: &std::path::Path) -> std::path::PathBuf {
        root.join("cache").join("epg-h_80.json")
    }

    /// The channel ids a cache file holds, smallest first.
    fn cached_ids(path: &std::path::Path) -> Vec<i64> {
        let mut ids: Vec<i64> = crate::guide::cache::load(path, chrono::Utc::now())
            .expect("the cache file reads")
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        ids.sort_unstable();
        ids
    }

    // R15.1: the check for a complete round is a walk over the guide's channels.

    #[tokio::test]
    async fn the_cache_write_looks_through_the_channels_once_a_second_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // EPG came in, but the guide's one channel has none in the store: the
        // round is not complete.
        let t0 = Instant::now();
        h.app.epg_cache_dirty_since = Some(t0);
        h.app.write_epg_cache_when_due(t0);
        assert!(h.app.epg_cache_dirty_since.is_some(), "not complete: waits");
        // Its EPG lands. Within a second of the look that found it missing the
        // channels are not looked through again ...
        let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        h.app
            .write_epg_cache_when_due(t0 + Duration::from_millis(900));
        assert!(
            h.app.epg_cache_dirty_since.is_some(),
            "no new walk within a second"
        );
        // ... a second after it they are: complete, and the write starts.
        h.app.write_epg_cache_when_due(t0 + Duration::from_secs(1));
        assert!(h.app.epg_cache_dirty_since.is_none(), "complete: written");
        assert!(h.app.epg_cache_writer.is_some(), "on a thread of its own");
        let path = cache_path(dir.path());
        wait_until("the cache file is written", || path.exists()).await;
        assert_eq!(cached_ids(&path), [13]);
    }

    #[tokio::test]
    async fn the_cache_write_waits_for_a_complete_round_but_never_longer_than_a_minute() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // EPG came in, but not for the guide's one channel: a pause of the
        // fetching, not the end of the round.
        let t0 = Instant::now();
        h.app.epg_cache_dirty_since = Some(t0);
        h.app
            .write_epg_cache_when_due(t0 + Duration::from_millis(59_900));
        assert!(h.app.epg_cache_dirty_since.is_some(), "waits for the round");
        // The minute needs no look at the channels, so it is exact: it comes
        // 100 ms after the look above, well within the second between looks.
        h.app.write_epg_cache_when_due(t0 + Duration::from_secs(60));
        assert!(h.app.epg_cache_dirty_since.is_none(), "a minute is enough");
        let path = cache_path(dir.path());
        wait_until("the cache file is written", || path.exists()).await;
    }

    #[tokio::test]
    async fn the_cache_write_waits_while_a_fetch_is_in_flight() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = epg_app(dir.path());
        h.app.open_guide();
        h.frame(&[]);
        assert_eq!(h.app.epg_fetcher.in_flight().len(), 2);
        // Every channel is fresh in the store, yet two fetches have not landed:
        // what they bring is not in the file yet.
        let now = chrono::Utc::now();
        for id in [601, 602, 603] {
            h.app
                .epg_store
                .insert(id, ChannelEpg::new(fresh_epg("Nu"), now));
        }
        let t0 = Instant::now();
        h.app.epg_cache_dirty_since = Some(t0);
        h.app.write_epg_cache_when_due(t0 + Duration::from_secs(30));
        assert!(
            h.app.epg_cache_dirty_since.is_some(),
            "a fetch is on its way"
        );
    }

    // R15.2: one writer at a time.

    #[tokio::test]
    async fn a_due_cache_write_waits_while_the_writer_runs() {
        // Due because the round is complete, or because EPG has been coming in
        // for a minute.
        for (complete, minutes) in [(true, 0), (false, 1)] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
            if complete {
                let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
                h.app.epg_store.insert(13, fresh);
            }
            // A writer that is still at work until the test lets it go.
            let (release, held) = std::sync::mpsc::channel::<()>();
            h.app.epg_cache_writer = Some(std::thread::spawn(move || {
                let _ = held.recv();
            }));
            let t0 = Instant::now();
            let due = t0 + Duration::from_secs(60 * minutes);
            h.app.epg_cache_dirty_since = Some(t0);
            h.app.write_epg_cache_when_due(due);
            let path = cache_path(dir.path());
            assert!(
                h.app.epg_cache_dirty_since.is_some(),
                "complete {complete}: the write waits, the store stays dirty"
            );
            assert!(!path.exists());
            // The writer is done: the next frame writes.
            release.send(()).unwrap();
            wait_until("the writer is done", || {
                h.app
                    .epg_cache_writer
                    .as_ref()
                    .is_some_and(|w| w.is_finished())
            })
            .await;
            h.app.write_epg_cache_when_due(due);
            assert!(
                h.app.epg_cache_dirty_since.is_none(),
                "complete {complete}: written once the writer is free"
            );
            wait_until("the cache file is written", || path.exists()).await;
        }
    }

    #[tokio::test]
    async fn exit_waits_for_a_running_cache_writer_before_its_own_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        h.app.epg_cache_dirty_since = Some(Instant::now());
        // A slow writer: it writes its file once it is let go. Had exit not
        // waited for it, that file would come after exit's own.
        let (release, held) = std::sync::mpsc::channel::<()>();
        let (finished, wait_for_it) = std::sync::mpsc::channel::<()>();
        let slow = path.clone();
        h.app.epg_cache_writer = Some(std::thread::spawn(move || {
            let _ = held.recv();
            std::fs::create_dir_all(slow.parent().unwrap()).unwrap();
            std::fs::write(&slow, b"not exit's file").unwrap();
            let _ = finished.send(());
        }));
        // It is let go while exit waits (or after exit, if it does not wait).
        let letting_go = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(200));
            let _ = release.send(());
        });
        drop(h);
        wait_for_it
            .recv_timeout(Duration::from_secs(5))
            .expect("the slow writer finished");
        letting_go.join().unwrap();
        assert_eq!(cached_ids(&path), [13], "exit's own write came last");
    }

    // The rest of the cache's life: reading, writing at exit, a portal save.

    #[tokio::test]
    async fn the_cache_read_keeps_what_the_guide_lists_and_leaves_the_store_clean() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let held = |id: i64| crate::guide::cache::ToSave {
            id,
            epg: ChannelEpg::new(fresh_epg("Nu"), now),
            archive_days: 7,
        };
        // 999 is a channel the portal does not list (any more).
        let path = cache_path(dir.path());
        crate::guide::cache::save(&path, &[held(13), held(999)], now).unwrap();
        let log = LogCapture::start();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        wait_until("the cache file is read", || {
            h.frame(&[]);
            h.app.epg_store.get(13).is_some()
        })
        .await;
        // Once per catalog, not once per frame.
        for _ in 0..20 {
            h.frame(&[]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        let text = log.text();
        let reads = text
            .lines()
            .filter(|l| l.contains("epg cache read ("))
            .count();
        assert_eq!(reads, 1, "{text}");
        assert!(
            text.contains("epg cache read (1 channels, 1 programmes)"),
            "{text}"
        );
        assert_eq!(h.app.epg_store.len(), 1, "only the guide's channels");
        assert!(h.app.epg_store.get(999).is_none());
        // What the file holds is no news to it. A store made dirty by the read
        // would be complete, so the same frame would write it back at once: and
        // a file written from the store has 13 only, not 999.
        assert!(h.app.epg_cache_dirty_since.is_none());
        assert_eq!(cached_ids(&path), [13, 999], "not written back");
    }

    #[tokio::test]
    async fn a_cache_read_for_an_older_catalog_is_not_taken_in() {
        let dir = tempfile::tempdir().unwrap();
        let portal = FakePortal::new(vec![archive_channel(13, "TEST 03 HD")], Vec::new());
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        // The portal changed (a new catalog generation) while a read that began
        // for the old one was on its way: its result lands in the slot.
        let old = vec![(13, ChannelEpg::new(fresh_epg("Oud"), chrono::Utc::now()))];
        *h.app.epg_cache_slot.lock() = Some((h.app.catalog_generation, Ok(old)));
        h.app.catalog_generation += 1;
        h.app.epg_cache_read_for = Some(h.app.catalog_generation);
        // The new catalog's own read is on its way.
        h.app.epg_cache_reading_since = Some(Instant::now());
        load_catalog(&catalog, &portal, &mut h).await;
        h.frame(&[]);
        assert!(
            h.app.epg_cache_reading_since.is_some(),
            "a read of the old catalog does not end the wait for this one's"
        );
        assert_eq!(
            h.app.guide_channels.len(),
            1,
            "13 is a column of the new catalog"
        );
        assert!(
            h.app.epg_store.get(13).is_none(),
            "the old portal's ids mean other channels"
        );
        assert!(h.app.epg_cache_slot.lock().is_none());
    }

    #[tokio::test]
    async fn an_unreadable_cache_file_is_logged_and_replaced_by_the_next_write() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let log = LogCapture::start();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        wait_until("the broken file is noticed", || {
            h.frame(&[]);
            log.text().contains("guide: epg cache unreadable (")
        })
        .await;
        let text = log.text();
        assert!(text.contains(") - building a new one"), "{text}");
        assert!(h.app.epg_store.is_empty());
        // The EPG that comes in afterwards is written over it.
        let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        h.app.epg_cache_dirty_since = Some(Instant::now());
        h.frame(&[]);
        wait_until("the file is made anew", || {
            crate::guide::cache::load(&path, chrono::Utc::now()).is_ok()
        })
        .await;
        assert_eq!(cached_ids(&path), [13]);
    }

    #[tokio::test]
    async fn the_cache_logs_what_the_regression_scenarios_look_for() {
        let dir = tempfile::tempdir().unwrap();
        let log = LogCapture::start();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let fresh = ChannelEpg::new(fresh_epg("Journaal"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        // A zap takes the channel's EPG from the store.
        h.app.zap_by_id(3);
        // The file is written. The writer's thread logs this line where this
        // thread's subscriber cannot see it, so the function runs here.
        let (path, channels) = h.app.epg_cache_contents().unwrap();
        write_epg_cache(&path, &channels);
        drop(h);
        // The next start reads it.
        let mut next = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        wait_until("the cache file is read", || {
            next.frame(&[]);
            next.app.epg_store.get(13).is_some()
        })
        .await;
        let text = log.text();
        let has = |what: &str| text.lines().any(|l| l.contains(what));
        assert!(has("epg for sid=3 from the guide's store"), "{text}");
        let written = "guide: epg cache written (1 channels, 1 programmes, 0 KB)";
        assert!(has(written), "{text}");
        let read = "guide: epg cache read (1 channels, 1 programmes)";
        assert!(has(read), "{text}");
    }

    #[tokio::test]
    async fn exit_writes_the_epg_that_came_in_since_the_last_write_and_only_that() {
        for came_in in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
            let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
            h.app.epg_store.insert(13, fresh);
            if came_in {
                h.app.epg_cache_dirty_since = Some(Instant::now());
            }
            drop(h);
            let path = cache_path(dir.path());
            assert_eq!(
                path.exists(),
                came_in,
                "came in since the last write: {came_in}"
            );
            if came_in {
                assert_eq!(cached_ids(&path), [13]);
            }
        }
    }

    #[tokio::test]
    async fn a_portal_save_forgets_the_epg_that_waited_for_the_cache_file() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.app.epg_cache_dirty_since = Some(Instant::now());
        // Nor does the old portal's read still on its way, or its failed write
        // waiting for a second try, hold up the new portal.
        h.app.epg_cache_reading_since = Some(Instant::now());
        h.app.epg_cache_retry_after = Some(Instant::now() + Duration::from_secs(60));
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        };
        let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
        std::env::remove_var("BONGO_INI");
        assert!(saved.is_ok());
        assert!(
            h.app.epg_cache_dirty_since.is_none(),
            "the old portal's EPG is not written under the new portal's name"
        );
        assert!(h.app.epg_cache_reading_since.is_none());
        assert!(h.app.epg_cache_retry_after.is_none());
    }

    /// An app on a catalog of TEST 03 (3) with its catch-up twin (13), and a
    /// channel with no catch-up (5), which the guide has no column for. Its
    /// portal answers every short EPG with `short_epg`: what a zap gets from the
    /// portal is that, what it gets from the guide's store is the store's.
    fn zap_app(root: &std::path::Path, short_epg: Vec<EpgEntry>) -> Harness {
        let plain = |stream_id: i64, name: &str| LiveChannel {
            tv_archive: 0,
            ..archive_channel(stream_id, name)
        };
        let live = vec![
            plain(3, "TEST 03"),
            archive_channel(13, "TEST 03 HD"),
            plain(5, "TEST 05"),
        ];
        let portal = FakePortal::new(live.clone(), Vec::new());
        portal.set_short_epg(short_epg);
        let catalog = crate::portal::Catalog {
            live,
            ..Default::default()
        };
        let catalog = Arc::new(CatalogStore::from_catalog(portal, catalog));
        Harness::start(root, catalog, PortalState::Configured, None)
    }

    #[tokio::test]
    async fn a_zap_to_either_variant_of_a_guide_channel_takes_the_guides_epg() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        h.frame(&[]);
        let fresh = ChannelEpg::new(fresh_epg("Journaal"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        // The live variant the guide plays, and the catch-up twin it fetches.
        for sid in [3, 13] {
            h.app.zap_by_id(sid);
            h.frame(&[]);
            let title = h
                .app
                .current_epg
                .as_ref()
                .and_then(|e| e.entries().first().map(|x| x.title.clone()));
            assert_eq!(title.as_deref(), Some("Journaal"), "zapped to {sid}");
        }
    }

    #[tokio::test]
    async fn a_zap_asks_the_portal_when_the_guide_has_no_fresh_epg_for_the_channel() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = zap_app(dir.path(), Vec::new());
        h.frame(&[]);
        // TEST 03's column has EPG, fetched 7 hours ago: stale. TEST 05 has no
        // column at all.
        let old = chrono::Utc::now() - chrono::Duration::hours(7);
        h.app
            .epg_store
            .insert(13, ChannelEpg::new(fresh_epg("Gisteren"), old));
        for sid in [3, 5] {
            h.app.zap_by_id(sid);
            wait_until("the portal's answer", || {
                h.frame(&[]);
                h.app.current_epg.is_some()
            })
            .await;
            let programmes = h.app.current_epg.as_ref().map_or(0, |e| e.entries().len());
            assert_eq!(
                programmes, 0,
                "zapped to {sid}: the portal's answer, not the store's"
            );
        }
    }

    // The zap toast gets the guide's EPG from the programme on now, at most 100
    // of them: what the short EPG gave it before.

    /// A programme from `from` to `to` minutes after `now`.
    fn programme_at(
        now: chrono::DateTime<chrono::Utc>,
        title: &str,
        from: i64,
        to: i64,
    ) -> EpgEntry {
        EpgEntry {
            title: title.into(),
            start: now + chrono::Duration::minutes(from),
            end: now + chrono::Duration::minutes(to),
            has_archive: None,
        }
    }

    /// The titles of the EPG the zap toast works from.
    fn toast_titles(h: &Harness) -> Vec<String> {
        let titles = h.app.current_epg.as_ref().map(|epg| {
            let all = epg.entries().iter();
            all.map(|e| e.title.clone()).collect::<Vec<_>>()
        });
        titles.unwrap_or_default()
    }

    #[tokio::test]
    async fn the_zap_toast_gets_the_guides_epg_from_the_programme_on_now() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let now = chrono::Utc::now();
        // The day table: a programme that ended two hours ago, the one on now
        // and the next one.
        let table = Epg::new(vec![
            programme_at(now, "Gisteren", -180, -120),
            programme_at(now, "Nu", -30, 30),
            programme_at(now, "Straks", 30, 90),
        ]);
        h.app.epg_store.insert(13, ChannelEpg::new(table, now));
        h.app.zap_by_id(3);
        h.frame(&[]);
        assert_eq!(toast_titles(&h), ["Nu", "Straks"], "no stale programme");
    }

    #[tokio::test]
    async fn the_zap_toast_gets_at_most_a_hundred_programmes() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let now = chrono::Utc::now();
        // 20 programmes that have ended, the one on now (P0) and 149 after it.
        let programmes =
            (-20..150).map(|i| programme_at(now, &format!("P{i}"), 10 * i - 5, 10 * i + 5));
        let table = Epg::new(programmes.collect());
        h.app.epg_store.insert(13, ChannelEpg::new(table, now));
        h.app.zap_by_id(3);
        h.frame(&[]);
        let titles = toast_titles(&h);
        assert_eq!(titles.len(), 100);
        assert_eq!((titles[0].as_str(), titles[99].as_str()), ("P0", "P99"));
    }

    #[tokio::test]
    async fn a_fresh_store_entry_with_nothing_from_now_on_sends_the_zap_to_the_portal() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let mut h = zap_app(dir.path(), vec![programme_at(now, "Portal", -10, 50)]);
        h.frame(&[]);
        // Fetched a moment ago, but everything in it has ended.
        let table = Epg::new(vec![programme_at(now, "Gisteren", -180, -120)]);
        h.app.epg_store.insert(13, ChannelEpg::new(table, now));
        h.app.zap_by_id(3);
        wait_until("the portal's answer", || {
            h.frame(&[]);
            h.app.current_epg.is_some()
        })
        .await;
        assert_eq!(
            toast_titles(&h),
            ["Portal"],
            "the portal's answer, not the store's"
        );
    }

    // Exit stops the player first; a cache write that failed is tried again.

    #[tokio::test]
    async fn exit_stops_the_player_before_it_waits_for_the_cache_writer() {
        let dir = tempfile::tempdir().unwrap();
        let started = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let Harness {
            mut app, mut cmds, ..
        } = started;
        // A writer that is still at work until it is let go.
        let (release, held) = std::sync::mpsc::channel::<()>();
        app.epg_cache_writer = Some(std::thread::spawn(move || {
            let _ = held.recv();
        }));
        // It is let go once the player has been told to quit, or after 2 s.
        let watcher = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(2);
            let mut told = false;
            while !told && Instant::now() < until {
                match cmds.try_recv() {
                    Ok(Cmd::Quit) => told = true,
                    Ok(_) => {}
                    Err(_) => std::thread::sleep(Duration::from_millis(5)),
                }
            }
            let _ = release.send(());
            told
        });
        drop(app);
        assert!(
            watcher.join().unwrap(),
            "mpv keeps running while exit waits for the writer"
        );
    }

    /// A store with a fresh channel 13, and a directory where its cache file
    /// should be: a write of it cannot succeed. The write starts at once and the
    /// test waits until it has finished (and failed). The instant it started at.
    async fn start_a_write_that_fails(h: &mut Harness, root: &std::path::Path) -> Instant {
        let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        std::fs::create_dir_all(cache_path(root)).unwrap();
        let t0 = Instant::now();
        h.app.epg_cache_dirty_since = Some(t0);
        h.app.write_epg_cache_when_due(t0);
        assert!(h.app.epg_cache_dirty_since.is_none(), "the write started");
        wait_until("the writer has finished", || {
            let writer = h.app.epg_cache_writer.as_ref();
            writer.is_some_and(|w| w.is_finished())
        })
        .await;
        t0
    }

    #[tokio::test]
    async fn a_failed_cache_write_marks_the_store_dirty_again_and_is_tried_a_minute_later() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let t0 = start_a_write_that_fails(&mut h, dir.path()).await;
        // The next frame takes the writer in and sees that it failed.
        let t1 = t0 + Duration::from_secs(1);
        h.app.write_epg_cache_when_due(t1);
        assert!(h.app.epg_cache_writer.is_none(), "taken in");
        assert!(
            h.app.epg_cache_dirty_since.is_some(),
            "the failed write is to be tried again"
        );
        // Not at once: a failure that stays would write, fail and log on every
        // frame. The obstacle is gone by the time of the next try.
        std::fs::remove_dir(&path).unwrap();
        h.app.write_epg_cache_when_due(t1 + Duration::from_secs(30));
        assert!(h.app.epg_cache_writer.is_none(), "not within the minute");
        h.app.write_epg_cache_when_due(t1 + Duration::from_secs(61));
        assert!(h.app.epg_cache_dirty_since.is_none(), "a minute later");
        wait_until("the cache file is written", || path.is_file()).await;
        assert_eq!(cached_ids(&path), [13]);
    }

    #[tokio::test]
    async fn exit_tries_a_cache_write_that_failed_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = cache_path(dir.path());
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        start_a_write_that_fails(&mut h, dir.path()).await;
        // The obstacle goes. Exit takes the failed writer in, and writes.
        std::fs::remove_dir(&path).unwrap();
        drop(h);
        assert_eq!(cached_ids(&path), [13]);
    }

    #[tokio::test]
    async fn a_failed_write_of_the_old_portal_does_not_dirty_the_new_portals_store() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // The portal is saved while that write is under way ...
        let fresh = ChannelEpg::new(fresh_epg("Nu"), chrono::Utc::now());
        h.app.epg_store.insert(13, fresh);
        std::fs::create_dir_all(cache_path(dir.path())).unwrap();
        let t0 = Instant::now();
        h.app.epg_cache_dirty_since = Some(t0);
        h.app.write_epg_cache_when_due(t0);
        let saved = {
            // The variable is the whole process's: held for the save only.
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let creds = XtreamCreds {
                username: "u2".into(),
                password: "p2".into(),
                host: "127.0.0.1".into(),
                port: 1,
            };
            let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
            std::env::remove_var("BONGO_INI");
            saved
        };
        assert!(saved.is_ok());
        // ... and fails. That is no news of the new portal's store, which a
        // write would turn into the new portal's cache file.
        wait_until("the writer has finished", || {
            let writer = h.app.epg_cache_writer.as_ref();
            writer.is_some_and(|w| w.is_finished())
        })
        .await;
        h.app.write_epg_cache_when_due(t0 + Duration::from_secs(1));
        assert!(h.app.epg_cache_writer.is_none(), "taken in");
        assert!(h.app.epg_cache_dirty_since.is_none());
    }

    // The guide's fetches wait for the cache file; the read log counts what the
    // read took in.

    #[tokio::test]
    async fn the_guides_fetches_wait_while_the_cache_is_read_and_start_once_it_has_landed() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = epg_app(dir.path());
        // A read on its way. (The app's own, of a file that is not there, has
        // landed in `Harness::start`.)
        h.app.epg_cache_reading_since = Some(Instant::now());
        h.app.open_guide();
        h.frame(&[]);
        h.frame(&[]);
        assert!(
            h.app.epg_fetcher.in_flight().is_empty(),
            "nothing starts while the cache is read"
        );
        // The read lands, empty: the frame that takes it in, and the next one.
        *h.app.epg_cache_slot.lock() = Some((h.app.catalog_generation, Ok(Vec::new())));
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.app.epg_fetcher.in_flight().len(), 2, "now they start");
    }

    #[tokio::test]
    async fn the_guides_fetches_wait_two_seconds_for_the_cache_at_most() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = epg_app(dir.path());
        // A read that hangs.
        let t0 = Instant::now();
        h.app.epg_cache_reading_since = Some(t0);
        h.app.open_guide();
        h.app.start_epg_fetches(t0 + Duration::from_millis(1900));
        assert!(h.app.epg_fetcher.in_flight().is_empty(), "still waiting");
        h.app.start_epg_fetches(t0 + Duration::from_secs(2));
        assert_eq!(h.app.epg_fetcher.in_flight().len(), 2, "not for ever");
    }

    #[tokio::test]
    async fn a_guide_opened_as_the_catalog_loads_does_not_fetch_what_the_cache_holds() {
        let dir = tempfile::tempdir().unwrap();
        let now = chrono::Utc::now();
        let held = crate::guide::cache::ToSave {
            id: 13,
            epg: ChannelEpg::new(fresh_epg("Nu"), now),
            archive_days: 7,
        };
        crate::guide::cache::save(&cache_path(dir.path()), &[held], now).unwrap();
        let portal = FakePortal::new(vec![archive_channel(13, "TEST 03 HD")], Vec::new());
        portal.release_epg();
        let catalog = Arc::new(CatalogStore::new(portal.clone()));
        let mut h = Harness::start(dir.path(), catalog.clone(), PortalState::Configured, None);
        // The frame in which the catalog loads starts the read; the guide is
        // opened right then.
        load_catalog(&catalog, &portal, &mut h).await;
        h.app.open_guide();
        wait_until("the cache is in the store", || {
            h.frame(&[]);
            h.app.epg_store.get(13).is_some()
        })
        .await;
        for _ in 0..10 {
            h.frame(&[]);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert_eq!(portal.epg_fetches(), 0, "the cache had it");
    }

    #[tokio::test]
    async fn the_read_log_counts_what_the_read_took_in_not_the_stores_total() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        let now = chrono::Utc::now();
        // The store holds a channel of its own already, with three programmes.
        let own = (0..3).map(|i| programme_at(now, &format!("Eigen {i}"), 10 * i, 10 * i + 10));
        let own = ChannelEpg::new(Epg::new(own.collect()), now);
        h.app.epg_store.insert(999, own);
        // A read lands with the guide's channel (one programme) and one the guide
        // does not list (two).
        let log = LogCapture::start();
        let two = Epg::new(vec![
            programme_at(now, "Een", 0, 10),
            programme_at(now, "Twee", 10, 20),
        ]);
        let landed = vec![
            (13, ChannelEpg::new(fresh_epg("Nu"), now)),
            (888, ChannelEpg::new(two, now)),
        ];
        *h.app.epg_cache_slot.lock() = Some((h.app.catalog_generation, Ok(landed)));
        h.frame(&[]);
        let text = log.text();
        let read = "guide: epg cache read (1 channels, 1 programmes)";
        assert!(text.contains(read), "{text}");
    }

    // ---- the news picker ----

    fn archive_channel(stream_id: i64, name: &str) -> LiveChannel {
        LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 1,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        }
    }

    /// An app whose catalog holds "NL | NPO1 HD", a catch-up channel the first
    /// default news rule finds, and a portal whose day EPG holds one bulletin
    /// and arrives on `release_epg`.
    fn news_app(root: &std::path::Path) -> (Arc<FakePortal>, Arc<CatalogStore>, Harness) {
        let now = chrono::Utc::now();
        let bulletin = EpgEntry {
            title: "NOS Journaal".into(),
            start: now - chrono::Duration::hours(2),
            end: now - chrono::Duration::minutes(110),
            has_archive: None,
        };
        let live = vec![archive_channel(601, "NL | NPO1 HD")];
        let portal = FakePortal::new(live.clone(), vec![bulletin]);
        let catalog = Arc::new(CatalogStore::from_catalog(
            portal.clone(),
            crate::portal::Catalog {
                live,
                ..Default::default()
            },
        ));
        let h = Harness::start(root, catalog.clone(), PortalState::Configured, None);
        (portal, catalog, h)
    }

    /// Lets the news fetch finish and the app notice: whether its list came up.
    async fn finish_news_fetch(portal: &FakePortal, h: &mut Harness) -> bool {
        portal.release_epg();
        let slot = h.app.news_picker_slot.clone();
        wait_until("the news list", || slot.lock().is_some()).await;
        h.frame(&[]);
        h.app.news_picker.is_some()
    }

    #[test]
    fn a_news_list_is_kept_only_for_the_catalog_and_the_lock_it_was_made_for() {
        let stamp = |generation, unlocked| NewsStamp {
            generation,
            unlocked,
        };
        assert!(keep_news_result(stamp(0, false), stamp(0, false)));
        assert!(keep_news_result(stamp(3, true), stamp(3, true)));
        // A Lock (or an unlock) since, or another catalog since: dropped.
        assert!(!keep_news_result(stamp(0, true), stamp(0, false)));
        assert!(!keep_news_result(stamp(0, false), stamp(0, true)));
        assert!(!keep_news_result(stamp(0, false), stamp(1, false)));
        assert!(!keep_news_result(stamp(2, true), stamp(3, false)));
    }

    #[tokio::test]
    async fn a_held_n_starts_one_news_fetch_and_the_next_n_waits_for_its_list() {
        let dir = tempfile::tempdir().unwrap();
        let (portal, _catalog, mut h) = news_app(dir.path());
        // A held key repeats: three presses in a row ...
        h.press(&[Key::N, Key::N, Key::N]);
        wait_until("the fetch", || portal.epg_fetches() >= 1).await;
        // ... and more while the portal is still thinking.
        h.press(&[Key::N, Key::N]);
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(portal.epg_fetches(), 1, "one batch, not one per press");
        assert!(finish_news_fetch(&portal, &mut h).await);
        let picker = h.app.news_picker.as_ref().expect("the list popped up");
        assert_eq!(picker.items.len(), 1);
        assert!(!h.app.news_fetching);
        // Closed again, the next `n` fetches anew.
        h.press(&[Key::Escape, Key::N]);
        wait_until("the second fetch", || portal.epg_fetches() == 2).await;
    }

    #[tokio::test]
    async fn a_news_list_fetched_under_another_lock_never_pops_up() {
        for unlocked_at_fetch in [true, false] {
            let dir = tempfile::tempdir().unwrap();
            let (portal, catalog, mut h) = news_app(dir.path());
            catalog.set_adult_unlocked(unlocked_at_fetch);
            h.press(&[Key::N]);
            wait_until("the fetch", || portal.epg_fetches() == 1).await;
            // Lock (or unlock) while the portal is still thinking.
            h.app.set_adult_visible(!unlocked_at_fetch);
            let shown = finish_news_fetch(&portal, &mut h).await;
            assert!(!shown, "unlocked when fetched: {unlocked_at_fetch}");
            // The fetch is over all the same: `n` works again.
            assert!(!h.app.news_fetching);
        }
    }

    #[tokio::test]
    async fn a_news_list_fetched_through_the_old_portal_is_dropped_after_a_save() {
        let dir = tempfile::tempdir().unwrap();
        let (portal, _catalog, mut h) = news_app(dir.path());
        h.press(&[Key::N]);
        wait_until("the fetch", || portal.epg_fetches() == 1).await;
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let creds = XtreamCreds {
                username: "u2".into(),
                password: "p2".into(),
                host: "127.0.0.1".into(),
                port: 1,
            };
            let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
            std::env::remove_var("BONGO_INI");
            assert!(saved.is_ok());
        }
        assert!(!finish_news_fetch(&portal, &mut h).await);
        assert!(!h.app.news_fetching);
    }

    #[tokio::test]
    async fn a_news_list_that_lands_under_the_settings_overlay_is_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let (portal, _catalog, mut h) = news_app(dir.path());
        h.press(&[Key::N]);
        wait_until("the fetch", || portal.epg_fetches() == 1).await;
        h.press(&[Key::O]);
        assert!(h.app.settings_ui.is_some());
        assert!(!finish_news_fetch(&portal, &mut h).await);
        assert!(h.app.settings_ui.is_some(), "the overlay stays up");
    }

    // ---- the portal fields ----

    #[test]
    fn the_setup_prompt_cleans_what_was_pasted_before_it_tests_it() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        let paste = |text: &str| egui::Event::Paste(text.into());
        h.press(&[Key::Tab]); // the server field
        h.frame_with(vec![paste("https://srv.example.net:8080/\r\n")]);
        h.press(&[Key::Tab]);
        h.frame_with(vec![paste(" bob\n")]);
        h.press(&[Key::Tab]);
        h.frame_with(vec![paste("se cret\r\n")]);
        assert_eq!(
            h.app.prompt_credentials(),
            "bob:se cret@srv.example.net:8080"
        );
    }

    #[tokio::test]
    async fn the_portal_section_shows_the_command_line_note_until_a_save_replaces_the_portal() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.app.creds_from_command_line = true;
        let note = crate::i18n::t(Lang::En, T::PortalFromCommandLine);
        let says_note = |h: &Harness| h.painted.iter().any(|p| p == note);
        // o, Down, Enter: the Portal section.
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        h.frame(&[]);
        assert!(says_note(&h), "{:?}", h.painted);
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let creds = XtreamCreds {
                username: "u2".into(),
                password: "p2".into(),
                host: "127.0.0.1".into(),
                port: 1,
            };
            let saved = h.app.apply_creds(creds, "u2:p2@127.0.0.1:1");
            std::env::remove_var("BONGO_INI");
            assert!(saved.is_ok());
        }
        // The portal in use is the saved one now, no longer the command line's.
        h.frame(&[]);
        assert!(!says_note(&h), "{:?}", h.painted);
    }

    // ---- the PIN dialogs, end to end through the app ----

    /// A locked app with the PIN 1234 and the Adult section of the settings
    /// open on its first button.
    fn app_in_the_adult_section(root: &std::path::Path) -> Harness {
        let mut h = Harness::start(root, loaded_catalog(), PortalState::Configured, None);
        h.app.settings.pin = Some(PinHash::new("1234").unwrap());
        // o, then Down to Adult, then Enter into the section: Unlock has the focus.
        h.press(&[
            Key::O,
            Key::ArrowDown,
            Key::ArrowDown,
            Key::ArrowDown,
            Key::Enter,
        ]);
        h
    }

    fn says(h: &Harness, text: &str) -> bool {
        h.painted.iter().any(|p| p == text)
    }

    #[test]
    fn a_wrong_pin_stays_in_the_dialog_with_the_apps_note_and_the_right_one_unlocks() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = app_in_the_adult_section(dir.path());
        h.press(&[Key::Enter]); // Unlock opens its dialog
        h.type_text("0000");
        h.press(&[Key::Enter]);
        assert!(!h.app.catalog.adult_unlocked());
        assert!(says(&h, "Wrong PIN"), "{:?}", h.painted);
        // The dialog is still there, empty, with the keyboard in it.
        h.type_text("1234");
        h.press(&[Key::Enter]);
        assert!(h.app.catalog.adult_unlocked());
        assert!(says(&h, "Unlocked until you quit"), "{:?}", h.painted);
        // The dialog went: Lock is the keyboard's button.
        h.press(&[Key::Enter]);
        assert!(!h.app.catalog.adult_unlocked());
    }

    #[test]
    fn three_wrong_pins_block_input_and_the_dialog_stays_with_its_ok_disabled() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = app_in_the_adult_section(dir.path());
        h.press(&[Key::Enter]);
        for _ in 0..3 {
            h.type_text("0000");
            h.press(&[Key::Enter]);
        }
        assert!(h
            .painted
            .iter()
            .any(|p| p.starts_with("Too many attempts - wait ")));
        // The right PIN does nothing while the block lasts.
        h.type_text("1234");
        h.press(&[Key::Enter]);
        assert!(!h.app.catalog.adult_unlocked());
    }

    #[test]
    fn a_pin_changed_twice_shows_each_attempts_own_answer_not_the_one_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = app_in_the_adult_section(dir.path());
        h.press(&[Key::Enter]);
        h.type_text("1234");
        h.press(&[Key::Enter]); // unlocked: the Adult section shows Lock first
        let change = |h: &mut Harness, old: &str, new: &str| {
            // Tab to Change PIN and open it: the current PIN, the new one, its repeat.
            h.press(&[Key::Tab, Key::Enter]);
            h.type_text(old);
            h.press(&[Key::Enter]);
            h.type_text(new);
            h.press(&[Key::Enter]);
            h.type_text(new);
            h.press(&[Key::Enter]);
        };
        change(&mut h, "1234", "5678");
        assert!(says(&h, "PIN changed"), "{:?}", h.painted);
        assert!(h.app.settings.pin.as_ref().unwrap().verify("5678"));
        // The old PIN is wrong now: the note must be the app's new answer, and the
        // dialog stays open for another try.
        change(&mut h, "1234", "4321");
        assert!(says(&h, "Wrong PIN"), "{:?}", h.painted);
        assert!(!says(&h, "PIN changed"), "{:?}", h.painted);
        assert!(h.app.settings.pin.as_ref().unwrap().verify("5678"));
    }

    #[test]
    fn the_note_of_an_earlier_pin_is_never_the_answer_to_the_next_one() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.app.settings.pin = Some(PinHash::new("1234").unwrap());
        for _ in 0..3 {
            h.app
                .apply_settings_action(SettingsAction::Unlock("0000".into()));
        }
        // Blocked now. A note left from before must not stand for an answer the
        // blocked app never gave.
        h.app.adult_note = Some(T::PinChanged);
        h.app.apply_settings_action(SettingsAction::ChangePin {
            old: "1234".into(),
            new: "5678".into(),
        });
        assert_eq!(h.app.adult_note, None);
        assert!(h.app.settings.pin.as_ref().unwrap().verify("1234"));
    }

    #[test]
    fn a_wrong_portal_password_stays_in_the_forgot_pin_dialog_and_the_right_one_clears_the_pin() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = app_in_the_adult_section(dir.path());
        // Unlock, Change PIN, then Forgot PIN?
        h.press(&[Key::Tab, Key::Tab, Key::Enter]);
        h.type_text("nope");
        h.press(&[Key::Enter]);
        assert!(h.app.settings.pin.is_some());
        assert!(says(&h, "Wrong portal password"), "{:?}", h.painted);
        // The portal password of the test app is "p".
        h.type_text("p");
        h.press(&[Key::Enter]);
        assert!(h.app.settings.pin.is_none());
        assert!(
            says(&h, "PIN cleared - choose a new one when unlocking"),
            "{:?}",
            h.painted
        );
    }
}
