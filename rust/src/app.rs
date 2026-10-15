use crate::args::{
    credentials_line, parse_xtream_creds, resolve_creds, CredsError, CredsSource, XtreamCreds,
};
use crate::catalog::GuideChannel;
use crate::catalog::{fallback_variant, quality_label, quality_rank, zap_position, CatalogNotice, CatalogStatus, CatalogStore};
use crate::epg::{Epg, EpgEntry};
use crate::favorites::{Entry as FavoriteEntry, Favorites};
use crate::guide::fetch::EpgFetcher;
use crate::guide::logos::LogoStore;
use crate::guide::store::{fetch_order, ChannelEpg, EpgStore};
use crate::guide::time::Playable;
use crate::guide_ui::{
    paint_wait_screen, starts_text, GuideAction, GuideData, GuideView, WaitInfo,
};
use crate::i18n::{Lang, T};
use crate::news::NewsRule;
use crate::pin::{PinGate, PinHash};
use crate::playback_health::{PlaybackHealth, ReloadBudget};
use crate::relay::Relay;
use crate::player::{Cmd, Event, FrameFence, PlayerHandle, RgbaFrame};
use crate::portal::m3u::M3uPortal;
use crate::presets::Presets;
use crate::portal::xtream::ConnCheck;
use crate::portal::{xtream::XtreamPortal, LiveChannel, Portal};
use crate::search::{self, ItemKind, SearchItem};
use crate::settings::Settings;
use crate::settings_ui::{
    action_button, conn_rich_text, marker_scroll_area, paint_scroll_marker, ConnState,
    SettingsAction, SettingsUi, SettingsView,
};
use crate::shortcuts;
use crate::sources::{Editing, Source, SourceDraft, SourceId, SourceKind, Sources};
use crate::storage::Storage;
use crate::window_drag::{self, Mouse, RightDrag};
use crate::zap::ZapPacer;
use egui::{Color32, ColorImage, Key, TextureHandle, TextureOptions};
use parking_lot::Mutex;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tracing::warn;

/// Persisted "what was playing" so startup can resume it. `live` flags whether
/// the stall watchdog should arm on resume (live channels yes, VOD/catch-up no).
/// `Debug` for the tests only: the URL holds the account.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[cfg_attr(test, derive(Debug))]
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

/// What the catalog says about a stream URL: `None` while it is not loaded (an
/// M3U list says "not adult" at once). A catalog that knows nothing is not one
/// that says "not adult".
fn catalog_says_adult(catalog: &CatalogStore, url: &str) -> Option<bool> {
    // An M3U list has no adult content: that is known before it has loaded.
    if catalog.kind() == SourceKind::M3u {
        return Some(false);
    }
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
    // No address plays nothing: the next start would resume nothing.
    if url.is_empty() {
        return None;
    }
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
                row(t(T::KeyRightDrag), t(T::HelpRightDrag)),
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
                row("/", t(T::HelpGuideFilter)),
                row("f", t(T::HelpGuideSearch)),
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

/// Whether the app has a source to run on, or shows the setup prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalState {
    /// A source runs: the portal of `--xtream` (non-placeholder values), or
    /// the source in use in `BongoPlayer.ini`: an M3U list, or Xtream
    /// credentials that are no placeholder.
    Configured,
    /// Nothing to run: neither `--xtream` nor a source in the ini gives
    /// credentials, or the ones they give cannot be read.
    Missing,
    /// The credentials found, on the command line or in the ini, are the
    /// placeholder template's.
    Placeholder,
}

/// What the app runs on for a saved source.
pub struct OpenedSource {
    pub portal: Arc<dyn Portal>,
    /// The Xtream credentials; none for an M3U list.
    pub creds: Option<XtreamCreds>,
    pub state: PortalState,
}

/// A portal that is never asked anything: what runs while the setup prompt shows.
pub fn anon_portal() -> Arc<dyn Portal> {
    Arc::new(XtreamPortal::new(XtreamCreds {
        username: "anon".into(),
        password: "anon".into(),
        host: "127.0.0.1".into(),
        port: 1,
    }))
}

/// What the app runs on for Xtream credentials that were looked for
/// ([`resolve_creds`]): usable ones run the portal, placeholder, missing or
/// unreadable ones show the setup prompt, on a portal that is never asked
/// anything. The one place of the words 0.6.1's log has for it, `main` and
/// [`open_source`] both come through here.
pub fn open_xtream(resolved: CredsSource) -> OpenedSource {
    let prompt = |state| OpenedSource {
        portal: anon_portal(),
        creds: None,
        state,
    };
    match resolved {
        CredsSource::Configured(c, _) => {
            tracing::info!("portal: xtream <user>@{}:{}", c.host, c.port);
            OpenedSource {
                portal: Arc::new(XtreamPortal::new(c.clone())),
                creds: Some(c),
                state: PortalState::Configured,
            }
        }
        CredsSource::Placeholder => {
            tracing::warn!("portal: placeholder credentials - showing the setup prompt");
            prompt(PortalState::Placeholder)
        }
        CredsSource::Missing => {
            tracing::warn!(
                "portal: no credentials (--xtream or {}.ini) - showing the setup prompt",
                crate::APP_ID
            );
            prompt(PortalState::Missing)
        }
        CredsSource::Invalid(e) => {
            tracing::warn!("portal: unreadable credentials ({e}) - showing the setup prompt");
            prompt(PortalState::Missing)
        }
    }
}

/// The portal of a saved source and the state the app runs in on it: an
/// Xtream source with placeholder or unreadable credentials shows the setup
/// prompt. Says in the log which source it is, never an M3U address.
pub fn open_source(source: &Source, storage: &Storage) -> OpenedSource {
    match source.kind {
        SourceKind::M3u => {
            tracing::info!(
                "source: m3u {} ({})",
                source.id,
                crate::redact::url_host(&source.address)
            );
            let cache = storage.m3u_cache_path(source.id);
            OpenedSource {
                portal: Arc::new(M3uPortal::new(source.id, &source.address, cache)),
                creds: None,
                state: PortalState::Configured,
            }
        }
        SourceKind::Xtream => open_xtream(resolve_creds(None, Some(&source.address))),
    }
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

/// What to keep when a credentials test logs in: the credentials, the line as
/// typed (what the ini gets), which source they are for and its name. No
/// `Debug` on purpose: it holds a password.
struct KeepXtream {
    creds: XtreamCreds,
    line: String,
    editing: Editing,
    name: String,
}

/// A finished credentials test, on its way from its background task to the UI.
struct ConnResult {
    check: ConnCheck,
    /// What to keep if the login worked (a Save); `None` for a plain test.
    keep: Option<KeepXtream>,
}

pub struct TvApp {
    player: PlayerHandle,
    catalog: Arc<CatalogStore>,
    favorites: Favorites,
    storage: Storage,
    portal_state: PortalState,
    /// The Xtream credentials of the source in use (the portal of `--xtream`
    /// included): the settings screen shows them and the PIN reset checks the
    /// password against them first. None on an M3U list and while the setup
    /// prompt shows; the PIN reset then asks the first saved Xtream source.
    creds: Option<XtreamCreds>,
    /// They came from `--xtream`, which beats the ini on every start: the
    /// settings say that what is saved there only counts without it.
    creds_from_command_line: bool,
    /// The saved sources (`BongoPlayer.ini`), and the one in use unless the
    /// portal of `--xtream` runs. Kept in step with the ini by every save,
    /// switch and delete (`write_sources`).
    sources: Sources,

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
    /// A channel of an M3U list that fails while it is set, and has not played
    /// since it was picked (`current_has_played`), has shown no picture of its
    /// own (see `drain_events` and `stop_for_refused_load`).
    stream_starting_since: Option<Instant>,
    /// The stream that plays has shown a picture since it was picked: its
    /// first `PlaybackStarted` sets this, and any load but a reload of it
    /// clears it. A reload sets `stream_starting_since` again, but the picture
    /// on the screen is still its own.
    current_has_played: bool,
    /// A catch-up pick loading behind the wait screen.
    pending_catchup: Option<PendingCatchup>,
    /// Tracks borderless+always-on-top toggle (`t` key).
    borderless: bool,
    /// The borderless window is being moved with the right mouse button.
    right_drag: Option<RightDrag>,
    /// What the drag asks Windows about the mouse: where the cursor is, whether
    /// the buttons are swapped, whether a button is down. The real thing is
    /// Windows' own; a test puts its own here.
    mouse: Mouse,
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
        sources: Sources,
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
            sources,
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
        sources: Sources,
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
            sources,
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
            current_has_played: false,
            pending_catchup: None,
            borderless: false,
            right_drag: None,
            mouse: Mouse::WINDOWS,
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
        // A resume before an M3U list was read could not tell the channel's id
        // (only the list knows it), and up/down would go from the list's end.
        if self.current_stream_id.is_none() && self.current_live {
            self.current_stream_id = self.catalog.portal().live_stream_id(&url);
        }
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
            SettingsAction::TestConnection(s) => self.start_conn_check(s, None),
            SettingsAction::SaveSource(draft) => self.save_source(draft),
            SettingsAction::ActivateSource(id) => self.activate_source(id),
            SettingsAction::DeleteSource(id) => self.delete_source(id),
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
                // The portal password in use; on an M3U source, the first
                // saved portal's.
                let first_saved = self
                    .sources
                    .first_xtream()
                    .and_then(|s| parse_xtream_creds(&s.address).ok())
                    .map(|c| c.password);
                let actual = self
                    .creds
                    .as_ref()
                    .map(|c| c.password.clone())
                    .or(first_saved)
                    .unwrap_or_default();
                if crate::pin::portal_password_ok(&password, &actual) {
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
            sources: &self.sources,
            active_source: if self.creds_from_command_line {
                None
            } else {
                self.sources.active_id()
            },
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
        // An answer to an action (a source to show next) gives the keyboard to
        // its form now, in this frame, so that a Tab in the next one moves on.
        if let Some(overlay) = self.settings_ui.as_mut() {
            overlay.settle_keyboard(ctx);
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
        tracing::debug!("toast: {}", crate::redact::redact(&s));
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
        // Another stream is a pick, which the catalog's first word leaves alone,
        // and which has shown nothing yet.
        if !reload {
            self.playing_resumed = false;
            self.current_has_played = false;
        }
        // Live goes through the local relay: one continuous stream for mpv
        // that rides out portal refusals instead of dying on them. The portal
        // URL stays the current/last-watched one (the relay port changes
        // between runs). Catch-up (and HLS VOD) comes from one portal request
        // there instead of mpv's two, each a playlist generation of seconds.
        // Only the Xtream portal's: an M3U source's streams come from servers
        // of every kind, with no session rule, and carry their own headers.
        let hls = url.split('?').next().is_some_and(|p| p.ends_with(".m3u8"));
        let via_relay = self.catalog.kind() == SourceKind::Xtream;
        let play = match &self.relay {
            Some(relay) if via_relay && watchdog => relay.local_url(&url),
            Some(relay) if via_relay && hls => relay.vod_url(&url),
            _ => url.clone(),
        };
        // After a stop: what is on the bus now is the stopped stream's. Read
        // before the load goes out, so no frame of the new stream is counted.
        self.frame_fence.load(self.player.frames.version());
        // A list's stream carries the headers its server wants in its URL;
        // any other goes to mpv as it is (an Xtream password may hold a `|`).
        let load = if self.catalog.kind() == SourceKind::M3u {
            Cmd::LoadWithHeaders(play)
        } else {
            Cmd::LoadUrl(play)
        };
        let _ = self.player.cmd_tx.send(load);
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
                    self.current_has_played = true;
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
                    // another one of the channel rather than an error. Only an
                    // Xtream portal refuses variants; an M3U source has none, and
                    // its streams skip the relay that sees the refusal.
                    if reason == "error" && live && self.catalog.kind() == SourceKind::Xtream {
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
                        // Named: on an M3U list many channels are dead or only
                        // seen in their own country, and the app stays on it.
                        let text = match self.current_name.clone() {
                            Some(name) => self.tr(T::StreamErrorOn, &[&name]),
                            None => self.t(T::StreamErrorRetry).to_owned(),
                        };
                        self.set_toast(text);
                        // A channel of a list that dies before it shows a picture
                        // (an error, or an end the reloads above did not get past)
                        // shows the empty state, not the picture of the channel
                        // before: that would look like this one, frozen. Once it
                        // has shown its own, that last frame stays, as on Xtream,
                        // also when its reloads fail. "Before a picture" is
                        // `stream_starting_since`, which every load sets and the
                        // first `PlaybackStarted` clears, while the channel has not
                        // played since it was picked (`current_has_played`; a
                        // reload sets `stream_starting_since` again). Frames from
                        // now on still show, so a load that is on its way already
                        // does (as in `end_catchup_wait`).
                        let no_picture_yet =
                            self.stream_starting_since.is_some() && !self.current_has_played;
                        if no_picture_yet && self.catalog.kind() == SourceKind::M3u {
                            self.frame_fence.stop();
                            self.frame_fence.load(self.player.frames.version());
                            self.video_tex = None;
                        }
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
                        None => {
                            if !self.stop_for_refused_load() {
                                self.set_toast(self.tr(T::ErrorMsg, &[&msg]));
                            }
                        }
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

    /// A player error while an M3U channel is still starting is mpv refusing
    /// the load itself (a list's address or header holds what it cannot take, a
    /// NUL byte): the stream before it was never replaced and would play on
    /// under the new channel's name. That stream stops and its picture goes,
    /// as a dead channel's does, and the toast names the channel; the app
    /// stays on it. True when the error was dealt with so. (M3U only: an
    /// Xtream portal's loads go through the relay, with nothing in them mpv can
    /// refuse.)
    fn stop_for_refused_load(&mut self) -> bool {
        if self.catalog.kind() != SourceKind::M3u || self.stream_starting_since.is_none() {
            return false;
        }
        let Some(name) = self.current_name.clone() else {
            return false;
        };
        let _ = self.player.cmd_tx.send(Cmd::Stop);
        // Its picture goes too, and none of its frames comes back: the empty
        // state, never the channel before under this one's name. The fence stays
        // shut until the next load narrows it (as after `stop_playback`), so a
        // frame mpv renders before the stop lands is kept away as well.
        self.frame_fence.stop();
        self.video_tex = None;
        // Nothing plays to watch, and a reload would be refused again.
        self.stall_watchdog_armed = false;
        self.stream_starting_since = None;
        self.set_toast(self.tr(T::StreamErrorOn, &[&name]));
        true
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
        // An M3U list has no EPG in round 1: the zap toast shows the name
        // alone, and nobody is asked anything.
        if self.catalog.kind() == SourceKind::M3u {
            return;
        }
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
        // An M3U list has no address for an id it does not hold: a target the
        // zap pacer held while a fresh list dropped it. What plays stays.
        if url.is_empty() {
            tracing::warn!("zap: stream_id={} has no address - not loaded", sid);
            return;
        }
        // A channel of an M3U list that has no name is named after its address.
        tracing::info!(
            "zap: -> {} (stream_id={}, idx={:?}, prev={:?})",
            crate::redact::redact(name),
            sid,
            idx,
            self.current_name.as_deref().map(crate::redact::redact)
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
        let sid = self.current_stream_id?;
        // Its own entry first: on an M3U list that is the only way to it. In
        // the deduped Xtream list an entry stands for every variant.
        if let Some(i) = zap.iter().position(|c| c.stream_id == sid) {
            return Some(i);
        }
        let nav = self.catalog.quality_nav(sid);
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
        // The list may have changed under the index (an M3U list's fresh copy, a
        // refetch): an index whose entry is not what plays is found again.
        let current_idx = self
            .current_idx
            .filter(|&i| zap.get(i).map(|c| c.stream_id) == self.current_stream_id);
        let from = self
            .zap_pacer
            .waiting()
            .map(|t| t.2)
            .or(current_idx)
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
        // Park the up/down cursor on this channel's entry in the zap list so
        // subsequent zapping continues from the right place; play the exact sid
        // requested. `zap_position` finds its own entry first (on an M3U list
        // two channels may share a name), else the one that stands for it in
        // the deduped Xtream list.
        let idx = zap_position(&self.catalog.zap_channels(), ch);
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
        let mut open_sources = false;
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
                        // No Tab stop while it is disabled: the link is the next one.
                        let clicked =
                            action_button(ui, ready, crate::i18n::t(lang, T::ConnectAndSave))
                                .clicked();
                        if let Some(text) = conn_rich_text(lang, &self.conn_state) {
                            ui.label(text.size(12.0));
                        }
                        ui.add_space(8.0);
                        // `o` does nothing here: typing must fire no hotkey. The link
                        // is the way to the sources section, its last Tab stop.
                        let link = ui.link(
                            egui::RichText::new(crate::i18n::t(lang, T::PortalSetupM3uLink))
                                .size(12.0),
                        );
                        if link.clicked() {
                            open_sources = true;
                        }
                        let entered =
                            ready && ctx.input(|i| i.key_pressed(Key::Enter)) && !link.has_focus();
                        if clicked || entered {
                            submit = Some(self.prompt_credentials());
                        }
                    });
            });
        if let Some(s) = submit {
            // Saving tests first: only credentials that log in are kept.
            let (editing, name) = self.prompt_target();
            self.start_conn_check(s, Some((editing, name)));
        }
        if open_sources {
            self.open_settings();
            if let Some(ui) = self.settings_ui.as_mut() {
                ui.open_new_list();
            }
            // The Enter or Space that clicked the link is spent: the overlay,
            // drawn later in this frame, would hand it to the address field it
            // focuses, and a single-line field lets go of the focus on Enter.
            ctx.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, Key::Enter);
                i.consume_key(egui::Modifiers::NONE, Key::Space);
            });
        }
    }

    /// Where the setup prompt's login is kept, and under which name: in the
    /// Xtream source in use (a hand-edited ini can leave the prompt up with a
    /// placeholder one that is not the first), else it is the command line's
    /// portal, which goes into the first saved Xtream source or is added. The
    /// source keeps the name it has.
    fn prompt_target(&self) -> (Editing, String) {
        let name_of = |s: &Source| s.display_name(self.lang);
        let in_use = self.sources.active();
        match in_use.filter(|s| s.kind == SourceKind::Xtream) {
            Some(s) => (Editing::Saved(s.id), name_of(s)),
            None => {
                let first = self.sources.first_xtream();
                (Editing::CommandLine, first.map(name_of).unwrap_or_default())
            }
        }
    }

    /// The `user:pass@server` line the setup prompt tests and saves, made of its
    /// three fields as they were typed or pasted (see `credentials_line`).
    fn prompt_credentials(&self) -> String {
        credentials_line(&self.cfg_user, &self.cfg_pass, &self.cfg_host)
    }

    /// Test credentials in the background; with `save` (which source, under
    /// which name), keep them once the login works (see `keep_xtream`). A
    /// failed test changes nothing.
    fn start_conn_check(&mut self, creds_str: String, save: Option<(Editing, String)>) {
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
            let keep = save.map(|(editing, name)| KeepXtream {
                creds: portal.creds,
                line: creds_str,
                editing,
                name,
            });
            *slot.lock() = Some(ConnResult { check, keep });
        });
    }

    /// When a credentials test has finished: say how it went and, if it was
    /// to save and the login worked, keep the credentials (`keep_xtream`).
    /// Whether the channel count came does not matter: a login that works is
    /// all a save needs.
    fn drain_conn_check(&mut self) {
        let Some(ConnResult { check, keep }) = self.conn_slot.lock().take() else {
            return;
        };
        tracing::info!("settings: connection check {:?}", check);
        let saving = keep.is_some();
        self.conn_state = match (check, keep) {
            (check @ ConnCheck::Ok { .. }, Some(keep)) => match self.keep_xtream(keep) {
                Ok(()) => ConnState::Done(check),
                Err(failed) => failed,
            },
            (check, _) => ConnState::Done(check),
        };
        // A Save whose login failed while its form is no longer on show (the
        // user went to another one, or closed the settings): a toast says so,
        // as one says that a Save went through.
        let login_failed =
            matches!(&self.conn_state, ConnState::Done(c) if !matches!(c, ConnCheck::Ok { .. }));
        if saving && login_failed && !self.verdict_on_show() {
            if let Some(text) = crate::settings_ui::conn_text(self.lang, &self.conn_state) {
                self.set_toast(text);
            }
        }
    }

    /// Whether the verdict of a connection test shows: under the setup
    /// prompt's button, which shows it whenever the prompt is drawn, or under
    /// the form in the settings that asked for the test.
    fn verdict_on_show(&self) -> bool {
        match &self.settings_ui {
            Some(settings) => settings.shows_verdict(),
            None => matches!(
                self.portal_state,
                PortalState::Missing | PortalState::Placeholder
            ),
        }
    }

    /// What the last fetch of the M3U list has to tell, as a toast: it was not
    /// updated, or what came was no channel list.
    fn show_catalog_notice(&mut self) {
        if let Some(notice) = self.catalog.take_notice() {
            let key = match notice {
                CatalogNotice::NotUpdated => T::ListNotUpdated,
                CatalogNotice::NotAList => T::NotAChannelList,
            };
            self.set_toast(self.t(key));
        }
    }

    /// Runs the app on `portal` from now on: a new catalog, locked, fetched at
    /// once when `state` is usable; whatever belonged to the old one is
    /// forgotten.
    fn install_portal(
        &mut self,
        portal: Arc<dyn Portal>,
        creds: Option<XtreamCreds>,
        state: PortalState,
    ) {
        // What the old portal still has under way writes nothing for it now:
        // its source may be deleted next, and its number given to another.
        self.catalog.portal().retire();
        self.creds = creds;
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
        // through it, an old stream id would be another channel, a catch-up
        // pick a programme of a channel it may not have.
        self.pending_resume = None;
        self.pending_catchup = None;
        self.current_stream_id = None;
        self.playing_resumed = false;
        self.catalog_noted = true;
        self.portal_state = state;
        if state == PortalState::Configured {
            self.catalog.spawn_fetch();
        }
    }

    /// Source `id`'s favorites, presets and last channel from now on. What
    /// plays is the other source's: it stops, and this source's last channel
    /// resumes, as at a start (which resumes only on a source that can run: the
    /// last stream of one that shows the setup prompt has its credentials in
    /// its address).
    fn use_data_of(&mut self, id: SourceId) {
        self.storage.set_source(id);
        self.favorites = Favorites::load(&self.storage.favorites_path()).unwrap_or_default();
        self.presets = Presets::load(&self.storage.presets_path()).unwrap_or_default();
        self.stop_playback();
        // A catch-up pick of the other source's waits no longer: it stopped.
        self.pending_catchup = None;
        // The resumed stream gets the new catalog's first word, as at a start.
        self.catalog_noted = false;
        if self.portal_state != PortalState::Configured {
            return;
        }
        if let Some(lw) = LastWatched::load(&self.storage.last_watched_path()) {
            self.resume(lw);
        }
    }

    /// The name a source is saved under: none when it is its kind's default
    /// in any language (it then shows in whichever language is chosen, also
    /// when the form was made in another one).
    fn stored_name(&self, kind: SourceKind, name: &str) -> String {
        let name = name.trim();
        if kind.is_default_name(name) {
            String::new()
        } else {
            name.to_owned()
        }
    }

    /// Whether saved source `id` is the one in use. The portal of `--xtream` is
    /// a row of its own: while it runs no saved source is in use.
    fn in_use(&self, id: SourceId) -> bool {
        !self.creds_from_command_line && self.sources.active_id() == Some(id)
    }

    /// Whether a save of a `kind` source as `editing` has a source to go to. A
    /// saved one must be there and of that kind (`Sources::keep` would add a
    /// new source for an id it does not know, and keeps the kind a source has);
    /// the command line's portal is an Xtream one. Says in the log when not.
    fn can_keep(&self, editing: Editing, kind: SourceKind) -> bool {
        let fits = match editing {
            Editing::Saved(id) => self.sources.get(id).is_some_and(|s| s.kind == kind),
            Editing::CommandLine => kind == SourceKind::Xtream,
            Editing::New => true,
        };
        if !fits {
            tracing::warn!(
                "settings: a {} source cannot be saved as {:?}",
                kind.code(),
                editing
            );
        }
        fits
    }

    /// Whether a save of `editing`, kept as source `id`, puts that source in
    /// use: the command line's portal always did (Save in 0.6.1 kept and used
    /// it); a saved source when it is the one in use; a new one when nothing
    /// usable is in use.
    fn save_activates(&self, editing: Editing, id: SourceId) -> bool {
        match editing {
            Editing::CommandLine => true,
            Editing::Saved(_) => self.in_use(id),
            Editing::New => {
                !self.creds_from_command_line
                    && (self.sources.active_id().is_none()
                        || self.portal_state != PortalState::Configured)
            }
        }
    }

    /// A new source of `kind` gets no number: the ini holds the highest there
    /// is (`Sources::keep`). Nothing is kept, and the toast says so.
    fn no_id_left(&mut self, kind: SourceKind) {
        tracing::warn!(
            "settings: no number left for a new {} source - not saved",
            kind.code()
        );
        self.set_toast(self.t(T::SaveFailed));
    }

    /// Writes `sources` to the ini; on failure says so in a toast and gives the
    /// state to show in place of a test's verdict.
    fn write_sources(&mut self, sources: &Sources) -> Result<(), ConnState> {
        let path = crate::args::ini_path();
        sources.save(&path).map_err(|e| {
            let (path, error) = (path.display().to_string(), e.to_string());
            self.set_toast(self.tr(T::CannotWrite, &[&path, &error]));
            ConnState::WriteFailed { path, error }
        })
    }

    /// Keeps Xtream credentials that logged in, as `keep.editing` says (the
    /// setup prompt's and the command line's go into the first saved Xtream
    /// source), and uses them at once when `save_activates` says so. The ini is
    /// written first: when that fails nothing changes, and the failure comes
    /// back to show in place of the test's verdict. So does a source that is
    /// gone by the time the test is over (`ConnState::Idle`: nothing to show).
    fn keep_xtream(&mut self, keep: KeepXtream) -> Result<(), ConnState> {
        if !self.can_keep(keep.editing, SourceKind::Xtream) {
            return Err(ConnState::Idle);
        }
        let name = self.stored_name(SourceKind::Xtream, &keep.name);
        let mut sources = self.sources.clone();
        let Some(id) = sources.keep(keep.editing, SourceKind::Xtream, &name, &keep.line) else {
            self.no_id_left(SourceKind::Xtream);
            return Err(ConnState::Idle);
        };
        let activate = self.save_activates(keep.editing, id);
        if activate {
            sources.set_active(id);
        }
        self.write_sources(&sources)?;
        self.sources = sources;
        self.show_kept_in_settings(Editing::Saved(id));
        if !activate {
            tracing::info!("settings: source {} saved (xtream)", id);
            let shown = self.sources.get(id).map(|s| s.display_name(self.lang));
            self.set_toast(self.tr(T::SourceSaved, &[&shown.unwrap_or_default()]));
            return Ok(());
        }
        tracing::info!(
            "settings: portal saved to {} - connecting",
            crate::args::ini_path().display()
        );
        // The new catalog starts locked, so a save locks like Lock does: asked of
        // the old catalog, the only one that knows this stream, before it goes.
        let playing_adult = self.playing_adult();
        self.creds_from_command_line = false;
        let portal: Arc<dyn Portal> = Arc::new(XtreamPortal::new(keep.creds.clone()));
        self.install_portal(portal, Some(keep.creds), PortalState::Configured);
        if self.storage.source() != id {
            self.use_data_of(id);
        } else if playing_adult {
            self.stop_playback();
        }
        self.set_toast(self.t(T::ConnectingPortal));
        Ok(())
    }

    /// A Save in the sources section. An Xtream source is tested first and
    /// kept only if it logs in; an M3U source is kept as it is.
    fn save_source(&mut self, draft: SourceDraft) {
        match draft.kind {
            SourceKind::Xtream => {
                // No test for a save that has no source to go to.
                if self.can_keep(draft.editing, SourceKind::Xtream) {
                    // Tested as it is kept: `Sources::keep` stores the line
                    // without line breaks and the spaces around it.
                    let line = crate::args::without_newlines(&draft.address)
                        .trim()
                        .to_owned();
                    self.start_conn_check(line, Some((draft.editing, draft.name)));
                }
            }
            SourceKind::M3u => {
                // The command line's portal is an Xtream one: a list saved from
                // its form is a new source.
                let editing = match draft.editing {
                    Editing::CommandLine => Editing::New,
                    other => other,
                };
                self.save_m3u(SourceDraft { editing, ..draft });
            }
        }
    }

    /// Keeps an M3U source and, when `save_activates` says so, uses it at once:
    /// its list is read again, and a changed address drops the old one's
    /// saved list. An address with nothing in it is not kept (it would be
    /// dropped again when the ini is read).
    fn save_m3u(&mut self, draft: SourceDraft) {
        if !self.can_keep(draft.editing, SourceKind::M3u) {
            return;
        }
        if crate::args::without_newlines(&draft.address)
            .trim()
            .is_empty()
        {
            tracing::warn!("settings: a list is not saved without an address");
            return;
        }
        let name = self.stored_name(SourceKind::M3u, &draft.name);
        let old_address = match draft.editing {
            Editing::Saved(id) => self.sources.get(id).map(|s| s.address.clone()),
            _ => None,
        };
        let mut sources = self.sources.clone();
        let Some(id) = sources.keep(draft.editing, SourceKind::M3u, &name, &draft.address) else {
            self.no_id_left(SourceKind::M3u);
            return;
        };
        let activate = self.save_activates(draft.editing, id);
        if activate {
            sources.set_active(id);
        }
        if self.write_sources(&sources).is_err() {
            return;
        }
        self.sources = sources;
        self.show_in_settings(Editing::Saved(id));
        tracing::info!("settings: source {} saved (m3u)", id);
        let Some(source) = self.sources.get(id).cloned() else {
            return;
        };
        if old_address.is_some_and(|old| old != source.address) {
            // The list in use: a download of the old address still under way
            // must not save its list again once it is dropped.
            if activate {
                self.catalog.portal().retire();
            }
            self.storage.remove_saved_list(id);
        }
        self.set_toast(self.tr(T::SourceSaved, &[&source.display_name(self.lang)]));
        if activate {
            let data_changed = self.storage.source() != id;
            self.creds_from_command_line = false;
            let opened = open_source(&source, &self.storage);
            self.install_portal(opened.portal, opened.creds, opened.state);
            if data_changed {
                self.use_data_of(id);
            }
        }
    }

    /// Puts saved source `id` in use, as a start on it would: its portal, its
    /// favorites and presets, its last channel. The ini says so for the next
    /// start and is written first: when that fails nothing changes, so what
    /// runs is always what the next start opens. Nothing to do for the source
    /// that is in use already.
    fn activate_source(&mut self, id: SourceId) {
        let Some(source) = self.sources.get(id).cloned() else {
            return;
        };
        if self.in_use(id) {
            return;
        }
        let mut sources = self.sources.clone();
        sources.set_active(id);
        if self.write_sources(&sources).is_err() {
            return;
        }
        self.sources = sources;
        self.show_in_settings(Editing::Saved(id));
        self.creds_from_command_line = false;
        tracing::info!("source: active {} ({})", id, source.kind.code());
        let opened = open_source(&source, &self.storage);
        self.install_portal(opened.portal, opened.creds, opened.state);
        self.use_data_of(id);
        self.set_toast(self.tr(T::SourceInUse, &[&source.display_name(self.lang)]));
    }

    /// Removes saved source `id`, with its favorites, presets, last channel and
    /// saved list. Never the source in use (the settings offer no Delete for
    /// it), and never the files whose data is in use (the command line's
    /// portal uses source 1's).
    fn delete_source(&mut self, id: SourceId) {
        if self.in_use(id) {
            tracing::warn!("settings: source {} is in use - not deleted", id);
            return;
        }
        let mut sources = self.sources.clone();
        let Some(gone) = sources.remove(id) else {
            return;
        };
        if self.write_sources(&sources).is_err() {
            return;
        }
        self.sources = sources;
        let files = if self.storage.source() == id {
            0
        } else {
            self.storage.remove_source_files(id)
        };
        tracing::info!("settings: source {} deleted ({} files)", id, files);
        self.set_toast(self.tr(T::SourceDeleted, &[&gone.display_name(self.lang)]));
        let in_use = self.sources.active_id();
        let shown = if self.creds_from_command_line {
            Editing::CommandLine
        } else {
            in_use.map_or(Editing::New, Editing::Saved)
        };
        self.show_in_settings(shown);
    }

    /// The settings' sources section shows this source next, if it is open:
    /// the answer to a Save of a list, a Use or a Delete, given in the frame of
    /// that action (`paint_settings` settles the keyboard then).
    fn show_in_settings(&mut self, editing: Editing) {
        if let Some(ui) = self.settings_ui.as_mut() {
            ui.show_source(editing);
        }
    }

    /// The same for the source an Xtream Save kept, which comes when its login
    /// has been tried, whenever that is: the section shows it only if the form
    /// that asked is still the one on show, not one the user moved on to.
    fn show_kept_in_settings(&mut self, editing: Editing) {
        if let Some(ui) = self.settings_ui.as_mut() {
            ui.show_kept_source(editing);
        }
    }

    fn handle_keys(&mut self, ctx: &egui::Context) {
        // Search box owns input when open; only handle escape/enter externally.
        if self.show_search {
            if ctx.input(|i| i.key_pressed(Key::Escape)) {
                self.show_search = false;
                self.search_query.clear();
                // Over the guide this Esc is the search's alone: the guide is
                // drawn in this same frame with its keys back, and would take
                // it to wipe its filter, or else to close.
                spend_key(ctx, Key::Escape);
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
            // The window does not say it is full screen before the next frame:
            // a move in this one would come after the command and win.
            self.end_right_drag("F11");
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
                    let key = match self.catalog.kind() {
                        SourceKind::Xtream => T::RetryingPortal,
                        SourceKind::M3u => T::FetchingList,
                    };
                    self.set_toast(self.t(key));
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

    /// Moves the borderless window while the right mouse button is held down
    /// (see `window_drag`). Idle it reads the input and nothing else: no Win32
    /// call, no command, no repaint.
    fn follow_right_drag(&mut self, ctx: &egui::Context) {
        if !self.borderless {
            self.end_right_drag("border back");
            return;
        }
        let (pressed, down, full_screen, maximized, focused, window) = ctx.input(|i| {
            (
                i.pointer.button_pressed(egui::PointerButton::Secondary),
                i.pointer.button_down(egui::PointerButton::Secondary),
                i.viewport().fullscreen.unwrap_or(false),
                i.viewport().maximized.unwrap_or(false),
                i.focused,
                i.viewport().outer_rect,
            )
        });
        // A drag ends with the button, in full screen, maximized (the first move
        // would restore the window and it would jump) and with the focus: the
        // release of a window that lost it never comes.
        let ends = if !down {
            Some("released")
        } else if full_screen {
            Some("full screen")
        } else if maximized {
            Some("maximized")
        } else if !focused {
            Some("focus lost")
        } else {
            None
        };
        if let Some(why) = ends {
            self.end_right_drag(why);
            return;
        }
        // Only a press begins a drag, never a button that is down already.
        if self.right_drag.is_none() && !pressed {
            return;
        }
        // A release that egui never hears of (another window took the mouse
        // capture) would glue the window to the cursor: while a drag runs, and
        // only then, ask Windows whether the button is still down.
        if let Some(button) = self.right_drag.as_ref().map(RightDrag::button) {
            if !(self.mouse.key_down)(button) {
                self.end_right_drag("button up per Win32");
                return;
            }
        }
        let Some(cursor) = (self.mouse.cursor)() else {
            return;
        };
        // egui's own scale, the UI zoom in it: the window's rect and the
        // `OuterPosition` command are both in points of it.
        let pixels_per_point = ctx.pixels_per_point();
        let drag = match &mut self.right_drag {
            Some(drag) => drag,
            None => {
                let Some(window) = window else {
                    return;
                };
                // The swap is read here, once per drag.
                let button = window_drag::button_to_poll((self.mouse.buttons_swapped)());
                let drag = RightDrag::begin(cursor, window.min, pixels_per_point, button);
                tracing::debug!(
                    "window drag: begins ({drag:?}, {pixels_per_point} pixels to a point)"
                );
                self.right_drag.insert(drag)
            }
        };
        if let Some(to) = drag.follow(cursor, pixels_per_point) {
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(to));
        }
        // The window follows the cursor frame by frame, not at the 30 per second
        // of the rest of the time.
        ctx.request_repaint();
    }

    /// Ends the drag of the window, if one runs, and says why in the log.
    fn end_right_drag(&mut self, why: &str) {
        if self.right_drag.take().is_some() {
            tracing::debug!("window drag: ended ({why})");
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
    /// That second leaves the empty state, never the channel before: no frame that
    /// is not newer than the restart shows any more. A failed wait stays, for the
    /// user to read it.
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
            // A frame of the channel before can have slipped in after the pick
            // (mpv renders one more while the load travels to its thread). It
            // spent the pick's fence, and dropping the texture alone does not
            // help: `upload_newest_frame` puts the bus's newest frame back
            // whenever the texture is gone. So the fence is set again, at the
            // restart: the first frame newer than that lifts it.
            self.frame_fence.stop();
            self.frame_fence.load(restart);
            self.video_tex = None;
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

    /// A pick in the search. When the search was opened over the guide, a pick
    /// that plays has done the guide's job and closes it; a series plays nothing
    /// yet (the toast says so), so there only the search goes and the guide stays
    /// as it was.
    fn handle_search_pick(&mut self, id: i64, name: &str, kind: ItemKind) {
        let plays = match kind {
            ItemKind::Live => {
                self.zap_by_id(id);
                true
            }
            ItemKind::Movie => {
                self.play_movie(id, name);
                true
            }
            ItemKind::Series => {
                self.set_toast(self.tr(T::SeriesNotYet, &[name]));
                false
            }
        };
        self.show_search = false;
        self.search_query.clear();
        if plays {
            self.close_guide();
        }
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
                    let headline = match self.catalog.kind() {
                        SourceKind::Xtream => T::PortalFetchFailed,
                        SourceKind::M3u => T::ListFetchFailed,
                    };
                    ui.label(
                        egui::RichText::new(self.t(headline))
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
        // Round 1 reads no XMLTV guide: an M3U source has none yet.
        if self.catalog.kind() == SourceKind::M3u {
            self.set_toast(self.t(T::NoGuideForSource));
            return;
        }
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
    /// help it takes no keys; `?` or Esc closes the help. Under the search (`f`
    /// opens it over the guide) it takes none either, and no click and no wheel:
    /// the search has the keyboard until Esc, or a pick that plays, which closes
    /// the guide.
    fn paint_guide(&mut self, ctx: &egui::Context) {
        let keys = !self.show_help && !self.show_search;
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
            Some(GuideAction::Search) => {
                // The search box is drawn later in this frame. It would not type
                // this f today (it asks for the keyboard only after it is drawn,
                // and a new area is invisible in its first frame), but the press
                // and its letter are spent all the same: a guard, so that no change
                // to that order can make the box find an f to type.
                spend_key(ctx, Key::F);
                self.show_search = true;
                self.search_query.clear();
            }
            None => {}
        }
    }

    /// A programme picked in the guide: live through the relay, catch-up from
    /// its start on the catch-up twin, or a word on why not.
    fn guide_play(&mut self, channel: GuideChannel, entry: EpgEntry, playable: Playable) {
        let (from, to) = (crate::epg::hhmm(entry.start), crate::epg::hhmm(entry.end));
        match playable {
            Playable::Future => {
                let today = crate::guide::time::day_of(chrono::Utc::now());
                self.set_toast(starts_text(self.lang, entry.start, today));
            }
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
                // a picture (the 1 s cap) shows the empty state, not that channel
                // (`end_catchup_wait` fences a frame that slipped in after this).
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
        self.show_catalog_notice();
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
        // catch-up pick takes Esc and g only. The search that `f` opened over
        // the guide has the keyboard: `handle_keys` then does its Esc, and
        // nothing else.
        if self.settings_ui.is_none() && !setup_prompt {
            if self.pending_catchup.is_some() {
                self.handle_wait_keys(ctx);
            } else if !self.guide_open || self.show_search {
                self.handle_keys(ctx);
            }
        }
        // The right button moves the borderless window, over the video, the guide
        // and every overlay alike.
        self.follow_right_drag(ctx);

        // The wait screen of a catch-up pick takes over the full window, and so
        // does the guide when open. The video texture was brought up to date
        // above in every case, so the frame that ends a wait draws the picture
        // that ended it.
        if self.pending_catchup.is_some() {
            self.paint_wait(ctx);
        } else if self.guide_open {
            self.paint_guide(ctx);
            // The search of `f`, in front of the guide, which keeps its state
            // under it. After the guide: its `f` opens the search in this frame.
            self.paint_search(ctx);
        } else if setup_prompt {
            // First-run: no credentials yet - show the portal setup prompt
            // over a dark background instead of the video surface.
            egui::CentralPanel::default()
                .frame(egui::Frame::none().fill(Color32::from_rgb(14, 14, 18)))
                .show(ctx, |_ui| {});
            // Under the settings the prompt's fields would be Tab stops too.
            if self.settings_ui.is_none() {
                self.paint_portal_prompt(ctx);
            }
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
            // would compete visually with the schedule grid (the search, which
            // `f` opens over the guide too, is drawn in the guide's branch).
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
    use crate::window_drag::{ScreenPos, VK_LBUTTON, VK_RBUTTON};

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
        Arc::new(XtreamPortal::new(test_creds()))
    }

    /// The Xtream credentials of the tests' portal (`idle_portal`).
    fn test_creds() -> XtreamCreds {
        XtreamCreds {
            username: "u".into(),
            password: "p".into(),
            host: "h".into(),
            port: 80,
        }
    }

    /// What a Save of these credentials on the command line's portal keeps:
    /// the first saved portal, under its default name.
    fn portal_save(creds: XtreamCreds, line: &str) -> KeepXtream {
        KeepXtream {
            creds,
            line: line.to_owned(),
            editing: Editing::CommandLine,
            name: String::new(),
        }
    }

    /// Three channels: Kanaal Een; Kanaal Twee, whose server wants its own
    /// User-Agent and Referer; and a second Kanaal Twee on another server.
    const TEST_LIST: &str = "#EXTM3U\n\
#EXTINF:-1 tvg-id=\"Een.nl\" group-title=\"Algemeen\",Kanaal Een\n\
https://a.example/een.m3u8\n\
#EXTINF:-1 group-title=\"Algemeen\",Kanaal Twee\n\
#EXTVLCOPT:http-user-agent=Bongo/1.0\n\
#EXTVLCOPT:http-referrer=https://www.example.com/\n\
https://b.example/twee.m3u8\n\
#EXTINF:-1 group-title=\"Nieuws\",Kanaal Twee\n\
https://c.example/twee-nieuws.m3u8\n";

    /// The app on an M3U source whose list is `text`, read already.
    fn m3u_app(root: &std::path::Path, text: &str) -> Harness {
        m3u_app_with(root, text, None)
    }

    fn m3u_app_with(root: &std::path::Path, text: &str, relay: Option<Relay>) -> Harness {
        let (portal, catalog) = M3uPortal::from_text(text).expect("a list");
        let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        Harness::start_with(
            root,
            store,
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            relay,
        )
    }

    /// The URLs among the player commands.
    fn loads(sent: Vec<Cmd>) -> Vec<String> {
        sent.into_iter()
            .filter_map(|c| match c {
                Cmd::LoadUrl(u) | Cmd::LoadWithHeaders(u) => Some(u),
                _ => None,
            })
            .collect()
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
    fn the_guides_help_names_slash_for_the_filter_and_f_for_the_search() {
        let search = [
            (Lang::Nl, "zoeken in alles: zenders, films, series"),
            (Lang::En, "search everything: channels, films, series"),
            (Lang::De, "alles durchsuchen: Sender, Filme, Serien"),
        ];
        for (lang, search) in search {
            let sections = help_rows(lang, false);
            let guide = &sections[1].1;
            let action = |key: &str| guide.iter().find(|r| r.key == key).map(|r| r.action);
            assert_eq!(
                action("/"),
                Some(crate::i18n::t(lang, T::HelpGuideFilter)),
                "{lang:?}: the filter"
            );
            assert_eq!(action("f"), Some(search), "{lang:?}: the search");
            // The viewer's `f` is the same search, in the other section.
            let viewer = &sections[0].1;
            let viewer_f = viewer.iter().find(|r| r.key == "f").map(|r| r.action);
            assert_eq!(viewer_f, Some(crate::i18n::t(lang, T::HelpSearch)));
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

    #[test]
    fn the_viewer_help_names_the_right_button_drag_below_the_borderless_key() {
        for lang in Lang::ALL {
            let sections = help_rows(lang, false);
            let viewer = &sections[0].1;
            let borderless = viewer.iter().position(|r| r.key == "t").expect("the t row");
            let drag = viewer.get(borderless + 1).expect("a row below the t row");
            assert_eq!(drag.key, crate::i18n::t(lang, T::KeyRightDrag), "{lang:?}");
            assert_eq!(
                drag.action,
                crate::i18n::t(lang, T::HelpRightDrag),
                "{lang:?}"
            );
            // The keys are monospace, in a column as wide as the widest of them: this
            // one must not make that column, and so every row, any wider.
            let widest = viewer
                .iter()
                .filter(|r| r.key != drag.key)
                .map(|r| r.key.chars().count())
                .max()
                .unwrap();
            assert!(drag.key.chars().count() <= widest, "{lang:?}: {}", drag.key);
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
        /// What the window tells egui about itself in every frame: where it is,
        /// the scale of its screen, whether it is full screen. Unknown until a
        /// test says.
        viewport: egui::ViewportInfo,
        /// The window has the keyboard focus.
        focused: bool,
        /// What the app told the window since `moved` last looked.
        window_cmds: Vec<egui::ViewportCommand>,
        /// How soon the last frame asked for the next one.
        repaint_in: Duration,
    }

    thread_local! {
        /// Where the cursor is on the screen for the app under test (`None`:
        /// Windows will not say), and how often the app asked. The real cursor
        /// would be the developer's.
        static CURSOR: std::cell::Cell<Option<ScreenPos>> = const { std::cell::Cell::new(None) };
        static CURSOR_READS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        /// Which physical buttons Windows says are down (left, right) and whether
        /// they are swapped, and how often the app asked about each. A right-handed
        /// mouse with the right button down, until a test says otherwise.
        static PHYSICAL: std::cell::Cell<(bool, bool)> = const { std::cell::Cell::new((false, true)) };
        static SWAPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
        static KEY_POLLS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
        static SWAP_READS: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    }

    fn fake_cursor() -> Option<ScreenPos> {
        CURSOR_READS.set(CURSOR_READS.get() + 1);
        CURSOR.get()
    }

    fn fake_buttons_swapped() -> bool {
        SWAP_READS.set(SWAP_READS.get() + 1);
        SWAPPED.get()
    }

    fn fake_key_down(vkey: i32) -> bool {
        KEY_POLLS.set(KEY_POLLS.get() + 1);
        let (left, right) = PHYSICAL.get();
        match vkey {
            VK_LBUTTON => left,
            VK_RBUTTON => right,
            _ => false,
        }
    }

    /// The mouse of the app under test, in place of Windows' own.
    fn fake_mouse() -> Mouse {
        Mouse {
            cursor: fake_cursor,
            buttons_swapped: fake_buttons_swapped,
            key_down: fake_key_down,
        }
    }

    impl Harness {
        /// The app as `main` starts it on the tests' Xtream portal (u:p@h:80
        /// when configured); see `start_with`.
        fn start(
            root: &std::path::Path,
            catalog: Arc<CatalogStore>,
            state: PortalState,
            initial_url: Option<&str>,
        ) -> Self {
            let creds = (state == PortalState::Configured).then(test_creds);
            Self::start_with(
                root,
                catalog,
                state,
                initial_url,
                creds,
                Sources::default(),
                None,
            )
        }

        /// The app as `main` starts it, with these Xtream credentials (none for
        /// an M3U source), saved sources and relay, `root` as its config folder,
        /// and two quiet frames drawn, so the widgets exist before the first key.
        fn start_with(
            root: &std::path::Path,
            catalog: Arc<CatalogStore>,
            state: PortalState,
            initial_url: Option<&str>,
            creds: Option<XtreamCreds>,
            sources: Sources,
            relay: Option<Relay>,
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
            let app = TvApp::start(
                player,
                catalog,
                Storage::with_root(root.to_path_buf()),
                state,
                initial_url.map(str::to_owned),
                relay,
                sources,
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
                viewport: egui::ViewportInfo::default(),
                focused: true,
                window_cmds: Vec::new(),
                repaint_in: Duration::MAX,
            };
            // The mouse is the test's: the cursor nowhere, the right button of a
            // right-handed mouse down, and nothing asked yet.
            harness.app.mouse = fake_mouse();
            CURSOR.set(None);
            CURSOR_READS.set(0);
            PHYSICAL.set((false, true));
            SWAPPED.set(false);
            KEY_POLLS.set(0);
            SWAP_READS.set(0);
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
            self.frame_holding(egui::Modifiers::NONE, events);
        }

        /// One frame in which these events arrive while the modifier keys are
        /// held down (a key press carries its own copy of them in its event).
        fn frame_holding(&mut self, modifiers: egui::Modifiers, events: Vec<egui::Event>) {
            let raw = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                viewports: std::iter::once((egui::ViewportId::ROOT, self.viewport.clone()))
                    .collect(),
                events,
                modifiers,
                focused: self.focused,
                ..Default::default()
            };
            let app = &mut self.app;
            let output = self.ctx.run(raw, |ctx| app.update_ui(ctx));
            if let Some(window) = output.viewport_output.get(&egui::ViewportId::ROOT) {
                self.window_cmds.extend(window.commands.iter().cloned());
                self.repaint_in = window.repaint_delay;
            }
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

        /// Where the app moved the window (the `OuterPosition` commands, in
        /// points) since the last call.
        fn moved(&mut self) -> Vec<egui::Pos2> {
            std::mem::take(&mut self.window_cmds)
                .into_iter()
                .filter_map(|cmd| match cmd {
                    egui::ViewportCommand::OuterPosition(to) => Some(to),
                    _ => None,
                })
                .collect()
        }

        /// The cursor goes to this pixel of the screen.
        fn cursor_to(&mut self, x: i32, y: i32) {
            CURSOR.set(Some((x, y)));
        }

        /// What Windows says about the physical buttons: whether the left and the
        /// right one are down. egui is told nothing.
        fn physical_buttons(&mut self, left: bool, right: bool) {
            PHYSICAL.set((left, right));
        }

        /// Whether Windows says the buttons are swapped (a left-handed mouse).
        fn buttons_swapped(&mut self, swapped: bool) {
            SWAPPED.set(swapped);
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
        sent.iter()
            .any(|c| matches!(c, Cmd::LoadUrl(_) | Cmd::LoadWithHeaders(_)))
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
    async fn a_programme_on_a_later_day_says_the_day_it_starts() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        open_guide(&mut h, false);
        let today = crate::guide::time::day_of(chrono::Utc::now());
        let tomorrow = today.succ_opt().unwrap();
        let start = crate::guide::time::at_hour(tomorrow, 20);
        let entry = EpgEntry {
            title: "Morgen".into(),
            start,
            end: start + chrono::Duration::hours(1),
            has_archive: None,
        };
        let channel = h.app.guide_channels[0].clone();
        h.app.guide_play(channel, entry, Playable::Future);
        let toast = h.app.toast.as_ref().map(|(t, _, _)| t.as_str());
        assert_eq!(toast, Some("starts tomorrow at 20:00"));
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
            let id = if live_flagged {
                "/911.m3u8"
            } else {
                "/901.m3u8"
            };
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

    // ---- `f` in the guide: the search over everything ----

    /// `guide_catalog`'s channels (TEST 03 and its catch-up twin) and a library:
    /// the film "Zebra Film" (501) and the series "Zebra Serie" (71).
    fn library_catalog() -> Arc<CatalogStore> {
        let channel = |stream_id: i64, name: &str, archive: i32| LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: archive,
            tv_archive_duration: 7 * archive,
            is_adult: false,
            stream_icon: None,
        };
        let catalog = crate::portal::Catalog {
            live: vec![channel(3, "TEST 03", 0), channel(13, "TEST 03 HD", 1)],
            movies: vec![crate::portal::Movie {
                stream_id: 501,
                name: "Zebra Film".into(),
                container_extension: Some("mp4".into()),
                is_adult: false,
            }],
            series: vec![crate::portal::Series {
                series_id: 71,
                name: "Zebra Serie".into(),
                is_adult: false,
            }],
        };
        Arc::new(CatalogStore::from_catalog(idle_portal(), catalog))
    }

    /// The press of `f` as a keyboard makes it: the key and its letter, together.
    fn f_pressed() -> Vec<egui::Event> {
        vec![key_press(Key::F), egui::Event::Text("f".into())]
    }

    /// The guide open on `h` with programmes, and the search opened over it by
    /// `f`; the frame after that one has drawn.
    fn search_over_the_guide(h: &mut Harness) {
        open_guide(h, true);
        h.frame_with(f_pressed());
        assert!(
            h.app.show_search && h.app.guide_open,
            "f opens the search over the guide"
        );
        h.frame(&[]);
    }

    #[tokio::test]
    async fn f_in_the_guide_opens_the_search_over_it_and_does_not_type_the_f() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        // Twice: the search's area is new at the first f and invisible for its
        // first frame, which would keep the f out of the box on its own.
        for round in 1..=2 {
            // What an earlier search left in the box is gone.
            h.app.search_query = "stale".into();
            h.frame_with(f_pressed());
            assert!(h.app.show_search, "f opens the search (round {round})");
            assert!(h.app.guide_open, "over the guide, which stays open");
            assert!(
                h.app.search_query.is_empty(),
                "the f that opened it is not typed into it (round {round})"
            );
            h.frame(&[]);
            assert!(h.app.search_query.is_empty(), "nor a frame later");
            h.frame(&[Key::Escape]);
            h.frame(&[]);
            assert!(!h.app.show_search && h.app.guide_open, "back in the guide");
        }
    }

    #[tokio::test]
    async fn the_f_that_opens_the_search_is_spent_for_the_rest_of_its_frame() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        h.frame_with(f_pressed());
        // The search box is drawn in this very frame, after the guide: the press
        // and its letter must not be there for it to find.
        let left = h.ctx.input(|i| i.events.clone());
        assert!(left.is_empty(), "the frame still holds {left:?}");
    }

    #[tokio::test]
    async fn the_search_over_the_guide_is_drawn_in_front_of_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        h.frame(&[]);
        let at = |h: &Harness, text: &str| h.painted.iter().position(|t| t == text);
        let guide = at(&h, "Journaal 0");
        let search = at(&h, "search live / movie / series...");
        assert!(guide.is_some(), "the guide is still there: {:?}", h.painted);
        assert!(search.is_some(), "and so is the search: {:?}", h.painted);
        assert!(guide < search, "painted after the guide, so in front of it");
        let top = h.ctx.layer_id_at(egui::pos2(640.0, 80.0));
        assert_eq!(top.map(|layer| layer.id), Some(egui::Id::new("__search__")));
    }

    #[tokio::test]
    async fn a_film_picked_in_the_search_over_the_guide_plays_and_closes_the_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        let log = LogCapture::start();
        search_over_the_guide(&mut h);
        h.type_text("zebra");
        let hits: Vec<_> = h
            .app
            .search_hits
            .iter()
            .map(|it| (it.kind, &it.name[..]))
            .collect();
        assert!(
            hits.contains(&(ItemKind::Movie, "Zebra Film")),
            "the film is a hit: {hits:?}"
        );
        h.sent();
        h.press(&[Key::Enter]);
        let sent = h.sent();
        assert!(
            sent.iter().any(|c| matches!(c, Cmd::LoadUrl(u)
                if u.contains("/movie/") && u.ends_with("/501.mp4"))),
            "{sent:?}"
        );
        // One load only: the Enter that picked is not the guide's too, or the
        // programme on now would play as well.
        let loads = sent.iter().filter(|c| matches!(c, Cmd::LoadUrl(_))).count();
        assert_eq!(
            loads, 1,
            "the Enter that picked reached the guide: {sent:?}"
        );
        assert!(!h.app.guide_open, "the pick closes the guide");
        assert!(!h.app.show_search, "and the search");
        let text = log.text();
        let closed = text.lines().filter(|l| l.contains("guide: closed")).count();
        assert_eq!(closed, 1, "{text}");
    }

    #[tokio::test]
    async fn a_channel_picked_in_the_search_over_the_guide_zaps_and_closes_the_guide() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        h.type_text("test 03");
        h.sent();
        h.press(&[Key::Enter]);
        let sent = h.sent();
        assert!(
            sent.iter()
                .any(|c| matches!(c, Cmd::LoadUrl(u) if *u == live_url(3))),
            "{sent:?}"
        );
        // One load only: the guide's programme on now is the same channel, so a
        // second one would be the same URL again.
        let loads = sent.iter().filter(|c| matches!(c, Cmd::LoadUrl(_))).count();
        assert_eq!(
            loads, 1,
            "the Enter that picked reached the guide: {sent:?}"
        );
        assert_eq!(h.app.current_stream_id, Some(3));
        assert!(h.app.current_live, "live, through the stall watchdog");
        assert!(!h.app.guide_open, "the pick closes the guide");
        assert!(!h.app.show_search);
    }

    #[tokio::test]
    async fn a_series_picked_in_the_search_over_the_guide_closes_only_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        let log = LogCapture::start();
        search_over_the_guide(&mut h);
        h.type_text("zebra serie");
        h.sent();
        h.press(&[Key::Enter]);
        // Nothing plays: not the series, and not the programme under the search
        // (the Enter that picked is not the guide's too).
        assert!(!zapped(&h.sent()), "a series does not play yet");
        let toast = h.app.toast.as_ref().map(|(t, _, _)| t.as_str());
        assert_eq!(
            toast,
            Some("playing series is not available yet: Zebra Serie")
        );
        assert!(!h.app.show_search, "the search is gone");
        assert!(h.app.search_query.is_empty());
        assert!(h.app.guide_open, "but nothing played, so the guide stays");
        assert!(
            !log.text().contains("guide: closed"),
            "and no close is logged"
        );
        // The guide has its keys back at once: Down steps to the programme still
        // to come, which Enter cannot play.
        h.frame(&[Key::ArrowDown]);
        h.sent();
        h.press(&[Key::Enter]);
        assert!(!zapped(&h.sent()));
        let toast = h.app.toast.as_ref().map(|(t, _, _)| t.clone());
        assert!(toast.is_some_and(|t| t.starts_with("starts at")));
        assert!(h.app.guide_open, "and the guide is still there");
    }

    #[tokio::test]
    async fn esc_in_the_search_over_the_guide_closes_only_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        h.type_text("zeb");
        // The guide is drawn in the frame of the Esc, with its keys back: it
        // would take that Esc for its own and close (the guide has no filter).
        h.frame(&[Key::Escape]);
        assert!(!h.app.show_search, "Esc closes the search");
        assert!(h.app.search_query.is_empty(), "and wipes what was typed");
        assert!(h.app.guide_open, "the guide is still open");
        // In the very next frame the arrow is the guide's: the cursor moves to
        // the programme still to come, which Enter cannot play.
        h.frame(&[Key::ArrowDown]);
        h.sent();
        h.press(&[Key::Enter]);
        assert!(!zapped(&h.sent()));
        let toast = h.app.toast.as_ref().map(|(t, _, _)| t.clone());
        assert!(toast.is_some_and(|t| t.starts_with("starts at")));
        assert!(h.app.guide_open, "and the guide stays");
    }

    #[tokio::test]
    async fn esc_in_the_search_over_the_guide_leaves_the_guides_filter_alone() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        // A filter that keeps the column, and the box let go of again, so that
        // the next f is the search's.
        h.frame_with(vec![egui::Event::Text("/".into())]);
        h.type_text("test");
        h.press(&[Key::Enter]);
        assert!(
            h.app.guide_view.filtered().is_some(),
            "the guide is filtered"
        );
        h.frame_with(f_pressed());
        h.frame(&[]);
        assert!(h.app.show_search, "f is the search's, not the box's");
        h.frame(&[Key::Escape]);
        assert!(!h.app.show_search && h.app.guide_open);
        assert!(
            h.app.guide_view.filtered().is_some(),
            "the Esc that closed the search did not wipe the filter"
        );
        // The guide's own Esc chain is as it was: the filter, then the guide.
        h.press(&[Key::Escape]);
        assert!(
            h.app.guide_view.filtered().is_none(),
            "the next Esc wipes it"
        );
        assert!(h.app.guide_open);
        h.press(&[Key::Escape]);
        assert!(!h.app.guide_open, "and the one after closes the guide");
    }

    #[tokio::test]
    async fn while_the_search_shows_the_guide_takes_no_keys() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        // Down, g and d are the guide's keys: here they are the search's.
        h.frame(&[Key::ArrowDown]);
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        assert!(h.app.guide_open, "g does not close the guide");
        h.frame_with(vec![key_press(Key::D), egui::Event::Text("d".into())]);
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.app.search_query, "gd", "they are typed in the search box");
        assert!(
            !h.painted.iter().any(|t| t == "Yesterday"),
            "d does not open the date list: {:?}",
            h.painted
        );
        // Back in the guide the cursor is where it was: Down did not move it.
        h.frame(&[Key::Escape]);
        h.sent();
        h.press(&[Key::Enter]);
        let sent = h.sent();
        assert!(zapped(&sent), "Enter plays the programme on now: {sent:?}");
    }

    #[tokio::test]
    async fn question_mark_and_o_typed_in_the_search_over_the_guide_are_characters() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        // ? asks for the help and o for the settings, in the guide and in the
        // viewer; not here, where the search box has the keyboard.
        let question = egui::Event::Key {
            key: Key::Questionmark,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        let typed_question = vec![question, egui::Event::Text("?".into())];
        h.frame_holding(egui::Modifiers::SHIFT, typed_question);
        h.frame_with(vec![key_press(Key::O), egui::Event::Text("o".into())]);
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.app.search_query, "?o", "typed in the search box");
        assert!(!h.app.show_help, "? is a character, not the help");
        assert!(
            h.app.settings_ui.is_none(),
            "o is a character, not the settings"
        );
        assert!(
            h.app.show_search && h.app.guide_open,
            "and the search is still over the guide"
        );
    }

    #[tokio::test]
    async fn while_the_search_shows_a_click_on_the_guide_plays_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        h.sent();
        // The programme on now, in the first column: not under the search.
        let on_now = egui::pos2(60.0, 325.0);
        click_at(&mut h, on_now);
        assert!(!zapped(&h.sent()), "no guide programme was played");
        assert!(h.app.guide_open && h.app.show_search, "and nothing closed");
        // The same click is a pick once the search is gone.
        h.frame(&[Key::Escape]);
        click_at(&mut h, on_now);
        assert!(
            zapped(&h.sent()),
            "the click does play it without the search"
        );
    }

    #[tokio::test]
    async fn an_f_typed_in_the_search_over_the_guide_is_a_letter() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        search_over_the_guide(&mut h);
        h.frame_with(f_pressed());
        assert_eq!(h.app.search_query, "f");
        assert!(h.app.show_search && h.app.guide_open);
    }

    #[tokio::test]
    async fn outside_the_guide_f_opens_the_search_and_esc_closes_it_as_ever() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        h.frame_with(f_pressed());
        assert!(h.app.show_search && !h.app.guide_open);
        assert!(h.app.search_query.is_empty(), "the f is not typed");
        h.frame(&[]);
        h.type_text("zeb");
        assert_eq!(h.app.search_query, "zeb");
        h.frame(&[Key::Escape]);
        assert!(!h.app.show_search && !h.app.guide_open);
        assert!(h.app.search_query.is_empty());
    }

    /// A press of Shift+F as a window reports it: the key with Shift in its own
    /// copy of the modifiers, and the capital it types.
    fn shift_f_pressed() -> Vec<egui::Event> {
        let shift_f = egui::Event::Key {
            key: Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::SHIFT,
        };
        vec![shift_f, egui::Event::Text("F".into())]
    }

    #[tokio::test]
    async fn outside_the_guide_shift_f_is_still_the_favourites_and_not_the_search() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        h.frame_holding(egui::Modifiers::SHIFT, shift_f_pressed());
        assert!(h.app.show_favs, "Shift+F lists the favourites");
        assert!(!h.app.show_search, "and is not the search");
    }

    #[tokio::test]
    async fn shift_f_in_the_guide_opens_neither_the_search_nor_the_favourites() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        h.frame_holding(egui::Modifiers::SHIFT, shift_f_pressed());
        h.frame(&[]);
        h.frame(&[]);
        assert!(!h.app.show_search, "Shift+F is not the search's");
        assert!(
            !h.app.show_favs,
            "the favourites do not show over the guide"
        );
        assert!(h.app.guide_open, "and the guide is as it was");
        // The keyboard did not go to the guide's filter box either: what is typed
        // now filters nothing.
        h.type_text("zz");
        assert!(
            h.app.guide_view.filtered().is_none(),
            "the box has no keyboard"
        );
        // A plain f is still the search.
        h.frame_with(f_pressed());
        assert!(h.app.show_search && h.app.guide_open);
    }

    #[tokio::test]
    async fn ctrl_f_in_the_guide_opens_the_search_like_f() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), library_catalog(), PortalState::Configured, None);
        open_guide(&mut h, true);
        let ctrl_f = egui::Event::Key {
            key: Key::F,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::CTRL,
        };
        h.frame_holding(egui::Modifiers::CTRL, vec![ctrl_f]);
        assert!(
            h.app.show_search && h.app.guide_open,
            "Ctrl+F opens the search"
        );
        assert!(h.app.search_query.is_empty());
        let left = h.ctx.input(|i| i.events.clone());
        assert!(left.is_empty(), "and its press is spent: {left:?}");
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
        let ago = Instant::now()
            .checked_sub(Duration::from_millis(1100))
            .unwrap();
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
    async fn a_frame_of_the_channel_before_that_slips_in_after_the_pick_never_shows_at_the_cap() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), guide_catalog(), PortalState::Configured, None);
        // The channel watched before the pick: its last picture is on screen.
        h.put_frame(4, 2);
        h.frame(&[]);
        assert!(h.app.video_tex.is_some(), "a live picture");
        pick_catch_up(&mut h);
        assert!(h.app.video_tex.is_none(), "the pick takes the picture away");
        // mpv renders one more frame of that channel while the load command is on
        // its way: newer than the bus was at the pick, so the fence lets it through
        // and is gone. The wait screen hides it.
        h.put_frame(6, 3);
        h.frame(&[]);
        assert_eq!(
            h.app.video_tex.as_ref().map(|t| t.size()),
            Some([6, 3]),
            "the fence let it through"
        );
        assert!(h.painted.iter().any(|t| t == "loading the programme…"));
        // The restart, and a second later no picture of the programme has come.
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_some(), "the wait goes on");
        let ago = Instant::now()
            .checked_sub(Duration::from_millis(1100))
            .unwrap();
        let wait = h.app.pending_catchup.as_mut().unwrap();
        let restart = wait.started.expect("the restart is noted").0;
        wait.started = Some((restart, ago));
        h.frame(&[]);
        assert!(h.app.pending_catchup.is_none(), "the cap ends the wait");
        assert!(h.app.video_tex.is_none(), "the old picture is gone");
        assert!(
            h.painted.iter().any(|t| t == crate::APP_NAME),
            "the empty state shows: {:?}",
            h.painted
        );
        // The straggler is no newer than the restart: it does not come back.
        for _ in 0..2 {
            h.frame(&[]);
            assert!(h.app.video_tex.is_none(), "the old picture came back");
            assert!(
                h.painted.iter().any(|t| t == crate::APP_NAME),
                "the empty state stays: {:?}",
                h.painted
            );
        }
        // A picture newer than the restart still shows.
        h.put_frame(8, 4);
        h.frame(&[]);
        let video = h.app.video_tex.as_ref().expect("a new picture shows");
        assert_eq!(video.size(), [8, 4], "its texture holds the new picture");
        assert!(h.drawn.contains(&video.id()), "and it is what is drawn");
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
        // A stream error names what played, a catch-up too (its label is the
        // channel and the programme).
        let retry = "stream error on TEST 03 - Journaal -1 - zap up or down to retry";
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
        let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
        let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
            Self::start_at(tracing::Level::INFO)
        }

        /// With the debug lines too.
        fn start_debug() -> Self {
            Self::start_at(tracing::Level::DEBUG)
        }

        fn start_at(level: tracing::Level) -> Self {
            let second = tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default());
            let buffer = LogBuffer::default();
            let sink = buffer.clone();
            let subscriber = tracing_subscriber::fmt()
                .with_writer(move || sink.clone())
                .with_ansi(false)
                .with_max_level(level)
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
        // The guide's filter box has `/`; `f` is the search's.
        h.frame_with(vec![egui::Event::Text("/".into())]);
        h.frame(&[]);
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
        let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
        let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
            let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
            let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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
            let saved = h.app.keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"));
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

    // ---- the right mouse button moves the borderless window ----

    /// The right mouse button going down or up.
    fn right_button(pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: egui::pos2(640.0, 360.0),
            button: egui::PointerButton::Secondary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }

    /// The app has not moved the window.
    const NO_MOVES: [egui::Pos2; 0] = [];

    /// A window of 1280 x 720 points with its outer corner at `corner`.
    fn window_rect(corner: egui::Pos2) -> egui::Rect {
        egui::Rect::from_min_size(corner, egui::vec2(1280.0, 720.0))
    }

    /// The app, with its window at (`x`, `y`) points on a screen of `scale`
    /// pixels to a point, and the border still on.
    fn app_with_window(x: f32, y: f32, scale: f32) -> (tempfile::TempDir, Harness) {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.viewport.outer_rect = Some(window_rect(egui::pos2(x, y)));
        h.viewport.native_pixels_per_point = Some(scale);
        h.frame(&[]);
        (dir, h)
    }

    /// `app_with_window` with the border off (the `t` key).
    fn borderless_app_with_window(x: f32, y: f32, scale: f32) -> (tempfile::TempDir, Harness) {
        let (dir, mut h) = app_with_window(x, y, scale);
        h.press(&[Key::T]);
        assert!(h.app.borderless, "the t key took the border off");
        (dir, h)
    }

    /// The right button goes down with the cursor at `from`, the cursor goes on to
    /// `to` a frame later, and the button goes up: where the app moved the window.
    fn drag_with_the_right_button(
        h: &mut Harness,
        from: ScreenPos,
        to: ScreenPos,
    ) -> Vec<egui::Pos2> {
        h.cursor_to(from.0, from.1);
        h.frame_with(vec![right_button(true)]);
        h.cursor_to(to.0, to.1);
        h.frame(&[]);
        h.frame_with(vec![right_button(false)]);
        h.moved()
    }

    /// A drag under way in a borderless window at (300, 200) points, scale 1: the
    /// button went down at pixel (500, 400), the cursor went on to (600, 450), the
    /// window moved to (400, 250), and the button is still down.
    fn start_dragging(h: &mut Harness) {
        h.cursor_to(500, 400);
        h.frame_with(vec![right_button(true)]);
        h.cursor_to(600, 450);
        h.frame(&[]);
        assert_eq!(
            h.moved(),
            [egui::pos2(400.0, 250.0)],
            "the drag is under way"
        );
    }

    /// The cursor goes on moving, two frames' worth, and the window stays where it
    /// is (two, so that a drag that began in the first would show in the second).
    fn assert_the_window_stays_put(h: &mut Harness, why: &str) {
        for (x, y) in [(700, 500), (800, 600)] {
            h.cursor_to(x, y);
            h.frame(&[]);
            assert_eq!(h.moved(), NO_MOVES, "{why}, the cursor at ({x}, {y})");
        }
    }

    #[tokio::test]
    async fn the_right_button_moves_the_borderless_window_as_far_as_the_cursor_moves_on_the_screen()
    {
        // The cursor moves 100 pixels right and 50 down, then 60 left and 30 up
        // of where it began. (scale, the window's first place, its second)
        for (scale, first, second) in [
            (1.0, egui::pos2(400.0, 250.0), egui::pos2(240.0, 170.0)),
            (
                1.5,
                egui::pos2(300.0 + 100.0 / 1.5, 200.0 + 50.0 / 1.5),
                egui::pos2(300.0 - 60.0 / 1.5, 200.0 - 30.0 / 1.5),
            ),
        ] {
            let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, scale);
            h.cursor_to(500, 400);
            h.frame_with(vec![right_button(true)]);
            assert_eq!(
                h.moved(),
                NO_MOVES,
                "scale {scale}: the press moves nothing"
            );
            for (cursor, goes_to) in [((600, 450), first), ((440, 370), second)] {
                h.cursor_to(cursor.0, cursor.1);
                h.frame(&[]);
                let moved = h.moved();
                assert_eq!(
                    moved.len(),
                    1,
                    "scale {scale}, cursor {cursor:?}: {moved:?}"
                );
                assert!(
                    (moved[0] - goes_to).length() < 0.01,
                    "scale {scale}, cursor {cursor:?}: {:?}, not {goes_to:?}",
                    moved[0]
                );
                // The cursor stands still, the button stays down: nothing more.
                h.frame(&[]);
                h.frame(&[]);
                assert_eq!(
                    h.moved(),
                    NO_MOVES,
                    "scale {scale}: the cursor did not move"
                );
            }
        }
    }

    #[tokio::test]
    async fn the_window_keeps_up_with_the_cursor_though_the_pointer_in_it_shifts_as_it_moves() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        // The window moves at once when told to, but reports its place (`outer_rect`)
        // a frame late: `stale` is where it was a frame ago.
        let mut really = egui::pos2(300.0, 200.0);
        let mut stale = really;
        // The pointer in the window is the cursor on the screen minus the window's
        // corner: it changes whenever the window moves, so a drag that went by it
        // would chase its own tail, and one that went by the window's place in
        // every frame would be a frame behind.
        let pointer = |really: egui::Pos2, x: i32, y: i32| {
            egui::Event::PointerMoved(egui::pos2(x as f32, y as f32) - really.to_vec2())
        };
        h.cursor_to(500, 400);
        h.viewport.outer_rect = Some(window_rect(stale));
        h.frame_with(vec![pointer(really, 500, 400), right_button(true)]);
        for (x, y) in [
            (520, 410),
            (560, 420),
            (560, 420),
            (640, 480),
            (640, 480),
            (600, 430),
        ] {
            h.cursor_to(x, y);
            h.viewport.outer_rect = Some(window_rect(stale));
            h.frame_with(vec![pointer(really, x, y)]);
            stale = really;
            if let Some(&to) = h.moved().last() {
                really = to;
            }
            assert_eq!(
                really,
                egui::pos2(300.0 + (x - 500) as f32, 200.0 + (y - 400) as f32),
                "the cursor at ({x}, {y})"
            );
        }
    }

    #[tokio::test]
    async fn a_window_with_a_border_is_not_moved_by_the_right_button() {
        let (_dir, mut h) = app_with_window(300.0, 200.0, 1.0);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, NO_MOVES, "with a border");
        assert_eq!(CURSOR_READS.get(), 0, "the cursor was not even asked for");
        assert_eq!(
            (KEY_POLLS.get(), SWAP_READS.get()),
            (0, 0),
            "nor the buttons"
        );
        // The very same gesture moves the window once the border is gone ...
        h.press(&[Key::T]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)], "borderless");
        // ... and no more once it is back.
        h.press(&[Key::T]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, NO_MOVES, "with the border back");
    }

    #[tokio::test]
    async fn the_left_and_the_middle_button_do_not_drag_the_window() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        for button in [egui::PointerButton::Primary, egui::PointerButton::Middle] {
            let event = |pressed| egui::Event::PointerButton {
                pos: egui::pos2(640.0, 360.0),
                button,
                pressed,
                modifiers: egui::Modifiers::NONE,
            };
            h.cursor_to(500, 400);
            h.frame_with(vec![event(true)]);
            h.cursor_to(600, 450);
            h.frame(&[]);
            h.frame_with(vec![event(false)]);
            assert_eq!(h.moved(), NO_MOVES, "{button:?}");
        }
        assert_eq!(CURSOR_READS.get(), 0, "the cursor was not even asked for");
        // The right button, in the same place, does.
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)], "the right button");
    }

    #[tokio::test]
    async fn a_press_while_the_window_does_not_know_its_place_begins_no_drag() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        h.viewport.outer_rect = None;
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, NO_MOVES, "no place");
        h.viewport.outer_rect = Some(window_rect(egui::pos2(300.0, 200.0)));
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)], "place known");
    }

    #[tokio::test]
    async fn letting_go_of_the_right_button_ends_the_drag() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)]);
        // The button is up and the cursor goes on: the window stays.
        h.cursor_to(900, 700);
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.moved(), NO_MOVES, "after the release");
        // The next press begins a drag of its own, from where the cursor is now.
        h.frame_with(vec![right_button(true)]);
        h.cursor_to(950, 720);
        h.frame(&[]);
        assert_eq!(h.moved(), [egui::pos2(350.0, 220.0)], "a new drag");
    }

    #[tokio::test]
    async fn nothing_moves_in_full_screen() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        h.viewport.fullscreen = Some(true);
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, NO_MOVES, "in full screen");
        h.viewport.fullscreen = Some(false);
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)], "out of full screen");
    }

    #[tokio::test]
    async fn a_drag_ends_when_the_window_goes_full_screen_and_stays_ended() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        h.viewport.fullscreen = Some(true);
        assert_the_window_stays_put(&mut h, "in full screen");
        // Out of it, the button still down: a drag begins with a press.
        h.viewport.fullscreen = Some(false);
        assert_the_window_stays_put(&mut h, "out of full screen, no new press");
    }

    #[tokio::test]
    async fn f11_ends_a_drag_in_the_frame_that_goes_full_screen() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        // F11 and a move of the cursor in one frame. The window does not report
        // full screen before the next frame, and a move sent after the Fullscreen
        // command would leave the monitor-sized window at the old window's corner.
        h.cursor_to(700, 500);
        h.frame(&[Key::F11]);
        let told = std::mem::take(&mut h.window_cmds);
        assert!(
            told.contains(&egui::ViewportCommand::Fullscreen(true)),
            "{told:?}"
        );
        assert!(
            !told
                .iter()
                .any(|cmd| matches!(cmd, egui::ViewportCommand::OuterPosition(_))),
            "{told:?}"
        );
        // The button is still down, the window does not say yet that it is full
        // screen: a drag begins with a press.
        assert_the_window_stays_put(&mut h, "after F11");
    }

    #[tokio::test]
    async fn nothing_moves_in_a_maximized_window() {
        // Win+Up: the first move would restore it, and the window would jump.
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        h.viewport.maximized = Some(true);
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, NO_MOVES, "maximized");
        h.viewport.maximized = Some(false);
        h.frame(&[]);
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(400.0, 250.0)], "restored");
    }

    #[tokio::test]
    async fn a_drag_ends_when_the_window_is_maximized_and_stays_ended() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        h.viewport.maximized = Some(true);
        assert_the_window_stays_put(&mut h, "maximized");
        // Restored, the button still down: a drag begins with a press.
        h.viewport.maximized = Some(false);
        assert_the_window_stays_put(&mut h, "restored, no new press");
    }

    #[tokio::test]
    async fn a_drag_ends_when_the_window_loses_the_focus_and_stays_ended() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        // The release goes to whatever has the focus now, egui never sees it.
        h.focused = false;
        assert_the_window_stays_put(&mut h, "without the focus");
        h.focused = true;
        assert_the_window_stays_put(&mut h, "focus back, no new press");
    }

    #[tokio::test]
    async fn a_drag_ends_when_the_border_comes_back_and_stays_ended() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        h.press(&[Key::T]);
        assert_the_window_stays_put(&mut h, "with the border back");
        // Borderless again, the button still down: a drag begins with a press.
        h.press(&[Key::T]);
        assert_the_window_stays_put(&mut h, "borderless again, no new press");
    }

    #[tokio::test]
    async fn a_cursor_windows_will_not_give_leaves_the_window_where_it_is() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        CURSOR.set(None);
        h.frame(&[]);
        assert_eq!(h.moved(), NO_MOVES, "no cursor");
        // It is back, a little further: the drag goes on from where it began.
        h.cursor_to(650, 480);
        h.frame(&[]);
        assert_eq!(h.moved(), [egui::pos2(450.0, 280.0)]);
    }

    #[tokio::test]
    async fn a_release_egui_never_saw_still_ends_the_drag() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        start_dragging(&mut h);
        // The button is let go where egui does not hear of it (another window took
        // the mouse capture): Windows says it is up, egui says it is down.
        h.physical_buttons(false, false);
        assert_the_window_stays_put(&mut h, "Windows says the button is up");
        // Down again, with no press egui saw: a drag begins with a press.
        h.physical_buttons(false, true);
        assert_the_window_stays_put(&mut h, "down again, no new press");
    }

    #[tokio::test]
    async fn the_drag_watches_the_physical_button_that_is_the_right_one() {
        // Swapped buttons (the left-handed setting) make the physical left button
        // the right one for egui. (swapped, left down, right down, the window moves)
        for (swapped, left, right, moves) in [
            (false, false, true, true),
            (false, true, false, false),
            (true, true, false, true),
            (true, false, true, false),
        ] {
            let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
            h.buttons_swapped(swapped);
            h.physical_buttons(left, right);
            let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
            let want = if moves {
                vec![egui::pos2(400.0, 250.0)]
            } else {
                Vec::new()
            };
            assert_eq!(
                moved, want,
                "swapped: {swapped}, left down: {left}, right down: {right}"
            );
        }
    }

    #[tokio::test]
    async fn a_zoomed_ui_does_not_scale_the_drag() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        // The UI zoom (Ctrl +) doubles the size of a point: it is two pixels now,
        // and the window, at pixel (600, 400), is at point (300, 200) still.
        h.ctx.set_zoom_factor(2.0);
        h.frame(&[]);
        assert_eq!(h.ctx.pixels_per_point(), 2.0);
        // The cursor's 100 pixels right and 50 down are 50 and 25 points.
        let moved = drag_with_the_right_button(&mut h, (500, 400), (600, 450));
        assert_eq!(moved, [egui::pos2(350.0, 225.0)]);
    }

    #[tokio::test]
    async fn a_drag_costs_cursor_reads_and_quick_frames_only_while_it_lasts() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        // Borderless but no button: the app's own pace, and the cursor is not asked.
        for _ in 0..12 {
            h.frame(&[]);
        }
        h.frame_with(vec![egui::Event::PointerMoved(egui::pos2(100.0, 100.0))]);
        for _ in 0..12 {
            h.frame(&[]);
        }
        assert_eq!(CURSOR_READS.get(), 0, "idle: the cursor is not asked for");
        assert_eq!(
            (KEY_POLLS.get(), SWAP_READS.get()),
            (0, 0),
            "idle: nor the buttons"
        );
        assert!(h.repaint_in > Duration::ZERO, "idle: {:?}", h.repaint_in);
        // The button down: the cursor is read, and the next frame is asked for at
        // once, the cursor moving or standing still.
        h.cursor_to(500, 400);
        h.frame_with(vec![right_button(true)]);
        for i in 0..4 {
            h.cursor_to(510 + i, 400);
            h.frame(&[]);
        }
        for _ in 0..4 {
            h.frame(&[]);
        }
        assert!(CURSOR_READS.get() >= 8, "reads: {}", CURSOR_READS.get());
        // Whether the buttons are swapped is read once, when the drag begins; the
        // button is asked about once in each frame after that.
        assert_eq!(
            (KEY_POLLS.get(), SWAP_READS.get()),
            (8, 1),
            "dragging: button polls, swap reads"
        );
        assert_eq!(h.repaint_in, Duration::ZERO, "dragging");
        // Let go: back to idle, with no more reads.
        h.frame_with(vec![right_button(false)]);
        for _ in 0..12 {
            h.frame(&[]);
        }
        let reads = CURSOR_READS.get();
        h.frame(&[]);
        assert_eq!(CURSOR_READS.get(), reads, "after the release");
        assert_eq!(
            (KEY_POLLS.get(), SWAP_READS.get()),
            (8, 1),
            "after the release: no more asking"
        );
        assert!(h.repaint_in > Duration::ZERO, "after: {:?}", h.repaint_in);
    }

    #[test]
    fn a_drag_says_in_the_log_when_it_begins_and_why_it_ends_and_never_per_frame() {
        let (_dir, mut h) = borderless_app_with_window(300.0, 200.0, 1.0);
        let log = LogCapture::start_debug();
        // Idle frames say nothing.
        for _ in 0..6 {
            h.frame(&[]);
        }
        // A drag of many frames, ended by the release.
        h.cursor_to(500, 400);
        h.frame_with(vec![right_button(true)]);
        h.cursor_to(600, 450);
        for _ in 0..6 {
            h.frame(&[]);
        }
        h.frame_with(vec![right_button(false)]);
        // The moves are not what this test is about.
        h.moved();
        // Ended by full screen.
        start_dragging(&mut h);
        h.viewport.fullscreen = Some(true);
        h.frame(&[]);
        h.viewport.fullscreen = Some(false);
        h.frame_with(vec![right_button(false)]);
        // Ended by the window being maximized.
        start_dragging(&mut h);
        h.viewport.maximized = Some(true);
        h.frame(&[]);
        h.viewport.maximized = Some(false);
        h.frame_with(vec![right_button(false)]);
        // Ended by the focus going.
        start_dragging(&mut h);
        h.focused = false;
        h.frame(&[]);
        h.focused = true;
        h.frame_with(vec![right_button(false)]);
        // Ended by Windows saying the button is up, which egui never heard.
        start_dragging(&mut h);
        h.physical_buttons(false, false);
        h.frame(&[]);
        h.physical_buttons(false, true);
        h.frame_with(vec![right_button(false)]);
        // Ended by F11.
        start_dragging(&mut h);
        h.frame(&[Key::F11]);
        h.frame_with(vec![right_button(false)]);
        // Ended by the border coming back.
        start_dragging(&mut h);
        h.press(&[Key::T]);
        let text = log.text();
        let said: Vec<&str> = text
            .lines()
            .filter(|line| line.contains("window drag"))
            .map(|line| match line.rsplit_once("ended (") {
                Some((_, why)) => why.trim_end_matches(')'),
                None if line.contains("begins") => "begins",
                None => line,
            })
            .collect();
        assert_eq!(
            said,
            [
                "begins",
                "released",
                "begins",
                "full screen",
                "begins",
                "maximized",
                "begins",
                "focus lost",
                "begins",
                "button up per Win32",
                "begins",
                "F11",
                "begins",
                "border back"
            ],
            "{text}"
        );
    }

    // ---- an M3U source ----

    #[tokio::test]
    async fn an_m3u_channel_plays_straight_from_its_server_with_its_headers_and_the_next_one_without(
    ) {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::start(crate::relay::RelayConfig::default()).unwrap();
        let mut h = m3u_app_with(dir.path(), TEST_LIST, Some(relay));
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.sent();
        h.press(&[Key::ArrowDown]); // the second channel: Kanaal Twee, with headers
        h.press(&[Key::ArrowDown]); // the third: Kanaal Twee on another server, none
        assert_eq!(
            loads(h.sent()),
            [
                "https://b.example/twee.m3u8|User-Agent=Bongo%2F1.0&Referer=https%3A%2F%2Fwww.example.com%2F",
                "https://c.example/twee-nieuws.m3u8",
            ],
            "straight from the servers, not through the relay"
        );
        assert!(h.app.current_live, "the stall watchdog watches it");
    }

    #[tokio::test]
    async fn an_xtream_live_channel_still_goes_through_the_relay() {
        let dir = tempfile::tempdir().unwrap();
        let relay = Relay::start(crate::relay::RelayConfig::default()).unwrap();
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::default(),
            Some(relay),
        );
        h.sent();
        h.press(&[Key::ArrowDown]);
        let loads = loads(h.sent());
        assert_eq!(loads.len(), 1, "{loads:?}");
        assert!(loads[0].starts_with("http://127.0.0.1:"), "{loads:?}");
    }

    #[tokio::test]
    async fn an_m3u_channel_hands_mpv_its_headers_as_options_of_that_one_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.sent();
        h.press(&[Key::ArrowDown]); // Kanaal Twee, with a User-Agent and a Referer
        let sent = h.sent();
        let load = sent.iter().find_map(crate::player::loadfile_command);
        let (args, line) = load.expect("a load");
        assert_eq!(
            args,
            [
                "loadfile",
                "https://b.example/twee.m3u8",
                "replace",
                "-1",
                "user-agent=%9%Bongo/1.0,http-header-fields=%33%Referer: https://www.example.com/",
            ]
        );
        assert_eq!(
            line,
            "loadfile https://b.example/twee.m3u8 (user-agent, referrer)"
        );
    }

    #[tokio::test]
    async fn an_xtream_film_with_a_pipe_in_the_password_reaches_mpv_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let creds = XtreamCreds {
            password: "pa|ss".into(),
            ..test_creds()
        };
        let portal = Arc::new(XtreamPortal::new(creds.clone()));
        let catalog = Arc::new(CatalogStore::from_catalog(portal, adult_catalog()));
        let mut h = Harness::start_with(
            dir.path(),
            catalog,
            PortalState::Configured,
            None,
            Some(creds),
            Sources::default(),
            None,
        );
        h.sent();
        h.app.play_movie(9, "Film");
        let sent = h.sent();
        let load = sent.iter().find_map(crate::player::loadfile_command);
        let (args, _) = load.expect("a load");
        // Byte for byte, as 0.6.1 sent it.
        assert_eq!(args, ["loadfile", "http://h:80/movie/u/pa|ss/9.mkv"]);
    }

    #[tokio::test]
    async fn an_m3u_list_is_searched_and_zapped_as_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        let zap: Vec<String> = h
            .app
            .catalog
            .zap_channels()
            .into_iter()
            .map(|c| c.name)
            .collect();
        assert_eq!(
            zap,
            ["Kanaal Een", "Kanaal Twee", "Kanaal Twee"],
            "nothing merged"
        );
        h.frame_with(vec![key_press(Key::F), egui::Event::Text("f".into())]);
        h.frame(&[]);
        h.type_text("een");
        h.sent();
        h.press(&[Key::Enter]);
        assert_eq!(loads(h.sent()), ["https://a.example/een.m3u8"]);
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal Een"));
    }

    #[tokio::test]
    async fn a_channel_the_list_no_longer_has_loads_nothing_and_is_not_the_last_channel() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.press(&[Key::ArrowDown]); // Kanaal Twee plays
        h.sent();
        let before = read_last_watched(dir.path()).url;
        // The zap pacer held a channel that the fresh list dropped meanwhile:
        // the list has no address for it.
        h.app.zap_to(12345, "Weg", Some(0));
        assert!(loads(h.sent()).is_empty(), "nothing loads");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal Twee"));
        assert_eq!(read_last_watched(dir.path()).url, before);
    }

    #[test]
    fn an_empty_address_is_never_recorded_as_the_last_channel() {
        let (portal, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let m3u = CatalogStore::from_catalog(Arc::new(portal), catalog);
        let xtream = CatalogStore::from_catalog(idle_portal(), adult_catalog());
        for store in [&m3u, &xtream] {
            for hint in [None, Some(false)] {
                let record = last_watched_record(store, "", true, hint, false);
                assert!(record.is_none(), "{:?}", record.map(|r| r.url));
            }
        }
    }

    #[tokio::test]
    async fn a_dead_m3u_channel_names_itself_and_the_app_stays_on_it() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.press(&[Key::ArrowDown]);
        h.sent();
        h.events
            .send(Event::EndOfFile {
                reason: "error".into(),
            })
            .unwrap();
        h.frame(&[]);
        assert!(
            h.painted
                .iter()
                .any(|t| t == "stream error on Kanaal Twee - zap up or down to retry"),
            "{:?}",
            h.painted
        );
        for _ in 0..5 {
            h.frame(&[]);
        }
        assert!(loads(h.sent()).is_empty(), "it stays on the channel");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal Twee"));
    }

    #[tokio::test]
    async fn g_on_an_m3u_source_says_there_is_no_guide_yet() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        h.frame(&[]);
        assert!(!h.app.guide_open);
        assert!(
            h.painted
                .iter()
                .any(|t| t == "this source has no guide yet"),
            "{:?}",
            h.painted
        );
    }

    #[tokio::test]
    async fn unlocking_changes_nothing_on_an_m3u_source() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        let before = h.app.catalog.zap_channels().len();
        h.app.catalog.set_adult_unlocked(true);
        assert_eq!(h.app.catalog.zap_channels().len(), before);
        h.press(&[Key::ArrowDown]);
        assert!(!h.app.playing_adult());
        // Known before any list has loaded: a resume never waits for one.
        let store = CatalogStore::new(Arc::new(M3uPortal::from_text(TEST_LIST).unwrap().0));
        assert_eq!(
            catalog_says_adult(&store, "http://h/live/u/p/3.m3u8"),
            Some(false)
        );
    }

    #[tokio::test]
    async fn up_after_a_resume_continues_from_the_resumed_m3u_channel() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        // What the last start played: Kanaal Twee, headers and all.
        let (known, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let twee = known.live_stream_url(catalog.live[1].stream_id);
        let een = known.live_stream_url(catalog.live[0].stream_id);
        let record = LastWatched {
            url: twee.clone(),
            live: true,
            adult: Some(false),
        };
        record.save(&Storage::with_root(dir.path().to_path_buf()).last_watched_path());
        // A list not read yet: the resume cannot know the channel's id.
        let portal = M3uPortal::new(1, &file.display().to_string(), dir.path().join("m3u-1.m3u"));
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        let mut h = Harness::start_with(
            dir.path(),
            store.clone(),
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        assert_eq!(loads(h.sent()), [twee], "the resume plays at once");
        assert_eq!(h.app.current_stream_id, None);
        store.spawn_fetch();
        wait_until("the list", || store.is_loaded()).await;
        h.frame(&[]);
        h.press(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), [een], "Kanaal Een, the one before Twee");
    }

    #[tokio::test]
    async fn zapping_after_the_list_changed_under_it_goes_on_from_the_channel_that_plays() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        // Kanaal Twee, at 1.
        h.press(&[Key::ArrowDown]);
        // The fresh copy of the list puts a new channel first: Twee is at 2 now.
        let fresh = format!(
            "#EXTM3U\n#EXTINF:-1,Nieuw\nhttps://n.example/nieuw.m3u8\n{}",
            &TEST_LIST["#EXTM3U\n".len()..]
        );
        let (portal, catalog) = M3uPortal::from_text(&fresh).unwrap();
        h.app.catalog = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        h.sent();
        h.press(&[Key::ArrowDown]);
        assert_eq!(
            loads(h.sent()),
            ["https://c.example/twee-nieuws.m3u8"],
            "the one after Twee, not the one after place 1"
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_says_the_list_was_not_updated_and_keeps_it() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(503))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("m3u-2.m3u");
        std::fs::write(&cache, TEST_LIST).unwrap();
        let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        store.spawn_fetch();
        let mut h = Harness::start_with(
            dir.path(),
            store.clone(),
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        let said = |h: &Harness| h.painted.iter().any(|t| t == "channel list not updated");
        for _ in 0..400 {
            h.frame(&[]);
            if said(&h) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(said(&h), "{:?}", h.painted);
        assert_eq!(
            h.app.catalog.zap_channels().len(),
            3,
            "the saved list stays"
        );
    }

    #[tokio::test]
    async fn an_m3u_list_that_never_came_shows_the_empty_state_in_the_lists_words() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("weg.m3u");
        let portal = M3uPortal::new(
            2,
            &missing.display().to_string(),
            dir.path().join("m3u-2.m3u"),
        );
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        store.spawn_fetch();
        wait_until("the failed read", || {
            matches!(store.status(), CatalogStatus::Failed(_))
        })
        .await;
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        h.frame(&[]);
        assert!(
            h.painted
                .iter()
                .any(|t| t == "fetching the channel list failed"),
            "{:?}",
            h.painted
        );
        assert!(
            h.painted.iter().any(|t| t.contains("weg.m3u")),
            "{:?}",
            h.painted
        );
    }

    #[tokio::test]
    async fn f5_on_an_m3u_source_fetches_the_list_again() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.press(&[Key::F5]);
        assert!(
            h.painted
                .iter()
                .any(|t| t == "fetching the channel list..."),
            "{:?}",
            h.painted
        );
    }

    #[test]
    fn the_pin_reset_takes_the_first_saved_portals_password_on_an_m3u_source() {
        let dir = tempfile::tempdir().unwrap();
        let (portal, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        let sources = Sources::parse(
            "source=1;xtream;;u:geheim@h:80\nsource=2;m3u;Gratis;https://a.example/tv.m3u\nactive=2\n",
        );
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            sources,
            None,
        );
        h.app.settings.pin = Some(PinHash::new("1234").unwrap());
        h.app
            .apply_settings_action(SettingsAction::ResetPin("fout".into()));
        assert_eq!(h.app.adult_note, Some(T::WrongPortalPassword));
        h.app
            .apply_settings_action(SettingsAction::ResetPin("geheim".into()));
        assert!(h.app.settings.pin.is_none());
    }

    #[test]
    fn the_pin_reset_has_no_password_to_check_when_no_portal_is_saved_either() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.app.settings.pin = Some(PinHash::new("1234").unwrap());
        // An empty password must not match the empty "password" of no portal.
        h.app
            .apply_settings_action(SettingsAction::ResetPin(String::new()));
        assert_eq!(h.app.adult_note, Some(T::WrongPortalPassword));
        assert!(h.app.settings.pin.is_some());
    }

    #[tokio::test]
    async fn an_html_page_instead_of_a_list_says_it_is_no_channel_list_and_keeps_the_saved_one() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string("<html><body>Please log in</body></html>"),
            )
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("m3u-2.m3u");
        std::fs::write(&cache, TEST_LIST).unwrap();
        let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        store.spawn_fetch();
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            None,
        );
        let said = |h: &Harness| h.painted.iter().any(|t| t == "this is not a channel list");
        for _ in 0..400 {
            h.frame(&[]);
            if said(&h) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        assert!(said(&h), "{:?}", h.painted);
        assert_eq!(
            h.app.catalog.zap_channels().len(),
            3,
            "the saved list stays"
        );
    }

    #[tokio::test]
    async fn an_m3u_zap_asks_for_no_epg_and_the_guide_never_starts_fetching() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.press(&[Key::ArrowDown]);
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal Twee"));
        assert_eq!(h.app.epg_fetch_pending_for, None, "no request for the zap");
        h.frame_with(vec![key_press(Key::G), egui::Event::Text("g".into())]);
        h.frame(&[]);
        assert!(!h.app.guide_fetching, "and the guide asked for nothing");
        assert!(h.app.epg_fetcher.in_flight().is_empty());
    }

    #[tokio::test]
    async fn plus_and_minus_on_an_m3u_source_say_there_is_no_other_quality() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.press(&[Key::ArrowDown]);
        h.sent();
        for key in ["+", "-"] {
            h.frame_with(vec![egui::Event::Text(key.into())]);
            h.frame(&[]);
            assert!(
                h.painted.iter().any(|t| t == "quality: no other variant"),
                "{key}: {:?}",
                h.painted
            );
        }
        assert!(loads(h.sent()).is_empty(), "nothing else loads");
    }

    #[tokio::test]
    async fn a_load_mpv_refuses_stops_the_stream_before_it_and_names_the_channel() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app(dir.path(), TEST_LIST);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.press(&[Key::ArrowDown]);
        h.sent();
        // mpv could not take the load (a NUL byte in a header of the list): the
        // stream before it was never replaced, and plays on.
        let error = |msg: &str| Event::Error { msg: msg.into() };
        h.events
            .send(error("mpv error -4: an argument holds a NUL byte"))
            .unwrap();
        h.frame(&[]);
        let named = "stream error on Kanaal Twee - zap up or down to retry";
        assert!(h.painted.iter().any(|t| t == named), "{:?}", h.painted);
        assert!(
            h.sent().iter().any(|c| matches!(c, Cmd::Stop)),
            "the stream before it stops"
        );
        assert!(!h.app.stall_watchdog_armed, "and no reload is tried");
        assert_eq!(h.app.current_name.as_deref(), Some("Kanaal Twee"));
        // Zapping goes on from the channel it stays on.
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), ["https://c.example/twee-nieuws.m3u8"]);
        // An error once a picture has come is the player's own words again.
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        h.events.send(error("boom")).unwrap();
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t == "error: boom"),
            "{:?}",
            h.painted
        );
        assert!(!h.sent().iter().any(|c| matches!(c, Cmd::Stop)));
    }

    #[tokio::test]
    async fn an_xtream_portal_shows_a_player_error_as_it_did_and_stops_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.press(&[Key::ArrowDown]);
        h.sent();
        h.events.send(Event::Error { msg: "boom".into() }).unwrap();
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t == "error: boom"),
            "{:?}",
            h.painted
        );
        assert!(!h.sent().iter().any(|c| matches!(c, Cmd::Stop)));
    }

    /// NPO 1 in three qualities, RTL 4 in two, and NOS. The zap list holds the
    /// best of each: NPO 1 FHD (3), RTL 4 HD (5) and NOS (6).
    fn variants_catalog() -> Arc<CatalogStore> {
        let ch = |stream_id: i64, name: &str| LiveChannel {
            stream_id,
            name: name.into(),
            category_id: None,
            epg_channel_id: None,
            tv_archive: 0,
            tv_archive_duration: 0,
            is_adult: false,
            stream_icon: None,
        };
        let catalog = crate::portal::Catalog {
            live: vec![
                ch(1, "NL | NPO 1 SD"),
                ch(2, "NL | NPO 1 HD"),
                ch(3, "NL | NPO 1 FHD"),
                ch(4, "NL | RTL 4"),
                ch(5, "NL | RTL 4 HD"),
                ch(6, "NL | NOS Journaal"),
            ],
            ..Default::default()
        };
        Arc::new(CatalogStore::from_catalog(idle_portal(), catalog))
    }

    #[tokio::test]
    async fn an_xtream_portal_zaps_on_from_the_channel_after_a_quality_switch_or_a_variant() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(
            dir.path(),
            variants_catalog(),
            PortalState::Configured,
            None,
        );
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        let urls = |ids: &[i64]| ids.iter().map(|&id| live_url(id)).collect::<Vec<_>>();
        h.sent();
        h.app.zap_by_id(3);
        assert_eq!(loads(h.sent()), urls(&[3]), "NPO 1 FHD, at 0");
        // A quality down: the variant that plays is no entry of the zap list.
        h.frame_with(vec![egui::Event::Text("-".into())]);
        assert_eq!(loads(h.sent()), urls(&[2]), "NPO 1 HD");
        h.press(&[Key::ArrowDown]);
        assert_eq!(
            loads(h.sent()),
            urls(&[5]),
            "RTL 4, the channel after NPO 1"
        );
        h.press(&[Key::ArrowUp]);
        assert_eq!(loads(h.sent()), urls(&[3]), "and back to NPO 1");
        // A variant played by its own id (a favorite, a preset): the same.
        h.app.zap_by_id(1);
        assert_eq!(loads(h.sent()), urls(&[1]), "NPO 1 SD");
        h.press(&[Key::ArrowUp]);
        assert_eq!(
            loads(h.sent()),
            urls(&[6]),
            "NOS, the one before NPO 1 (wrapped)"
        );
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), urls(&[3]), "NPO 1 again");
    }

    #[tokio::test]
    async fn an_xtream_resume_of_a_variant_zaps_on_from_its_channel() {
        let dir = tempfile::tempdir().unwrap();
        write_last_watched(dir.path(), &live_url(2), true, Some(false));
        let mut h = Harness::start(
            dir.path(),
            variants_catalog(),
            PortalState::Configured,
            None,
        );
        assert_eq!(loads(h.sent()), [live_url(2)], "NPO 1 HD resumes");
        assert_eq!(h.app.current_stream_id, Some(2));
        h.press(&[Key::ArrowDown]);
        assert_eq!(loads(h.sent()), [live_url(5)], "RTL 4, the one after NPO 1");
    }

    #[tokio::test]
    async fn a_channel_the_list_names_by_an_address_leaves_no_password_in_the_log() {
        let dir = tempfile::tempdir().unwrap();
        // The list's own author named the second channel by an address: a name
        // from a list is text like any other, and the log masks what it holds.
        let list = "#EXTM3U\n#EXTINF:-1,Eerste\nhttps://a.example/1.m3u8\n\
                    #EXTINF:-1,https://user:secret@c.example/live/2.ts\n\
                    https://c.example/2.m3u8\n";
        let mut h = m3u_app(dir.path(), list);
        let log = LogCapture::start_debug();
        h.press(&[Key::ArrowDown]);
        assert_eq!(
            h.app.current_name.as_deref(),
            Some("https://user:secret@c.example/live/2.ts")
        );
        let text = log.text();
        assert!(text.contains("zap: -> https://<u>:<p>@c.example"), "{text}");
        assert!(
            text.contains("toast: [TV] https://<u>:<p>@c.example"),
            "{text}"
        );
        assert!(
            !text.contains("secret"),
            "the password is in the log:\n{text}"
        );
    }

    #[test]
    fn open_source_opens_an_m3u_list_and_logs_its_id_and_host_never_its_address() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf());
        let source = |id, address: &str| Source {
            id,
            kind: SourceKind::M3u,
            name: String::new(),
            address: address.into(),
        };
        let log = LogCapture::start();
        let web = open_source(
            &source(2, "https://user:secret@lists.example/tv.m3u?token=abc123"),
            &storage,
        );
        let file = open_source(&source(3, r"C:\lijsten\gratis.m3u"), &storage);
        for opened in [&web, &file] {
            assert_eq!(opened.state, PortalState::Configured);
            assert!(opened.creds.is_none(), "no Xtream credentials");
            assert_eq!(opened.portal.kind(), SourceKind::M3u);
        }
        let text = log.text();
        assert!(text.contains("source: m3u 2 (lists.example)"), "{text}");
        assert!(text.contains("source: m3u 3 (file)"), "{text}");
        for secret in ["secret", "abc123", "tv.m3u", "lijsten", "gratis"] {
            assert!(!text.contains(secret), "{secret} in the log:\n{text}");
        }
    }

    #[test]
    fn open_source_opens_an_xtream_portal_or_the_setup_prompt_and_logs_no_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf());
        let source = |address: &str| Source {
            id: 1,
            kind: SourceKind::Xtream,
            name: String::new(),
            address: address.into(),
        };
        let log = LogCapture::start();
        let real = open_source(&source("bob:geheim@srv.example.net:8080"), &storage);
        assert_eq!(real.state, PortalState::Configured);
        assert_eq!(real.portal.kind(), SourceKind::Xtream);
        let creds = real.creds.expect("its credentials");
        assert_eq!(
            (creds.username.as_str(), creds.password.as_str()),
            ("bob", "geheim")
        );
        assert_eq!((creds.host.as_str(), creds.port), ("srv.example.net", 8080));
        // The stand-in values of the zip's run.bat, and something unreadable:
        // the setup prompt, on a portal that is never asked anything.
        let placeholder = open_source(&source("user:pass@host.example.com:80"), &storage);
        let unreadable = open_source(&source("no at sign here"), &storage);
        assert_eq!(placeholder.state, PortalState::Placeholder);
        assert_eq!(unreadable.state, PortalState::Missing);
        for prompt in [&placeholder, &unreadable] {
            assert!(prompt.creds.is_none());
        }
        let text = log.text();
        assert!(
            text.contains("portal: xtream <user>@srv.example.net:8080"),
            "{text}"
        );
        for secret in ["bob", "geheim", "no at sign"] {
            assert!(!text.contains(secret), "{secret} in the log:\n{text}");
        }
    }

    #[test]
    fn open_xtream_is_the_one_place_of_the_log_lines_0_6_1_has_for_the_credentials() {
        let log = LogCapture::start();
        let creds = XtreamCreds {
            username: "bob".into(),
            password: "geheim".into(),
            host: "srv.example.net".into(),
            port: 8080,
        };
        let from = crate::args::CredsOrigin::CommandLine;
        let real = open_xtream(CredsSource::Configured(creds, from));
        let placeholder = open_xtream(CredsSource::Placeholder);
        let missing = open_xtream(CredsSource::Missing);
        let invalid = open_xtream(CredsSource::Invalid(
            "missing '@' separator in creds".into(),
        ));
        // What the app runs on: the portal, or the setup prompt on a portal that
        // is never asked anything.
        assert_eq!(real.state, PortalState::Configured);
        let password = real.creds.as_ref().map(|c| c.password.as_str());
        assert_eq!(password, Some("geheim"));
        let prompts = [
            (&placeholder, PortalState::Placeholder),
            (&missing, PortalState::Missing),
            (&invalid, PortalState::Missing),
        ];
        for (opened, state) in prompts {
            assert_eq!(opened.state, state);
            assert!(opened.creds.is_none());
        }
        for opened in [&real, &placeholder, &missing, &invalid] {
            assert_eq!(opened.portal.kind(), SourceKind::Xtream);
        }
        // The words of 0.6.1's log, in the order they were said.
        let text = log.text();
        let said: Vec<&str> = text
            .lines()
            .filter_map(|line| line.split_once("bongo_player::app: "))
            .map(|(_, message)| message)
            .collect();
        assert_eq!(
            said,
            [
                "portal: xtream <user>@srv.example.net:8080",
                "portal: placeholder credentials - showing the setup prompt",
                "portal: no credentials (--xtream or BongoPlayer.ini) - showing the setup prompt",
                "portal: unreadable credentials (missing '@' separator in creds) - showing the setup prompt",
            ],
            "{text}"
        );
    }

    // ---- the picture of the channel before ----

    /// An app on `TEST_LIST`, on its second channel, whose picture (4 x 2) is on
    /// the screen.
    fn m3u_app_with_a_picture(root: &std::path::Path) -> Harness {
        let mut h = m3u_app(root, TEST_LIST);
        h.app.zap_pacer = ZapPacer::new(Duration::ZERO);
        h.press(&[Key::ArrowDown]);
        h.put_frame(4, 2);
        h.frame(&[]);
        assert!(h.app.video_tex.is_some(), "the picture is on the screen");
        h.sent();
        h
    }

    /// Frames go by with the picture of the channel before still on the bus,
    /// and none of it comes back: the empty state shows instead.
    fn the_old_picture_stays_away(h: &mut Harness) {
        for _ in 0..3 {
            h.frame(&[]);
        }
        assert!(h.app.video_tex.is_none(), "the old picture came back");
        assert!(
            h.painted.iter().any(|t| t == crate::APP_NAME),
            "the empty state shows: {:?}",
            h.painted
        );
    }

    #[tokio::test]
    async fn a_refused_m3u_load_takes_the_picture_of_the_channel_before_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app_with_a_picture(dir.path());
        h.press(&[Key::ArrowDown]);
        let refused = Event::Error {
            msg: "mpv error -4: an argument holds a NUL byte".into(),
        };
        h.events.send(refused).unwrap();
        h.frame(&[]);
        assert!(h.app.video_tex.is_none(), "the picture goes at once");
        the_old_picture_stays_away(&mut h);
        // The next load shows its own pictures.
        h.press(&[Key::ArrowUp]);
        h.put_frame(8, 4);
        h.frame(&[]);
        let video = h.app.video_tex.as_ref().expect("the new picture");
        assert_eq!(video.size(), [8, 4]);
    }

    #[tokio::test]
    async fn a_dead_m3u_channel_takes_the_picture_of_the_channel_before_off_the_screen() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app_with_a_picture(dir.path());
        h.press(&[Key::ArrowDown]);
        // It never showed a picture of its own: the one on the screen is the
        // channel before's.
        assert!(h.app.stream_starting_since.is_some());
        let dead = Event::EndOfFile {
            reason: "error".into(),
        };
        h.events.send(dead).unwrap();
        h.frame(&[]);
        assert!(h.app.video_tex.is_none(), "the picture goes at once");
        the_old_picture_stays_away(&mut h);
        // A load that was on its way already: its first picture is newer than
        // the error, and shows.
        h.put_frame(8, 4);
        h.frame(&[]);
        let video = h.app.video_tex.as_ref().expect("the new picture");
        assert_eq!(video.size(), [8, 4]);
        // So does the load of the channel the user zaps to next.
        h.press(&[Key::ArrowUp]);
        h.put_frame(12, 6);
        h.frame(&[]);
        let video = h.app.video_tex.as_ref().expect("the next picture");
        assert_eq!(video.size(), [12, 6]);
    }

    #[tokio::test]
    async fn an_m3u_channel_that_keeps_ending_before_its_first_picture_takes_the_one_before_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app_with_a_picture(dir.path());
        h.press(&[Key::ArrowDown]);
        assert!(
            h.app.stream_starting_since.is_some(),
            "no picture of its own"
        );
        let eof = || Event::EndOfFile {
            reason: "eof".into(),
        };
        // The first ends are reloaded, as before; the picture stays meanwhile.
        for _ in 0..3 {
            h.events.send(eof()).unwrap();
            h.frame(&[]);
            assert!(h.app.video_tex.is_some(), "a reload keeps going");
        }
        assert_eq!(loads(h.sent()).len(), 4, "the zap and three reloads");
        // The reloads are spent: this end is the channel's last.
        h.events.send(eof()).unwrap();
        h.frame(&[]);
        assert!(h.app.video_tex.is_none(), "the picture goes at once");
        the_old_picture_stays_away(&mut h);
        let named = "stream error on Kanaal Twee - zap up or down to retry";
        assert!(h.painted.iter().any(|t| t == named), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn an_m3u_channel_that_has_played_keeps_its_own_last_frame_when_its_reloads_fail() {
        // Its reloads end before a new picture: up to the eof that ends them, or
        // with an error. Each case: how the stream ends, and how many loads that
        // makes (the zap and its reloads).
        for (ends, loaded) in [(&["eof"; 4][..], 4), (&["eof", "error"][..], 2)] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = m3u_app_with_a_picture(dir.path());
            h.press(&[Key::ArrowDown]);
            // The channel shows a picture of its own.
            h.events.send(Event::PlaybackStarted).unwrap();
            h.frame(&[]);
            h.put_frame(8, 4);
            h.frame(&[]);
            for reason in ends {
                let end = Event::EndOfFile {
                    reason: (*reason).into(),
                };
                h.events.send(end).unwrap();
                h.frame(&[]);
            }
            assert_eq!(loads(h.sent()).len(), loaded, "{ends:?}");
            for _ in 0..3 {
                h.frame(&[]);
            }
            let shown = h.app.video_tex.as_ref().map(|t| t.size());
            assert_eq!(shown, Some([8, 4]), "{ends:?}: its own last frame stays");
        }
    }

    #[tokio::test]
    async fn an_m3u_channel_that_showed_its_own_picture_keeps_its_last_frame_when_it_errors() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = m3u_app_with_a_picture(dir.path());
        h.press(&[Key::ArrowDown]);
        assert!(
            h.app.stream_starting_since.is_some(),
            "no picture of its own yet"
        );
        // The channel starts and shows a picture of its own.
        h.events.send(Event::PlaybackStarted).unwrap();
        h.frame(&[]);
        assert!(
            h.app.stream_starting_since.is_none(),
            "its first frame is shown"
        );
        h.put_frame(8, 4);
        h.frame(&[]);
        let own = h.app.video_tex.as_ref().expect("its own picture");
        assert_eq!(own.size(), [8, 4]);
        // It dies later: the app stays on it, and its last frame stays too.
        let dead = Event::EndOfFile {
            reason: "error".into(),
        };
        h.events.send(dead).unwrap();
        h.frame(&[]);
        let named = "stream error on Kanaal Twee - zap up or down to retry";
        assert!(h.painted.iter().any(|t| t == named), "{:?}", h.painted);
        for _ in 0..3 {
            h.frame(&[]);
        }
        let kept = h.app.video_tex.as_ref().expect("its last frame stays");
        assert_eq!(kept.size(), [8, 4]);
    }

    #[tokio::test]
    async fn an_xtream_dead_channel_leaves_the_last_picture_on_the_screen_as_it_did() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        h.put_frame(4, 2);
        h.frame(&[]);
        h.press(&[Key::ArrowDown]);
        assert!(h.app.video_tex.is_some());
        for event in [
            Event::EndOfFile {
                reason: "error".into(),
            },
            Event::Error { msg: "boom".into() },
        ] {
            h.events.send(event).unwrap();
            h.frame(&[]);
            h.frame(&[]);
            assert!(h.app.video_tex.is_some(), "the picture stays");
        }
    }

    // ---- a channel with no name ----

    #[tokio::test]
    async fn a_channel_with_no_name_is_shown_by_its_masked_address_and_plays_from_the_real_one() {
        let dir = tempfile::tempdir().unwrap();
        // The second channel has no name: the list reader names it after its
        // address, with the account masked.
        let list = "#EXTM3U\n#EXTINF:-1,Eerste\nhttps://a.example/1.m3u8\n\
                    https://user:secret@c.example/live/2.ts\n";
        let mut h = m3u_app(dir.path(), list);
        let log = LogCapture::start_debug();
        let masked = "https://<u>:<p>@c.example/live/2.ts";
        let nothing_secret = |h: &Harness| {
            let seen = h.painted.iter().any(|t| t.contains("secret"));
            assert!(!seen, "the account is on the screen: {:?}", h.painted);
        };
        h.sent();
        h.press(&[Key::ArrowDown]);
        assert_eq!(h.app.current_name.as_deref(), Some(masked));
        let zap_toast = format!("[TV] {masked}");
        assert!(h.painted.contains(&zap_toast), "{:?}", h.painted);
        nothing_secret(&h);
        // mpv gets the real address: the account is what the server wants.
        let real = "https://user:secret@c.example/live/2.ts";
        assert_eq!(loads(h.sent()), [real]);
        // The error names the channel as the zap toast did.
        let dead = Event::EndOfFile {
            reason: "error".into(),
        };
        h.events.send(dead).unwrap();
        h.frame(&[]);
        let named = format!("stream error on {masked} - zap up or down to retry");
        assert!(h.painted.contains(&named), "{:?}", h.painted);
        nothing_secret(&h);
        // A favorite keeps the name that shows.
        h.frame_with(vec![egui::Event::Text("*".into())]);
        let saved = std::fs::read_to_string(dir.path().join("favorites.json")).unwrap();
        assert!(saved.contains(masked), "{saved}");
        assert!(!saved.contains("secret"), "the account is saved: {saved}");
        // The search finds it by that name.
        h.frame_with(vec![key_press(Key::F), egui::Event::Text("f".into())]);
        h.frame(&[]);
        h.type_text("c.example");
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|t| t.contains(masked)),
            "{:?}",
            h.painted
        );
        nothing_secret(&h);
        let text = log.text();
        assert!(
            !text.contains("secret"),
            "the account is in the log:\n{text}"
        );
    }

    // ---- the news on a list ----

    #[tokio::test]
    async fn n_on_an_m3u_source_lists_the_channel_a_rule_names_and_plays_it_live_and_straight() {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(TEST_LIST))
            .mount(&server)
            .await;
        let dir = tempfile::tempdir().unwrap();
        let cache = dir.path().join("m3u-2.m3u");
        let portal = M3uPortal::new(2, &format!("{}/tv.m3u", server.uri()), cache);
        let store = Arc::new(CatalogStore::new(Arc::new(portal)));
        store.spawn_fetch();
        wait_until("the list", || store.is_loaded()).await;
        let relay = Relay::start(crate::relay::RelayConfig::default()).unwrap();
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            Sources::default(),
            Some(relay),
        );
        // A rule that names a channel of the list. The list has no catch-up and
        // no guide: nothing for the news to fetch.
        h.app.settings.news_rules = Some(vec![NewsRule {
            label: "EEN".into(),
            ..news_rule("Kanaal Een")
        }]);
        let asked = server.received_requests().await.unwrap().len();
        assert_eq!(asked, 1, "the list, and nothing else yet");
        h.press(&[Key::N]);
        let slot = h.app.news_picker_slot.clone();
        wait_until("the news list", || slot.lock().is_some()).await;
        h.frame(&[]);
        h.frame(&[]);
        let picker = h.app.news_picker.as_ref().expect("the list popped up");
        assert_eq!(picker.items.len(), 1);
        assert!(picker.items[0].live, "no catch-up: the channel itself");
        assert!(h.painted.iter().any(|t| t == "EEN"), "{:?}", h.painted);
        h.sent();
        h.press(&[Key::Enter]);
        let straight = ["https://a.example/een.m3u8"];
        assert_eq!(loads(h.sent()), straight, "live, not through the relay");
        assert_eq!(h.app.current_name.as_deref(), Some("EEN - Live"));
        assert!(h.app.current_live, "the stall watchdog watches it");
        // Up and down go on from the channel the news put on.
        h.press(&[Key::ArrowDown]);
        let twee = "https://b.example/twee.m3u8|User-Agent=Bongo%2F1.0&Referer=https%3A%2F%2Fwww.example.com%2F";
        assert_eq!(loads(h.sent()), [twee]);
        let after = server.received_requests().await.unwrap().len();
        assert_eq!(after, asked, "the news asked the server for nothing");
    }

    // ---- using, saving and deleting sources ----

    /// Source 1, the tests' Xtream portal, in use and playing TEST 03; source 2,
    /// the M3U list TEST_LIST in a file, whose last channel was Kanaal Een
    /// (returned). Nothing in it writes the ini; a test that then does points
    /// `BONGO_INI` into `dir` and holds ENV_LOCK for that.
    fn two_sources(dir: &std::path::Path) -> (Harness, String) {
        let file = dir.join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let sources = Sources::parse(&format!(
            "source=1;xtream;;u:p@h:80\nsource=2;m3u;Lijst;{}\nactive=1\n",
            file.display()
        ));
        let (known, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let een = known.live_stream_url(catalog.live[0].stream_id);
        let mut two = Storage::with_root(dir.to_path_buf());
        two.set_source(2);
        let record = LastWatched {
            url: een.clone(),
            live: true,
            adult: Some(false),
        };
        record.save(&two.last_watched_path());
        let mut h = Harness::start_with(
            dir,
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        );
        h.app.zap_by_id(3);
        h.sent();
        (h, een)
    }

    #[tokio::test]
    async fn switching_stops_the_old_stream_and_resumes_the_new_sources_last_channel() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let (mut h, een) = two_sources(dir.path());
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(2));
        let sent = h.sent();
        assert!(matches!(sent.first(), Some(Cmd::Stop)), "{sent:?}");
        assert_eq!(loads(sent), [een], "source 2's last channel");
        assert_eq!(h.app.catalog.kind(), SourceKind::M3u);
        assert_eq!(h.app.storage.source(), 2);
        assert_eq!(h.app.creds, None);
        std::env::remove_var("BONGO_INI");
    }

    #[tokio::test]
    async fn the_next_start_opens_the_source_that_was_in_use() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let (mut h, _) = two_sources(dir.path());
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(2));
        std::env::remove_var("BONGO_INI");
        // What main reads at the next start.
        let saved = Sources::load(&ini);
        assert_eq!(saved.active_id(), Some(2));
        let opened = open_source(
            saved.active().unwrap(),
            &Storage::with_root(dir.path().to_path_buf()),
        );
        assert_eq!(opened.portal.kind(), SourceKind::M3u);
        assert_eq!(opened.state, PortalState::Configured);
    }

    #[tokio::test]
    async fn using_a_url_source_shows_its_saved_list_before_the_download() {
        let dir = tempfile::tempdir().unwrap();
        let server = wiremock::MockServer::start().await;
        let fresh = "#EXTM3U\n#EXTINF:-1,Kanaal Drie\nhttps://d.example/drie.m3u8\n";
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(200)
                    .set_body_string(fresh)
                    .set_delay(Duration::from_millis(400)),
            )
            .mount(&server)
            .await;
        let storage = Storage::with_root(dir.path().to_path_buf());
        std::fs::create_dir_all(storage.cache_dir()).unwrap();
        std::fs::write(storage.m3u_cache_path(2), TEST_LIST).unwrap();
        let sources = Sources::parse(&format!(
            "source=1;xtream;;u:p@h:80\nsource=2;m3u;Lijst;{}/tv.m3u\nactive=1\n",
            server.uri()
        ));
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        );
        {
            // The variable is the whole process's: held for the switch only.
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            h.app.activate_source(2);
            std::env::remove_var("BONGO_INI");
        }
        let names = |h: &Harness| -> Vec<String> {
            h.app
                .catalog
                .zap_channels()
                .into_iter()
                .map(|c| c.name)
                .collect()
        };
        wait_until("the saved list", || h.app.catalog.is_loaded()).await;
        assert_eq!(names(&h), ["Kanaal Een", "Kanaal Twee", "Kanaal Twee"]);
        wait_until("the fresh list", || names(&h) == ["Kanaal Drie"]).await;
    }

    #[tokio::test]
    async fn favorites_and_presets_are_kept_per_source() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        // The variable is the whole process's: held for a switch only.
        let switch = |h: &mut Harness, id| {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", &ini);
            h.app.activate_source(id);
            std::env::remove_var("BONGO_INI");
        };
        let (mut h, _) = two_sources(dir.path()); // TEST 03 plays
        h.app.toggle_favorite_current();
        h.app.save_preset(1);
        switch(&mut h, 2);
        assert!(!h.app.favorites.contains(3), "source 2 starts with none");
        assert!(h.app.presets.get(1).is_none());
        wait_until("the list", || h.app.catalog.is_loaded()).await;
        h.app.zap_by_id(crate::m3u::stream_id("Een.nl"));
        h.app.toggle_favorite_current();
        assert!(dir.path().join("favorites-2.json").exists());
        switch(&mut h, 1);
        assert!(h.app.favorites.contains(3), "source 1's own again");
        assert_eq!(h.app.presets.get(1).map(|p| p.stream_id), Some(3));
    }

    #[test]
    fn deleting_a_source_takes_its_files_and_never_the_source_in_use() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let sources = Sources::parse(
            "source=1;xtream;;u:p@h:80\nsource=2;m3u;Lijst;https://a.example/tv.m3u\nactive=1\n",
        );
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        );
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        std::fs::create_dir_all(two.cache_dir()).unwrap();
        for path in [
            two.favorites_path(),
            two.presets_path(),
            two.last_watched_path(),
            two.m3u_cache_path(2),
        ] {
            std::fs::write(path, "x").unwrap();
        }
        h.app.apply_settings_action(SettingsAction::DeleteSource(1));
        assert!(h.app.sources.get(1).is_some(), "the source in use stays");
        h.app.apply_settings_action(SettingsAction::DeleteSource(2));
        assert!(h.app.sources.get(2).is_none());
        assert!(!two.favorites_path().exists() && !two.m3u_cache_path(2).exists());
        assert_eq!(
            std::fs::read_to_string(&ini).unwrap(),
            "portal=u:p@h:80\n",
            "one portal left: 0.6.1's ini"
        );
        std::env::remove_var("BONGO_INI");
    }

    #[tokio::test]
    async fn a_setup_prompt_save_writes_the_ini_0_6_1_wrote() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        };
        assert!(h
            .app
            .keep_xtream(portal_save(creds, "u2:p2@127.0.0.1:1"))
            .is_ok());
        std::env::remove_var("BONGO_INI");
        assert_eq!(
            std::fs::read_to_string(&ini).unwrap(),
            "portal=u2:p2@127.0.0.1:1\n"
        );
        assert_eq!(h.app.portal_state, PortalState::Configured);
        assert_eq!(h.app.sources.active_id(), Some(1));
    }

    #[tokio::test]
    async fn a_new_source_is_used_at_once_only_when_none_is_in_use() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let draft = SourceDraft {
            editing: Editing::New,
            kind: SourceKind::M3u,
            name: "Gratis".into(),
            address: file.display().to_string(),
        };
        // A portal is in use: the list is only added.
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        h.app
            .apply_settings_action(SettingsAction::SaveSource(draft.clone()));
        // An area shows from its second frame on.
        h.frame(&[]);
        h.frame(&[]);
        assert_eq!(h.app.sources.list().len(), 2);
        assert_eq!(h.app.sources.active_id(), Some(1));
        assert_eq!(h.app.catalog.kind(), SourceKind::Xtream);
        assert!(
            h.painted.iter().any(|t| t == "source saved: Gratis"),
            "{:?}",
            h.painted
        );
        // Nothing usable in use (the setup prompt): the list is used at once.
        let empty = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut fresh = Harness::start(empty.path(), catalog, PortalState::Missing, None);
        fresh
            .app
            .apply_settings_action(SettingsAction::SaveSource(draft));
        // Source 1's files are the portal's: a list is never source 1.
        assert_eq!(fresh.app.sources.active_id(), Some(2));
        assert_eq!(fresh.app.catalog.kind(), SourceKind::M3u);
        assert_eq!(fresh.app.portal_state, PortalState::Configured);
        std::env::remove_var("BONGO_INI");
    }

    #[tokio::test]
    async fn the_m3u_source_in_use_saved_with_another_address_drops_its_saved_list() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let (old, new) = (dir.path().join("oud.m3u"), dir.path().join("nieuw.m3u"));
        std::fs::write(&old, TEST_LIST).unwrap();
        std::fs::write(&new, TEST_LIST).unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf());
        std::fs::create_dir_all(storage.cache_dir()).unwrap();
        std::fs::write(storage.m3u_cache_path(1), TEST_LIST).unwrap();
        let sources = Sources::parse(&format!("source=1;m3u;Lijst;{}\nactive=1\n", old.display()));
        let (portal, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            sources,
            None,
        );
        let generation = h.app.catalog_generation;
        h.app
            .apply_settings_action(SettingsAction::SaveSource(SourceDraft {
                editing: Editing::Saved(1),
                kind: SourceKind::M3u,
                name: "Lijst".into(),
                address: new.display().to_string(),
            }));
        assert!(
            !storage.m3u_cache_path(1).exists(),
            "the old address's list"
        );
        assert_eq!(h.app.catalog_generation, generation + 1, "read again");
        assert_eq!(
            h.app.sources.get(1).map(|s| s.address.clone()),
            Some(new.display().to_string())
        );
        std::env::remove_var("BONGO_INI");
    }

    /// A server that answers every GET with `status` and `body`, `delay` later.
    async fn list_server(status: u16, body: &str, delay: Duration) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .set_body_string(body)
                    .set_delay(delay),
            )
            .mount(&server)
            .await;
        server
    }

    /// Waits (2 s at most) until `server` was asked something.
    async fn until_asked(server: &wiremock::MockServer) {
        for _ in 0..400 {
            if server
                .received_requests()
                .await
                .is_some_and(|r| !r.is_empty())
            {
                return;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
        panic!("the server was never asked");
    }

    /// The app on M3U source 2 of `ini`, whose download from `server` is under
    /// way (the catalog it runs on is returned too).
    async fn downloading(
        dir: &std::path::Path,
        ini: &str,
        server: &wiremock::MockServer,
    ) -> (Harness, Arc<CatalogStore>) {
        let sources = Sources::parse(ini);
        let storage = Storage::with_root(dir.to_path_buf());
        let list = sources.get(2).cloned().expect("source 2");
        let store = Arc::new(CatalogStore::new(open_source(&list, &storage).portal));
        store.spawn_fetch();
        let mut h = Harness::start_with(
            dir,
            store.clone(),
            PortalState::Configured,
            None,
            None,
            sources,
            None,
        );
        h.app.storage.set_source(2);
        until_asked(server).await;
        (h, store)
    }

    #[tokio::test]
    async fn a_download_under_way_never_brings_back_the_list_of_a_source_deleted_meanwhile() {
        let dir = tempfile::tempdir().unwrap();
        let slow = list_server(200, TEST_LIST, Duration::from_millis(400)).await;
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let ini = format!(
            "source=1;m3u;Bestand;{}\nsource=2;m3u;Lijst;{}/tv.m3u\nactive=2\n",
            file.display(),
            slow.uri()
        );
        let (mut h, old) = downloading(dir.path(), &ini, &slow).await;
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            h.app
                .apply_settings_action(SettingsAction::ActivateSource(1));
            h.app.apply_settings_action(SettingsAction::DeleteSource(2));
            std::env::remove_var("BONGO_INI");
        }
        wait_until("the old download", || old.is_loaded()).await;
        let saved = Storage::with_root(dir.path().to_path_buf()).m3u_cache_path(2);
        assert!(!saved.exists(), "the deleted source's list came back");
        // The next list added is source 2 again, and shows none of the old one.
        let refusing = list_server(503, "", Duration::ZERO).await;
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let address = format!("{}/nieuw.m3u", refusing.uri());
            let list = list_draft(Editing::New, "Nieuw", &address);
            h.app
                .apply_settings_action(SettingsAction::SaveSource(list));
            h.app
                .apply_settings_action(SettingsAction::ActivateSource(2));
            std::env::remove_var("BONGO_INI");
        }
        assert_eq!(h.app.sources.get(2).map(|s| s.name.as_str()), Some("Nieuw"));
        wait_until("the new list's download", || {
            matches!(h.app.catalog.status(), CatalogStatus::Failed(_))
        })
        .await;
        assert!(!h.app.catalog.is_loaded(), "a list shows");
    }

    #[tokio::test]
    async fn a_download_under_way_never_brings_back_the_list_of_an_address_saved_over() {
        let dir = tempfile::tempdir().unwrap();
        let slow = list_server(200, TEST_LIST, Duration::from_millis(400)).await;
        let refusing = list_server(503, "", Duration::ZERO).await;
        let ini = format!("source=2;m3u;Lijst;{}/tv.m3u\nactive=2\n", slow.uri());
        let (mut h, old) = downloading(dir.path(), &ini, &slow).await;
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let address = format!("{}/ander.m3u", refusing.uri());
            let list = list_draft(Editing::Saved(2), "Lijst", &address);
            h.app
                .apply_settings_action(SettingsAction::SaveSource(list));
            std::env::remove_var("BONGO_INI");
        }
        wait_until("the old download", || old.is_loaded()).await;
        let saved = Storage::with_root(dir.path().to_path_buf()).m3u_cache_path(2);
        assert!(!saved.exists(), "the old address's list came back");
        wait_until("the new address's download", || {
            matches!(h.app.catalog.status(), CatalogStatus::Failed(_))
        })
        .await;
        assert!(!h.app.catalog.is_loaded(), "a list shows");
    }

    #[test]
    fn a_file_of_a_deleted_source_that_cannot_be_removed_is_named_in_the_log() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_list(dir.path());
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        // A folder where source 2's favorites would be: no file to remove.
        std::fs::create_dir(two.favorites_path()).unwrap();
        let log = LogCapture::start();
        h.app.apply_settings_action(SettingsAction::DeleteSource(2));
        std::env::remove_var("BONGO_INI");
        let text = log.text();
        let named = format!("storage: {} not removed: ", two.favorites_path().display());
        assert!(text.contains(&named), "{text}");
        // The files that were not there are no failure.
        assert!(!text.contains("presets-2.json"), "{text}");
        assert!(
            text.contains("settings: source 2 deleted (0 files)"),
            "{text}"
        );
    }

    #[test]
    fn a_name_equal_to_its_kinds_default_is_kept_as_none() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        h.app
            .apply_settings_action(SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::M3u,
                name: " M3U list ".into(),
                address: "https://a.example/tv.m3u".into(),
            }));
        assert_eq!(h.app.sources.get(2).map(|s| s.name.as_str()), Some(""));
        std::env::remove_var("BONGO_INI");
    }

    #[test]
    fn an_xtream_source_saved_while_another_is_in_use_is_only_kept() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let sources = Sources::parse(
            "source=1;m3u;Lijst;https://a.example/tv.m3u\nsource=2;xtream;Oud;u:p@h:80\nactive=1\n",
        );
        let (portal, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            sources,
            None,
        );
        let keep = KeepXtream {
            creds: XtreamCreds {
                username: "u2".into(),
                password: "p2".into(),
                host: "127.0.0.1".into(),
                port: 1,
            },
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::Saved(2),
            name: "Nieuw".into(),
        };
        assert!(h.app.keep_xtream(keep).is_ok());
        let two = h.app.sources.get(2).unwrap();
        assert_eq!(
            (two.name.as_str(), two.address.as_str()),
            ("Nieuw", "u2:p2@127.0.0.1:1")
        );
        assert_eq!(
            h.app.catalog.kind(),
            SourceKind::M3u,
            "the list stays in use"
        );
        assert_eq!(h.app.creds, None);
        std::env::remove_var("BONGO_INI");
    }

    /// Source 1, the tests' Xtream portal, in use, and source 2, a list on the
    /// web, saved next to it.
    const PORTAL_AND_LIST: &str =
        "source=1;xtream;;u:p@h:80\nsource=2;m3u;Lijst;https://a.example/tv.m3u\nactive=1\n";

    fn portal_and_list(dir: &std::path::Path) -> Harness {
        Harness::start_with(
            dir,
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse(PORTAL_AND_LIST),
            None,
        )
    }

    /// Other Xtream credentials than the tests' portal's.
    fn other_creds() -> XtreamCreds {
        XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port: 1,
        }
    }

    /// A list, as the sources section's Save hands it over.
    fn list_draft(editing: Editing, name: &str, address: &str) -> SourceDraft {
        SourceDraft {
            editing,
            kind: SourceKind::M3u,
            name: name.into(),
            address: address.into(),
        }
    }

    /// Points `BONGO_INI` at a file in a folder that is not there, so that
    /// nothing can be written to it. Hold ENV_LOCK; the caller removes the
    /// variable.
    fn point_ini_nowhere(dir: &std::path::Path) -> std::path::PathBuf {
        let ini = dir.join("no-such-folder").join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        ini
    }

    /// The text of the toast on show.
    fn toast_text(h: &Harness) -> String {
        h.app
            .toast
            .as_ref()
            .map(|t| t.0.clone())
            .unwrap_or_default()
    }

    #[test]
    fn a_default_name_in_any_language_is_kept_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start(dir.path(), loaded_catalog(), PortalState::Configured, None);
        for lang in Lang::ALL {
            h.app.lang = lang;
            for kind in [SourceKind::Xtream, SourceKind::M3u] {
                // The form may have been made in another language than the one in
                // use now: the default of the kind in any of them is no name of its
                // own, whatever spaces are around it.
                for made_in in Lang::ALL {
                    let spaced = format!("  {} ", kind.default_name(made_in));
                    let stored = h.app.stored_name(kind, &spaced);
                    assert_eq!(stored, "", "{made_in:?}'s {kind:?} name in {lang:?}");
                }
                assert_eq!(h.app.stored_name(kind, " Thuis "), "Thuis");
                // The default of the other kind is a name like any other.
                let other = match kind {
                    SourceKind::Xtream => SourceKind::M3u,
                    SourceKind::M3u => SourceKind::Xtream,
                };
                let theirs = other.default_name(lang);
                assert_eq!(h.app.stored_name(kind, theirs), theirs);
            }
        }
    }

    #[test]
    fn deleting_the_saved_source_the_command_lines_portal_runs_on_leaves_its_files() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_list(dir.path());
        // `--xtream` runs: no saved source is in use, and its data is source 1's.
        h.app.creds_from_command_line = true;
        let one = Storage::with_root(dir.path().to_path_buf());
        let files = [
            one.favorites_path(),
            one.presets_path(),
            one.last_watched_path(),
        ];
        for path in &files {
            std::fs::write(path, "x").unwrap();
        }
        h.app.apply_settings_action(SettingsAction::DeleteSource(1));
        std::env::remove_var("BONGO_INI");
        assert!(h.app.sources.get(1).is_none(), "no saved source is in use");
        assert!(
            files.iter().all(|path| path.exists()),
            "the portal runs on them"
        );
    }

    #[test]
    fn a_list_the_ini_cannot_hold_is_not_kept_and_nothing_about_the_old_one_changes() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        point_ini_nowhere(dir.path());
        let (old, new) = (dir.path().join("oud.m3u"), dir.path().join("nieuw.m3u"));
        std::fs::write(&old, TEST_LIST).unwrap();
        std::fs::write(&new, TEST_LIST).unwrap();
        let storage = Storage::with_root(dir.path().to_path_buf());
        std::fs::create_dir_all(storage.cache_dir()).unwrap();
        std::fs::write(storage.m3u_cache_path(1), TEST_LIST).unwrap();
        let sources = Sources::parse(&format!("source=1;m3u;Lijst;{}\nactive=1\n", old.display()));
        let (portal, catalog) = M3uPortal::from_text(TEST_LIST).unwrap();
        let store = Arc::new(CatalogStore::from_catalog(Arc::new(portal), catalog));
        let mut h = Harness::start_with(
            dir.path(),
            store,
            PortalState::Configured,
            None,
            None,
            sources.clone(),
            None,
        );
        let generation = h.app.catalog_generation;
        // Another address for the list in use, and a second list.
        for editing in [Editing::Saved(1), Editing::New] {
            let draft = list_draft(editing, "Lijst", &new.display().to_string());
            h.app
                .apply_settings_action(SettingsAction::SaveSource(draft));
        }
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources, sources, "nothing was kept");
        assert_eq!(h.app.catalog_generation, generation, "nothing was opened");
        assert!(storage.m3u_cache_path(1).exists(), "the saved list stays");
        assert!(
            toast_text(&h).starts_with("could not write "),
            "{}",
            toast_text(&h)
        );
    }

    #[test]
    fn an_xtream_login_the_ini_cannot_hold_is_not_kept() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = point_ini_nowhere(dir.path());
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        let generation = h.app.catalog_generation;
        let kept = h
            .app
            .keep_xtream(portal_save(other_creds(), "u2:p2@127.0.0.1:1"));
        std::env::remove_var("BONGO_INI");
        let Err(ConnState::WriteFailed { path, .. }) = kept else {
            panic!("expected a failed write, got {kept:?}");
        };
        assert_eq!(std::path::PathBuf::from(path), ini);
        assert!(h.app.sources.list().is_empty());
        assert_eq!(h.app.portal_state, PortalState::Missing, "the prompt stays");
        assert_eq!(h.app.creds, None);
        assert_eq!(h.app.catalog_generation, generation);
        assert!(
            toast_text(&h).starts_with("could not write "),
            "{}",
            toast_text(&h)
        );
    }

    #[tokio::test]
    async fn a_switch_the_ini_cannot_hold_does_not_happen() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        point_ini_nowhere(dir.path());
        let (mut h, _) = two_sources(dir.path());
        let generation = h.app.catalog_generation;
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(2));
        std::env::remove_var("BONGO_INI");
        assert!(h.sent().is_empty(), "the stream plays on");
        assert_eq!(h.app.sources.active_id(), Some(1));
        assert_eq!(h.app.storage.source(), 1);
        assert_eq!(h.app.catalog.kind(), SourceKind::Xtream);
        assert_eq!(h.app.creds, Some(test_creds()));
        assert_eq!(h.app.catalog_generation, generation);
        assert!(
            toast_text(&h).starts_with("could not write "),
            "{}",
            toast_text(&h)
        );
    }

    #[test]
    fn a_delete_the_ini_cannot_hold_takes_nothing() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        point_ini_nowhere(dir.path());
        let mut h = portal_and_list(dir.path());
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        std::fs::create_dir_all(two.cache_dir()).unwrap();
        let files = [two.favorites_path(), two.m3u_cache_path(2)];
        for path in &files {
            std::fs::write(path, "x").unwrap();
        }
        h.app.apply_settings_action(SettingsAction::DeleteSource(2));
        std::env::remove_var("BONGO_INI");
        assert!(h.app.sources.get(2).is_some(), "still saved");
        assert!(files.iter().all(|path| path.exists()), "and its files stay");
        assert!(
            toast_text(&h).starts_with("could not write "),
            "{}",
            toast_text(&h)
        );
    }

    #[test]
    fn a_save_for_a_source_that_is_not_saved_adds_none() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let mut h = portal_and_list(dir.path());
        let before = h.app.sources.clone();
        let draft = |kind, address: &str| SourceDraft {
            editing: Editing::Saved(9),
            kind,
            name: "Weg".into(),
            address: address.into(),
        };
        let list = draft(SourceKind::M3u, "https://b.example/tv.m3u");
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        // An Xtream source is not even tested.
        let portal = draft(SourceKind::Xtream, "u2:p2@127.0.0.1:1");
        h.app
            .apply_settings_action(SettingsAction::SaveSource(portal));
        assert_eq!(h.app.conn_state, ConnState::Idle);
        // A test that finished after its source was deleted keeps nothing either.
        let late = KeepXtream {
            creds: other_creds(),
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::Saved(9),
            name: "Weg".into(),
        };
        assert_eq!(h.app.keep_xtream(late), Err(ConnState::Idle));
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources, before);
        assert!(!ini.exists(), "nothing was written");
    }

    #[test]
    fn a_save_of_the_other_kind_than_the_saved_source_is_refused() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let mut h = portal_and_list(dir.path());
        let before = h.app.sources.clone();
        // Credentials for the list (source 2) ...
        h.app
            .apply_settings_action(SettingsAction::SaveSource(SourceDraft {
                editing: Editing::Saved(2),
                kind: SourceKind::Xtream,
                name: "Lijst".into(),
                address: "u2:p2@127.0.0.1:1".into(),
            }));
        assert_eq!(h.app.conn_state, ConnState::Idle, "no test starts");
        // ... and a list for the portal (source 1).
        let list = list_draft(Editing::Saved(1), "Portaal", "https://b.example/tv.m3u");
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources, before);
        assert!(!ini.exists(), "nothing was written");
    }

    #[test]
    fn a_list_saved_from_the_command_lines_form_is_a_new_source_and_the_portal_stays() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        h.app.creds_from_command_line = true;
        let list = list_draft(Editing::CommandLine, "Gratis", "https://a.example/tv.m3u");
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        std::env::remove_var("BONGO_INI");
        let portal = h.app.sources.get(1).unwrap();
        assert_eq!(
            (portal.kind, portal.address.as_str()),
            (SourceKind::Xtream, "u:p@h:80"),
            "the portal keeps its address"
        );
        assert_eq!(h.app.sources.get(2).map(|s| s.kind), Some(SourceKind::M3u));
        assert!(h.app.creds_from_command_line, "and keeps running");
        assert_eq!(h.app.catalog.kind(), SourceKind::Xtream);
    }

    #[test]
    fn an_address_with_nothing_in_it_is_not_kept() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let mut h = portal_and_list(dir.path());
        let before = h.app.sources.clone();
        for address in ["", "   ", "\r\n"] {
            let draft = list_draft(Editing::New, "Leeg", address);
            h.app
                .apply_settings_action(SettingsAction::SaveSource(draft));
        }
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources, before);
        assert!(!ini.exists(), "nothing was written");
    }

    #[test]
    fn a_source_with_no_number_left_for_it_is_not_kept() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        // A hand-edited ini holds the highest number there is.
        let sources = Sources::parse(
            "source=1;xtream;;u:p@h:80\nsource=4294967295;m3u;Groot;https://a.example/tv.m3u\nactive=1\n",
        );
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources.clone(),
            None,
        );
        let list = list_draft(Editing::New, "Nog een", "https://b.example/tv.m3u");
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        assert_eq!(toast_text(&h), "Settings could not be saved");
        h.app.toast = None;
        let portal = KeepXtream {
            creds: other_creds(),
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::New,
            name: "Nog een".into(),
        };
        let kept = h.app.keep_xtream(portal);
        std::env::remove_var("BONGO_INI");
        assert!(kept.is_err());
        assert_eq!(toast_text(&h), "Settings could not be saved");
        assert_eq!(h.app.sources, sources, "nothing was kept");
        assert!(!ini.exists(), "nothing was written");
    }

    #[tokio::test]
    async fn using_the_source_in_use_or_one_that_is_not_saved_changes_nothing() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let (mut h, _) = two_sources(dir.path());
        let generation = h.app.catalog_generation;
        for id in [1, 9] {
            h.app
                .apply_settings_action(SettingsAction::ActivateSource(id));
        }
        std::env::remove_var("BONGO_INI");
        assert!(h.sent().is_empty(), "the stream plays on");
        assert_eq!(h.app.catalog_generation, generation);
        assert!(!ini.exists(), "nothing was written");
    }

    #[tokio::test]
    async fn with_the_command_lines_portal_running_no_saved_source_is_in_use() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let (mut h, _) = two_sources(dir.path());
        h.app.creds_from_command_line = true;
        let generation = h.app.catalog_generation;
        // Source 1, with the same address, is a switch like any other: the
        // command line's portal goes.
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(1));
        std::env::remove_var("BONGO_INI");
        assert!(!h.app.creds_from_command_line);
        assert_eq!(h.app.catalog_generation, generation + 1);
        assert_eq!(Sources::load(&ini).active_id(), Some(1));
    }

    #[test]
    fn using_a_source_with_placeholder_credentials_shows_the_prompt_and_resumes_nothing() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let sources = Sources::parse(
            "source=1;xtream;;u:p@h:80\nsource=2;xtream;Voorbeeld;user:pass@host.example.com:80\nactive=1\n",
        );
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        );
        // Its last stream has the credentials in its address.
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        let record = LastWatched {
            url: "http://host.example.com:80/live/user/pass/7.m3u8".into(),
            live: true,
            adult: Some(false),
        };
        record.save(&two.last_watched_path());
        h.sent();
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(2));
        std::env::remove_var("BONGO_INI");
        assert_eq!(
            h.app.portal_state,
            PortalState::Placeholder,
            "the prompt shows"
        );
        let sent = h.sent();
        assert!(matches!(sent.as_slice(), [Cmd::Stop]), "{sent:?}");
    }

    #[tokio::test]
    async fn an_xtream_source_is_kept_once_it_logs_in_and_not_before() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        // A portal that takes every call as a login that works, and one that
        // refuses the login.
        let portal_answering = |status: u16| async move {
            let server = wiremock::MockServer::start().await;
            wiremock::Mock::given(wiremock::matchers::path("/player_api.php"))
                .respond_with(
                    wiremock::ResponseTemplate::new(status)
                        .set_body_string(r#"{"user_info":{"auth":1}}"#),
                )
                .mount(&server)
                .await;
            server
        };
        let (works, refuses) = (portal_answering(200).await, portal_answering(401).await);
        let draft = |server: &wiremock::MockServer| SourceDraft {
            editing: Editing::New,
            kind: SourceKind::Xtream,
            name: "Thuis".into(),
            address: format!("u2:p2@127.0.0.1:{}", server.address().port()),
        };
        let mut h = portal_and_list(dir.path());
        // A login that is refused keeps nothing.
        h.app
            .apply_settings_action(SettingsAction::SaveSource(draft(&refuses)));
        assert_eq!(h.app.conn_state, ConnState::Testing);
        wait_until("the verdict", || h.app.conn_slot.lock().is_some()).await;
        h.frame(&[]);
        assert_eq!(h.app.conn_state, ConnState::Done(ConnCheck::BadLogin));
        assert_eq!(h.app.sources.list().len(), 2);
        assert!(!ini.exists(), "nothing was written");
        // One that logs in is kept, and not used: a portal is in use.
        h.app
            .apply_settings_action(SettingsAction::SaveSource(draft(&works)));
        wait_until("the verdict", || h.app.conn_slot.lock().is_some()).await;
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", &ini);
            h.frame(&[]);
            std::env::remove_var("BONGO_INI");
        }
        let state = h.app.conn_state.clone();
        assert!(
            matches!(state, ConnState::Done(ConnCheck::Ok { .. })),
            "{state:?}"
        );
        let three = h.app.sources.get(3).expect("kept as source 3");
        assert_eq!(
            (three.kind, three.name.as_str()),
            (SourceKind::Xtream, "Thuis")
        );
        assert_eq!(h.app.sources.active_id(), Some(1), "and not used");
        assert_eq!(h.app.creds, Some(test_creds()));
        let saved = Sources::load(&ini);
        let address = draft(&works).address;
        assert_eq!(saved.get(3).map(|s| s.address.clone()), Some(address));
    }

    #[tokio::test]
    async fn an_xtream_source_in_use_saved_again_is_used_at_once() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let mut h = portal_and_list(dir.path());
        h.app.zap_by_id(3);
        h.sent();
        let generation = h.app.catalog_generation;
        let keep = KeepXtream {
            creds: other_creds(),
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::Saved(1),
            name: "Thuis".into(),
        };
        assert!(h.app.keep_xtream(keep).is_ok());
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.catalog_generation, generation + 1, "a new catalog");
        assert_eq!(h.app.creds, Some(other_creds()));
        assert!(h.sent().is_empty(), "TEST 03 plays on");
        assert_eq!(
            std::fs::read_to_string(&ini).unwrap(),
            "portal=u2:p2@127.0.0.1:1\n\
             source=1;xtream;Thuis;u2:p2@127.0.0.1:1\n\
             source=2;m3u;Lijst;https://a.example/tv.m3u\n\
             active=1\n"
        );
    }

    #[tokio::test]
    async fn a_switch_ends_the_wait_for_a_catch_up_pick_of_the_portal_before() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = Harness::start_with(
            dir.path(),
            guide_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        pick_catch_up(&mut h);
        assert!(h.app.pending_catchup.is_some(), "the wait screen shows");
        // The login of a Save of the portal in use comes in meanwhile: a new
        // catalog, in which the programme waited for is no programme.
        let keep = KeepXtream {
            creds: other_creds(),
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::Saved(1),
            name: String::new(),
        };
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            assert!(h.app.keep_xtream(keep).is_ok());
            std::env::remove_var("BONGO_INI");
        }
        assert!(h.app.pending_catchup.is_none(), "the old portal's wait");
        h.frame(&[]);
        let waits = h.painted.iter().any(|t| t == "loading the programme…");
        assert!(!waits, "{:?}", h.painted);
        // Another source's data, as a switch takes it: the same.
        let other = tempfile::tempdir().unwrap();
        let mut h = Harness::start(other.path(), guide_catalog(), PortalState::Configured, None);
        pick_catch_up(&mut h);
        assert!(h.app.pending_catchup.is_some());
        h.app.use_data_of(2);
        assert!(h.app.pending_catchup.is_none(), "source 1's wait");
    }

    #[tokio::test]
    async fn a_portal_save_stops_adult_playback_and_leaves_other_streams_playing() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        for (sid, stops) in [(3, false), (901, true)] {
            let root = tempfile::tempdir().unwrap();
            let mut h =
                Harness::start(root.path(), loaded_catalog(), PortalState::Configured, None);
            h.app.set_adult_visible(true);
            h.app.zap_by_id(sid);
            h.sent();
            let keep = portal_save(other_creds(), "u2:p2@127.0.0.1:1");
            assert!(h.app.keep_xtream(keep).is_ok());
            let stopped = h.sent().iter().any(|c| matches!(c, Cmd::Stop));
            assert_eq!(stopped, stops, "channel {sid}");
            assert_eq!(h.app.current_url.is_none(), stops, "channel {sid}");
        }
        std::env::remove_var("BONGO_INI");
    }

    #[tokio::test]
    async fn after_the_setup_prompts_login_the_pin_reset_finds_its_portal_on_a_list() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        assert!(h
            .app
            .keep_xtream(portal_save(other_creds(), "u2:p2@127.0.0.1:1"))
            .is_ok());
        // The portal is in use: the list is only added, then put in use.
        let list = list_draft(Editing::New, "Gratis", &file.display().to_string());
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        h.app
            .apply_settings_action(SettingsAction::ActivateSource(2));
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.creds, None, "a list is in use");
        h.app.settings.pin = Some(PinHash::new("1234").unwrap());
        h.app
            .apply_settings_action(SettingsAction::ResetPin("p2".into()));
        assert!(h.app.settings.pin.is_none(), "the saved portal's password");
    }

    #[tokio::test]
    async fn the_log_names_a_source_by_its_number_and_kind_never_by_its_address() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        std::fs::write(two.favorites_path(), "x").unwrap();
        let address = "https://user:secret@lists.example/tv.m3u?token=abc123";
        let log = LogCapture::start();
        for action in [
            SettingsAction::SaveSource(list_draft(Editing::New, "Gratis", address)),
            SettingsAction::ActivateSource(2),
            SettingsAction::ActivateSource(1),
            SettingsAction::DeleteSource(1),
            SettingsAction::DeleteSource(2),
        ] {
            h.app.apply_settings_action(action);
        }
        std::env::remove_var("BONGO_INI");
        let text = log.text();
        for line in [
            "settings: source 2 saved (m3u)",
            "source: active 2 (m3u)",
            "source: m3u 2 (lists.example)",
            "source: active 1 (xtream)",
            "settings: source 1 is in use - not deleted",
            "settings: source 2 deleted (1 files)",
        ] {
            assert!(text.contains(line), "{line:?} is not in the log:\n{text}");
        }
        for secret in ["secret", "abc123", "tv.m3u"] {
            assert!(!text.contains(secret), "{secret} in the log:\n{text}");
        }
    }

    #[tokio::test]
    async fn a_list_added_at_the_setup_prompt_is_used_with_its_own_data() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        // The saved portal has the stand-in credentials: the setup prompt shows.
        let sources = Sources::parse("source=1;xtream;;user:pass@host.example.com:80\nactive=1\n");
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start_with(
            dir.path(),
            catalog,
            PortalState::Placeholder,
            None,
            None,
            sources,
            None,
        );
        h.app.favorites.toggle(3, "TEST 03"); // source 1's, in memory
        let list = list_draft(Editing::New, "Gratis", &file.display().to_string());
        h.app
            .apply_settings_action(SettingsAction::SaveSource(list));
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources.active_id(), Some(2));
        assert_eq!(h.app.portal_state, PortalState::Configured);
        assert_eq!(h.app.storage.source(), 2, "its own favorites and presets");
        assert!(!h.app.favorites.contains(3), "not source 1's");
    }

    /// A portable zip unzipped into a new folder: no ini beside the exe, but the
    /// config folder holds the favorites 0.6.1's portal left (`favorites.json`,
    /// source 1's). The setup prompt shows. Also the list `lijst.m3u` in `dir`.
    fn a_new_folder_with_old_favorites(dir: &std::path::Path) -> (Harness, String) {
        let mut old = Favorites::default();
        old.toggle(3, "TEST 03");
        let storage = Storage::with_root(dir.to_path_buf());
        old.save(&storage.favorites_path()).unwrap();
        let file = dir.join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let h = Harness::start(dir, catalog, PortalState::Missing, None);
        (h, file.display().to_string())
    }

    #[tokio::test]
    async fn a_list_saved_from_the_setup_prompts_link_never_takes_the_portals_files() {
        let env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let (mut h, file) = a_new_folder_with_old_favorites(dir.path());
        // The prompt's link (Connect, disabled, is no stop), the list's address
        // in the form it opens, Save.
        h.press(&[Key::Tab, Key::Tab, Key::Tab, Key::Tab, Key::Enter]);
        h.type_text(&file);
        h.press(&[Key::Tab, Key::Enter]);
        std::env::remove_var("BONGO_INI");
        drop(env);
        assert_eq!(h.app.catalog.kind(), SourceKind::M3u, "the list is in use");
        assert_ne!(
            h.app.storage.source(),
            1,
            "source 1's files are the portal's"
        );
        wait_until("the list", || h.app.catalog.is_loaded()).await;
        let shown = shown_favorites(&h.app.favorites, &h.app.catalog);
        assert!(
            shown.is_empty(),
            "the portal's favorites on the list: {shown:?}"
        );
    }

    #[tokio::test]
    async fn a_portal_saved_after_that_list_is_source_1_with_its_old_favorites() {
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let (mut h, file) = a_new_folder_with_old_favorites(dir.path());
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            let list = list_draft(Editing::New, "Gratis", &file);
            h.app
                .apply_settings_action(SettingsAction::SaveSource(list));
            std::env::remove_var("BONGO_INI");
        }
        assert_eq!(h.app.catalog.kind(), SourceKind::M3u, "the list is in use");
        // A portal added in Bronnen: tested, kept, not used.
        let portal = SourceDraft {
            editing: Editing::New,
            kind: SourceKind::Xtream,
            name: "Thuis".into(),
            address: format!("u2:p2@127.0.0.1:{}", server.address().port()),
        };
        h.app
            .apply_settings_action(SettingsAction::SaveSource(portal));
        take_in_the_login(&mut h, dir.path()).await;
        let kind = h.app.sources.get(1).map(|s| s.kind);
        assert_eq!(kind, Some(SourceKind::Xtream), "the portal is source 1");
        {
            let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
            h.app
                .apply_settings_action(SettingsAction::ActivateSource(1));
            std::env::remove_var("BONGO_INI");
        }
        assert_eq!(h.app.storage.source(), 1);
        assert!(
            h.app.favorites.contains(3),
            "0.6.1's favorites are the portal's"
        );
    }

    #[tokio::test]
    async fn a_login_kept_as_a_source_whose_data_is_not_in_use_takes_that_sources_data() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        // `--xtream` runs on source 1's files; what it saves goes into the first
        // saved portal, source 2.
        let sources = Sources::parse(
            "source=1;m3u;Lijst;https://a.example/tv.m3u\nsource=2;xtream;;u:p@h:80\nactive=1\n",
        );
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        );
        h.app.creds_from_command_line = true;
        h.app.favorites.toggle(5, "TEST 05"); // source 1's, in memory
        let mut two = Storage::with_root(dir.path().to_path_buf());
        two.set_source(2);
        let record = LastWatched {
            url: live_url(3),
            live: true,
            adult: Some(false),
        };
        record.save(&two.last_watched_path());
        h.sent();
        let keep = portal_save(other_creds(), "u2:p2@127.0.0.1:1");
        assert!(h.app.keep_xtream(keep).is_ok());
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources.active_id(), Some(2));
        assert_eq!(h.app.storage.source(), 2);
        assert!(!h.app.favorites.contains(5), "not source 1's");
        let sent = h.sent();
        assert!(matches!(sent.first(), Some(Cmd::Stop)), "{sent:?}");
        assert_eq!(loads(sent), [live_url(3)], "source 2's last channel");
    }

    #[test]
    fn an_xtream_source_saved_under_its_kinds_default_name_is_kept_as_none() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_list(dir.path());
        let keep = KeepXtream {
            creds: other_creds(),
            line: "u2:p2@127.0.0.1:1".into(),
            editing: Editing::New,
            name: " Portal ".into(),
        };
        assert!(h.app.keep_xtream(keep).is_ok());
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources.get(3).map(|s| s.name.as_str()), Some(""));
    }

    // ---- the settings' sources section, end to end ----

    #[tokio::test]
    async fn an_m3u_list_is_added_and_used_by_keyboard_alone() {
        // The keys of the e2e scenario M-add, with a file in place of the mock's URL.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        // The scenario's ini: the portal saved as source 1, in use.
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..6 {
            h.press(&[Key::Tab]);
        }
        h.press(&[Key::Enter, Key::Tab, Key::Enter, Key::Tab]);
        h.type_text(&file.display().to_string());
        h.press(&[Key::Tab, Key::Enter]);
        assert_eq!(h.app.sources.list().len(), 2, "saved as source 2");
        assert_eq!(
            h.app.catalog.kind(),
            SourceKind::Xtream,
            "the portal still runs"
        );
        for _ in 0..5 {
            h.press(&[Key::Tab]);
        }
        h.press(&[Key::Enter]);
        assert_eq!(h.app.sources.active_id(), Some(2));
        assert_eq!(h.app.catalog.kind(), SourceKind::M3u);
        std::env::remove_var("BONGO_INI");
    }

    #[test]
    fn the_setup_prompts_link_opens_the_sources_section_on_a_new_list() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        // Server, user, password (Connect is disabled while they are empty), the link.
        h.press(&[Key::Tab, Key::Tab, Key::Tab, Key::Tab, Key::Enter]);
        assert!(h.app.settings_ui.is_some(), "the settings opened");
        h.frame(&[]);
        assert!(
            !h.painted.iter().any(|t| t == "Set up your portal"),
            "the prompt is not drawn under the settings: {:?}",
            h.painted
        );
        h.type_text("x");
        let form = h.app.settings_ui.as_ref().unwrap().form_address();
        assert_eq!(form, Some((SourceKind::M3u, "x".to_owned())));
    }

    #[tokio::test]
    async fn enter_on_the_setup_prompts_link_does_not_also_submit_the_form() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        h.app.cfg_host = "127.0.0.1:1".into();
        h.app.cfg_user = "u".into();
        h.app.cfg_pass = "p".into();
        h.frame(&[]);
        // Filled in: Connect is a stop now, before the link.
        h.press(&[Key::Tab, Key::Tab, Key::Tab, Key::Tab, Key::Tab, Key::Enter]);
        assert!(h.app.settings_ui.is_some());
        assert_eq!(h.app.conn_state, ConnState::Idle, "no test started");
    }

    #[test]
    fn the_prompt_is_back_when_the_settings_opened_from_its_link_close() {
        let dir = tempfile::tempdir().unwrap();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        h.press(&[Key::Tab, Key::Tab, Key::Tab, Key::Tab, Key::Enter]);
        assert!(h.app.settings_ui.is_some());
        // Esc leaves the section, the next Esc closes the settings.
        h.press(&[Key::Escape, Key::Escape]);
        assert!(h.app.settings_ui.is_none());
        assert!(
            h.painted.iter().any(|t| t == "Set up your portal"),
            "{:?}",
            h.painted
        );
    }

    /// A portal that answers every call with this status (and a body that is a
    /// login that works, for a 200).
    async fn portal_answering(status: u16) -> wiremock::MockServer {
        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::path("/player_api.php"))
            .respond_with(
                wiremock::ResponseTemplate::new(status)
                    .set_body_string(r#"{"user_info":{"auth":1}}"#),
            )
            .mount(&server)
            .await;
        server
    }

    /// A portal that takes every call as a login that works.
    async fn portal_that_logs_in() -> wiremock::MockServer {
        portal_answering(200).await
    }

    /// The test of a login has come in; the frame that takes it in writes the
    /// ini, so it points into `dir` (and holds ENV_LOCK for that frame only).
    async fn take_in_the_login(h: &mut Harness, dir: &std::path::Path) {
        wait_until("the verdict", || h.app.conn_slot.lock().is_some()).await;
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        std::env::set_var("BONGO_INI", dir.join("test.ini"));
        h.frame(&[]);
        std::env::remove_var("BONGO_INI");
    }

    #[tokio::test]
    async fn the_setup_prompts_login_goes_into_the_portal_in_use_under_its_name() {
        // A hand-edited ini: source 1 is a good portal, source 2, in use, has
        // the stand-in credentials, so the prompt shows. The login belongs to
        // source 2, not to the first portal.
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let port = server.address().port();
        let sources = Sources::parse(
            "source=1;xtream;Thuis;u:p@h:80\n\
             source=2;xtream;Werk;user:pass@host.example.com:80\n\
             active=2\n",
        );
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start_with(
            dir.path(),
            catalog,
            PortalState::Placeholder,
            None,
            None,
            sources,
            None,
        );
        h.app.cfg_host = format!("127.0.0.1:{port}");
        h.app.cfg_user = "u2".into();
        h.app.cfg_pass = "p2".into();
        h.frame(&[]);
        h.frame(&[Key::Enter]);
        assert_eq!(h.app.conn_state, ConnState::Testing);
        take_in_the_login(&mut h, dir.path()).await;
        let sources = &h.app.sources;
        let line = |id| sources.get(id).map(|s| (s.name.clone(), s.address.clone()));
        assert_eq!(line(1), Some(("Thuis".into(), "u:p@h:80".into())));
        assert_eq!(
            line(2),
            Some(("Werk".into(), format!("u2:p2@127.0.0.1:{port}")))
        );
        assert_eq!(h.app.sources.list().len(), 2);
        assert_eq!(h.app.sources.active_id(), Some(2));
        assert_eq!(h.app.portal_state, PortalState::Configured);
        let saved = Sources::load(&dir.path().join("test.ini"));
        assert_eq!(saved.get(1).map(|s| s.address.as_str()), Some("u:p@h:80"));
    }

    #[tokio::test]
    async fn the_setup_prompts_login_without_a_saved_portal_is_the_first_one() {
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let port = server.address().port();
        let catalog = Arc::new(CatalogStore::new(idle_portal()));
        let mut h = Harness::start(dir.path(), catalog, PortalState::Missing, None);
        h.app.cfg_host = format!("127.0.0.1:{port}");
        h.app.cfg_user = "u2".into();
        h.app.cfg_pass = "p2".into();
        h.frame(&[]);
        h.frame(&[Key::Enter]);
        take_in_the_login(&mut h, dir.path()).await;
        assert_eq!(
            std::fs::read_to_string(dir.path().join("test.ini")).unwrap(),
            format!("portal=u2:p2@127.0.0.1:{port}\n"),
            "0.6.1's ini"
        );
        assert_eq!(h.app.sources.active_id(), Some(1));
        assert_eq!(h.app.portal_state, PortalState::Configured);
    }

    #[tokio::test]
    async fn save_on_the_command_line_row_keeps_the_name_of_the_first_portal() {
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let port = server.address().port();
        let creds = XtreamCreds {
            username: "u2".into(),
            password: "p2".into(),
            host: "127.0.0.1".into(),
            port,
        };
        let sources = Sources::parse("source=1;xtream;Thuis;u:p@h:80\nactive=1\n");
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(creds),
            sources,
            None,
        );
        h.app.creds_from_command_line = true;
        // The section opens on the --xtream row; user, password, Test, Save.
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..4 {
            h.press(&[Key::Tab]);
        }
        h.press(&[Key::Enter]);
        assert_eq!(h.app.conn_state, ConnState::Testing);
        take_in_the_login(&mut h, dir.path()).await;
        assert_eq!(h.app.sources.list().len(), 1, "the first portal is updated");
        let one = h.app.sources.get(1).unwrap();
        assert_eq!(
            (one.name.as_str(), one.address.as_str()),
            ("Thuis", format!("u2:p2@127.0.0.1:{port}").as_str())
        );
        assert!(!h.app.creds_from_command_line, "the saved portal runs now");
    }

    #[tokio::test]
    async fn an_xtream_line_is_tested_as_it_is_kept() {
        // `Sources::keep` stores the line without line breaks and the spaces
        // around it; the test must be of that line, not of the one as typed.
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let port = server.address().port();
        let mut h = portal_and_list(dir.path());
        h.app
            .apply_settings_action(SettingsAction::SaveSource(SourceDraft {
                editing: Editing::New,
                kind: SourceKind::Xtream,
                name: "Thuis".into(),
                address: format!("  u2:p2@127.0.0.1:{port} \r\n"),
            }));
        assert_eq!(h.app.conn_state, ConnState::Testing, "a line to test");
        take_in_the_login(&mut h, dir.path()).await;
        let kept = h.app.sources.get(3).map(|s| s.address.clone());
        assert_eq!(kept, Some(format!("u2:p2@127.0.0.1:{port}")));
        // The login that was tried is the kept one's, not "  u2".
        let asked = server.received_requests().await.unwrap();
        let query = asked.first().and_then(|r| r.url.query().map(str::to_owned));
        assert_eq!(query.as_deref(), Some("username=u2&password=p2"));
    }

    #[test]
    fn deleting_a_source_with_the_keyboard_leaves_the_form_on_the_source_in_use() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_list(dir.path());
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..6 {
            h.press(&[Key::Tab]); // user, password, Test, Save, row 1, row 2
        }
        h.press(&[Key::Enter]); // row 2, the list
        for _ in 0..7 {
            h.press(&[Key::Tab]); // address, Save, row 1, row 2, Use, New, Delete
        }
        h.press(&[Key::Enter]); // asks
        assert_eq!(h.app.sources.list().len(), 2);
        h.press(&[Key::Enter]); // deletes
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources.list().len(), 1);
        // The form shows the portal in use again, the keyboard on its name: the
        // Delete button it was on is disabled now and keeps no focus.
        let shown = h.app.settings_ui.as_ref().unwrap().form_address();
        assert_eq!(shown, Some((SourceKind::Xtream, "h:80".to_owned())));
        h.type_text("x");
        h.frame(&[]);
        assert!(
            h.painted.iter().any(|p| p == "Portalx"),
            "a typed character lands in the name: {:?}",
            h.painted
        );
        let shown = h.app.settings_ui.as_ref().unwrap().form_address();
        assert_eq!(shown, Some((SourceKind::Xtream, "h:80".to_owned())));
    }

    // ---- the sources section: the verdict, a late answer, keys without pauses ----

    /// Into a new Xtream source's form in the settings, from the portal in use
    /// (`portal_and_list`), with this server, and Save pressed: its login is on
    /// its way (nothing runs until the test awaits). The Save button is disabled
    /// while the login is tried, and has taken the keyboard along.
    fn save_a_new_portal(h: &mut Harness, server_port: u16) {
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..7 {
            h.press(&[Key::Tab]); // user, password, Test, Save, row 1, row 2, New source
        }
        h.press(&[Key::Enter]); // a new source: its Xtream radio
        h.press(&[Key::Tab, Key::Tab]); // the M3U radio, the server field
        h.type_text(&format!("127.0.0.1:{server_port}"));
        h.press(&[Key::Tab]);
        h.type_text("u2");
        h.press(&[Key::Tab]);
        h.type_text("p2");
        h.press(&[Key::Tab, Key::Tab, Key::Enter]); // Test, Save
        assert_eq!(h.app.conn_state, ConnState::Testing);
    }

    /// Shift+Tab, and the quiet frame in which the focus has moved.
    fn press_shift_tab(h: &mut Harness) {
        let shift = egui::Modifiers::SHIFT;
        let tab = egui::Event::Key {
            key: Key::Tab,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: shift,
        };
        h.frame_holding(shift, vec![tab]);
        h.frame(&[]);
    }

    #[tokio::test]
    async fn a_failed_save_shows_its_failure_under_the_form_that_asked_and_nowhere_else() {
        let dir = tempfile::tempdir().unwrap();
        let server = portal_answering(401).await;
        let mut h = portal_and_list(dir.path());
        save_a_new_portal(&mut h, server.address().port());
        take_in_the_login(&mut h, dir.path()).await;
        assert_eq!(h.app.conn_state, ConnState::Done(ConnCheck::BadLogin));
        let failure = "Wrong username or password";
        let says = |h: &Harness| h.painted.iter().any(|p| p == failure);
        assert!(says(&h), "under the form that asked: {:?}", h.painted);
        // The keyboard went with the Save button, which is disabled while a login is
        // tried. Shift+Tab from nowhere goes to the last stop (New source), then back
        // to row 2 and row 1: the portal in use, which is fine.
        for _ in 0..3 {
            press_shift_tab(&mut h);
        }
        h.press(&[Key::Enter]);
        let shown = h.app.settings_ui.as_ref().unwrap().form_address();
        assert_eq!(shown, Some((SourceKind::Xtream, "h:80".to_owned())));
        assert!(!says(&h), "{:?}", h.painted);
    }

    #[tokio::test]
    async fn a_failed_save_whose_form_is_no_longer_on_show_says_so_in_a_toast() {
        let failure = "Wrong username or password";
        let server = portal_answering(401).await;
        let port = server.address().port();
        // Where the user is when the answer comes: under the form that asked,
        // in another form, or out of the settings.
        for moved in ["stays", "another form", "settings closed"] {
            let dir = tempfile::tempdir().unwrap();
            let mut h = portal_and_list(dir.path());
            save_a_new_portal(&mut h, port);
            match moved {
                "another form" => {
                    // Shift+Tab from nowhere goes to the last stop, New source.
                    press_shift_tab(&mut h);
                    h.press(&[Key::Enter]);
                }
                "settings closed" => h.press(&[Key::Escape, Key::Escape]),
                _ => {}
            }
            take_in_the_login(&mut h, dir.path()).await;
            assert_eq!(h.app.conn_state, ConnState::Done(ConnCheck::BadLogin));
            let toasted = toast_text(&h) == failure;
            assert_eq!(toasted, moved != "stays", "{moved}: {}", toast_text(&h));
        }
    }

    #[tokio::test]
    async fn a_late_login_does_not_replace_the_form_the_user_moved_on_to() {
        let dir = tempfile::tempdir().unwrap();
        let server = portal_that_logs_in().await;
        let mut h = portal_and_list(dir.path());
        save_a_new_portal(&mut h, server.address().port());
        // The login is on its way. Meanwhile the user starts another new source. The
        // Save button is disabled for now and has taken the keyboard along:
        // Shift+Tab from nowhere goes to the last stop, New source.
        press_shift_tab(&mut h);
        h.press(&[Key::Enter]);
        h.press(&[Key::Tab, Key::Tab]); // the M3U radio, the server field
        h.type_text("x");
        take_in_the_login(&mut h, dir.path()).await;
        // The portal is kept all the same, and listed ...
        assert_eq!(h.app.sources.list().len(), 3);
        assert!(
            h.painted.iter().any(|p| p == "Portal · Xtream"),
            "{:?}",
            h.painted
        );
        // ... but the new form is where the user left it, with its character, and
        // the keyboard is still in its field.
        let host = |s: &str| Some((SourceKind::Xtream, s.to_owned()));
        assert_eq!(
            h.app.settings_ui.as_ref().unwrap().form_address(),
            host("x")
        );
        h.type_text("y");
        assert_eq!(
            h.app.settings_ui.as_ref().unwrap().form_address(),
            host("xy")
        );
    }

    #[tokio::test]
    async fn the_keys_of_m_add_work_with_no_pause_between_the_save_and_the_tabs() {
        // The keys of the e2e scenario M-add, each in a frame of its own, the Tabs
        // right behind the Enter that saves: no quiet frame between them.
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let mut h = Harness::start_with(
            dir.path(),
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            Sources::parse("portal=u:p@h:80\n"),
            None,
        );
        let keys = |h: &mut Harness, keys: &[Key]| {
            for &key in keys {
                h.frame(&[key]);
            }
        };
        h.press(&[Key::O]);
        keys(&mut h, &[Key::ArrowDown, Key::Enter]);
        keys(&mut h, &[Key::Tab; 6]);
        keys(&mut h, &[Key::Enter, Key::Tab, Key::Enter, Key::Tab]);
        h.type_text(&file.display().to_string());
        keys(&mut h, &[Key::Tab, Key::Enter]); // Save
        assert_eq!(h.app.sources.list().len(), 2, "saved as source 2");
        keys(&mut h, &[Key::Tab; 5]); // address, Save, row 1, row 2, Use this source
        keys(&mut h, &[Key::Enter]);
        assert_eq!(h.app.sources.active_id(), Some(2));
        assert_eq!(h.app.catalog.kind(), SourceKind::M3u);
        std::env::remove_var("BONGO_INI");
    }

    /// Source 1, the tests' portal, in use, and source 2, a list in a file in
    /// `dir`. Nothing in it writes the ini or asks the network: hold ENV_LOCK
    /// with `BONGO_INI` in `dir` for what does.
    fn portal_and_a_list_in_a_file(dir: &std::path::Path) -> Harness {
        let file = dir.join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let sources = Sources::parse(&format!(
            "source=1;xtream;;u:p@h:80\nsource=2;m3u;Lijst;{}\nactive=1\n",
            file.display()
        ));
        Harness::start_with(
            dir,
            loaded_catalog(),
            PortalState::Configured,
            None,
            Some(test_creds()),
            sources,
            None,
        )
    }

    #[tokio::test]
    async fn a_tab_in_the_frame_after_a_use_moves_on_from_the_name() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_a_list_in_a_file(dir.path());
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..6 {
            h.press(&[Key::Tab]); // user, password, Test, Save, row 1, row 2
        }
        h.press(&[Key::Enter]); // the list
        for _ in 0..5 {
            h.frame(&[Key::Tab]); // address, Save, row 1, row 2, Use this source
        }
        h.frame(&[Key::Enter]); // the list is in use now: the form shows it again
        assert_eq!(h.app.sources.active_id(), Some(2));
        // The very next frame has the Tab, then a character: it goes to the address.
        h.frame(&[Key::Tab]);
        h.type_text("x");
        std::env::remove_var("BONGO_INI");
        let shown = h.app.settings_ui.as_ref().unwrap().form_address();
        let address = shown.map(|(_, address)| address).unwrap_or_default();
        assert!(address.ends_with("lijst.m3ux"), "{address}");
    }

    #[test]
    fn a_tab_in_the_frame_after_a_delete_moves_on_from_the_name() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("BONGO_INI", dir.path().join("test.ini"));
        let mut h = portal_and_a_list_in_a_file(dir.path());
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..6 {
            h.press(&[Key::Tab]); // user, password, Test, Save, row 1, row 2
        }
        h.press(&[Key::Enter]); // the list
        for _ in 0..7 {
            h.frame(&[Key::Tab]); // address, Save, row 1, row 2, Use, New, Delete
        }
        h.frame(&[Key::Enter]); // asks
        h.frame(&[Key::Enter]); // deletes: the form shows the portal in use again
        assert_eq!(h.app.sources.list().len(), 1);
        // The very next frame has the Tab, then a character: it goes to the server.
        h.frame(&[Key::Tab]);
        h.type_text("x");
        std::env::remove_var("BONGO_INI");
        let shown = h.app.settings_ui.as_ref().unwrap().form_address();
        assert_eq!(shown, Some((SourceKind::Xtream, "h:80x".to_owned())));
    }

    #[test]
    fn a_default_name_made_in_one_language_is_kept_as_none_in_another() {
        let _env = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("test.ini");
        std::env::set_var("BONGO_INI", &ini);
        let file = dir.path().join("lijst.m3u");
        std::fs::write(&file, TEST_LIST).unwrap();
        let mut h = portal_and_list(dir.path());
        h.app.lang = Lang::Nl;
        h.press(&[Key::O, Key::ArrowDown, Key::Enter]);
        for _ in 0..7 {
            h.press(&[Key::Tab]); // user, password, Test, Save, row 1, row 2, New source
        }
        h.press(&[Key::Enter, Key::Tab, Key::Enter, Key::Tab]); // a new list: "M3U-lijst"
        h.type_text(&file.display().to_string());
        // The language is switched while the form is open, as in the Language section.
        h.app
            .apply_settings_action(SettingsAction::SetLanguage(Lang::En));
        h.press(&[Key::Tab, Key::Enter]); // Save
        std::env::remove_var("BONGO_INI");
        assert_eq!(h.app.sources.get(3).map(|s| s.name.as_str()), Some(""));
        let saved = std::fs::read_to_string(&ini).unwrap();
        assert!(saved.contains("source=3;m3u;;"), "{saved}");
        assert!(
            h.painted.iter().any(|p| p == "M3U list · M3U"),
            "the row shows the English default: {:?}",
            h.painted
        );
    }
}
