// No console window by default - clean for end users. Pass --console for a
// live debug console; logs always also go to the file under %APPDATA%.
#![cfg_attr(windows, windows_subsystem = "windows")]

use bongo_player::app::{open_source, open_xtream, OpenedSource, TvApp};
use bongo_player::args::{ini_path, resolve_creds, Cli, CredsSource};
use bongo_player::catalog::CatalogStore;
use bongo_player::legacy;
use bongo_player::player;
use bongo_player::relay::{Relay, RelayConfig};
use bongo_player::settings::Settings;
use bongo_player::sources::Sources;
use bongo_player::storage::Storage;
use bongo_player::{APP_ID, APP_NAME};
use clap::Parser;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};

/// Optional debug console (opt-in via --console). Without it the binary is a
/// windowed app with no console at all. Logs always also go to the file.
#[cfg(windows)]
mod win_console {
    use std::os::raw::c_void;
    #[link(name = "kernel32")]
    extern "system" {
        fn AllocConsole() -> i32;
        fn SetStdHandle(which: u32, handle: *mut c_void) -> i32;
        fn CreateFileA(
            name: *const u8,
            access: u32,
            share: u32,
            sec: *mut c_void,
            disposition: u32,
            flags: u32,
            template: *mut c_void,
        ) -> *mut c_void;
    }

    /// Allocate a console and point stdout/stderr at it so tracing's stderr
    /// layer becomes visible. Call before init_logging.
    pub fn alloc() {
        const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5; // (DWORD)-11
        const STD_ERROR_HANDLE: u32 = 0xFFFF_FFF4; // (DWORD)-12
        const GENERIC_READ: u32 = 0x8000_0000;
        const GENERIC_WRITE: u32 = 0x4000_0000;
        const FILE_SHARE_READ: u32 = 0x1;
        const FILE_SHARE_WRITE: u32 = 0x2;
        const OPEN_EXISTING: u32 = 3;
        unsafe {
            if AllocConsole() == 0 {
                return; // already attached to a console
            }
            let h = CreateFileA(
                b"CONOUT$\0".as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null_mut(),
                OPEN_EXISTING,
                0,
                std::ptr::null_mut(),
            );
            if !h.is_null() && h as isize != -1 {
                SetStdHandle(STD_OUTPUT_HANDLE, h);
                SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }
}

/// Initialize tracing: stderr + rolling daily file under
/// %APPDATA%\BongoIPTVPlayer\config\log\. Returns the log directory path so callers
/// can show it in the UI / panic hook.
fn init_logging(storage: &Storage) -> std::path::PathBuf {
    let log_dir = storage.config_dir().join("log");
    std::fs::create_dir_all(&log_dir).ok();

    // Sync rolling appender: writes are line-flushed so a panic-abort still
    // leaves the last few seconds of context on disk.
    let file_appender = tracing_appender::rolling::daily(&log_dir, format!("{APP_ID}.log"));

    // Targets are crate names (lib + this bin). Derived via module_path!() so a
    // rename can't silently mute the app's own logs.
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| {
        EnvFilter::new(format!(
            "{}=debug,{}=debug,reqwest=warn,hyper=warn",
            bongo_player::LOG_TARGET,
            module_path!()
        ))
    });

    let stderr_layer = fmt::layer()
        .with_target(false)
        .with_thread_names(true)
        .with_writer(std::io::stderr);

    let file_layer = fmt::layer()
        .with_target(true)
        .with_thread_names(true)
        .with_ansi(false)
        .with_writer(file_appender);

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .init();

    log_dir
}

fn install_panic_hook(log_dir: std::path::PathBuf) {
    std::panic::set_hook(Box::new(move |info| {
        let bt = std::backtrace::Backtrace::force_capture();
        let payload = if let Some(s) = info.payload().downcast_ref::<&str>() {
            (*s).to_owned()
        } else if let Some(s) = info.payload().downcast_ref::<String>() {
            s.clone()
        } else {
            "<non-string panic payload>".to_owned()
        };
        let loc = info
            .location()
            .map(|l| format!("{}:{}:{}", l.file(), l.line(), l.column()))
            .unwrap_or_else(|| "<unknown location>".to_owned());

        tracing::error!("PANIC at {}: {}\n{}", loc, payload, bt);

        // Also write a standalone panic file so the user can find it without
        // grepping the daily rolling log.
        let ts = chrono::Utc::now().format("%Y%m%dT%H%M%SZ");
        let panic_path = log_dir.join(format!("panic-{}.log", ts));
        let _ = std::fs::write(
            &panic_path,
            format!(
                "PANIC at {}\nmessage: {}\nbacktrace:\n{}\n",
                loc, payload, bt
            ),
        );
        eprintln!("PANIC -> {}", panic_path.display());
    }));
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();

    if cli.selftest {
        println!("selftest: args parsed OK");
        return Ok(());
    }

    // Windowed binary: no console by default. --console opens one with live
    // logs (the file log under %APPDATA%\BongoIPTVPlayer\config\log\ is always
    // written).
    #[cfg(windows)]
    if cli.console {
        win_console::alloc();
    }

    let storage = Storage::standard()?;
    // First start after a rename (tvplayer, then Bongo Player): carry over the
    // newest earlier name's data and ini. Must run before ensure_config_dir (an
    // existing config dir means "already migrated"); results are logged once
    // logging is up. Under the test override no earlier data may be copied in.
    let isolated = std::env::var_os("BONGO_CONFIG_DIR").is_some();
    let (migrated_config, migrated_ini) = if isolated {
        (None, None)
    } else {
        let dirs: Vec<(&str, std::path::PathBuf)> = legacy::LEGACY_APP_IDS
            .iter()
            .filter_map(|id| legacy::legacy_config_dir(id).map(|dir| (*id, dir)))
            .collect();
        let inis: Vec<(&str, std::path::PathBuf)> = legacy::LEGACY_APP_IDS
            .iter()
            .map(|id| (*id, legacy::legacy_ini_path(id)))
            .collect();
        (
            legacy::migrate_config_from(&dirs, storage.config_dir()),
            legacy::migrate_ini_from(&inis, &ini_path()),
        )
    };
    storage.ensure_config_dir()?;
    let log_dir = init_logging(&storage);
    install_panic_hook(log_dir.clone());

    tracing::info!("{} v{} starting", APP_NAME, env!("CARGO_PKG_VERSION"));
    tracing::info!("config dir: {}", storage.config_dir().display());
    tracing::info!("log dir:    {}", log_dir.display());
    match migrated_config {
        Some((id, Ok(n))) if n > 0 => tracing::info!("migrated {n} files from the {id} config dir"),
        Some((id, Err(e))) => tracing::warn!("could not migrate the {id} config dir: {e}"),
        _ => {}
    }
    match migrated_ini {
        Some((id, Ok(true))) => tracing::info!("renamed {id}.ini to {}", ini_path().display()),
        Some((id, Err(e))) => tracing::warn!("could not rename {id}.ini: {e}"),
        _ => {}
    }

    let rt = Runtime::new()?;
    let _rt_guard = rt.enter();

    // The thuisbioscoop's covers folder keeps to its 500 MB (spec 2026-10-15
    // §4.4): the oldest files go, in the background.
    bongo_player::vod::covers::prune_in_background(storage.cache_dir().join("covers"));

    // The portal of --xtream with real values wins (see args::resolve_creds),
    // then the source in use in BongoIPTVPlayer.ini. Placeholder, missing or
    // unreadable credentials never stop the app: it shows the setup prompt,
    // which writes BongoIPTVPlayer.ini.
    let sources = Sources::load(&ini_path());
    let mut storage = storage;
    let (opened, creds_from_cli) =
        match (resolve_creds(cli.xtream.as_deref(), None), sources.active()) {
            (resolved @ CredsSource::Configured(..), _) => {
                let opened = open_xtream(resolved);
                // The settings tell the user that what they save there does not
                // count while this --xtream is given.
                tracing::info!("portal: from --xtream, which beats {APP_ID}.ini on every start");
                // Its favorites and presets are source 1's, as in 0.6.1.
                storage.set_source(1);
                (opened, true)
            }
            (_, Some(source)) => {
                storage.set_source(source.id);
                (open_source(source, &storage), false)
            }
            (resolved, None) => (open_xtream(resolved), false),
        };
    let OpenedSource {
        portal,
        creds,
        state: portal_state,
    } = opened;
    // The thuisbioscoop's titles of the last start: read after the first frame.
    let vod_cache = bongo_player::vod::cache::path_for(&storage.cache_dir(), creds.as_ref());
    let catalog = Arc::new(CatalogStore::new(portal).with_vod_cache(vod_cache));

    // Local relay for live HLS (see relay/mod.rs). Loopback only; if it can't
    // bind, live plays straight from the portal as before.
    let relay = match Relay::start(RelayConfig::default()) {
        Ok(relay) => Some(relay),
        Err(e) => {
            tracing::warn!("relay: could not start ({}) - live plays directly", e);
            None
        }
    };

    let player_handle = player::spawn(1280, 720)?;

    // Read once here; the app writes it only when something changes.
    let settings = Settings::load(&storage.settings_path());

    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_inner_size([1280.0, 720.0])
        .with_min_inner_size([640.0, 360.0])
        .with_title(format!("{} v{}", APP_NAME, env!("CARGO_PKG_VERSION")));

    let result = eframe::run_native(
        APP_NAME,
        options,
        Box::new(move |cc| {
            Box::new(TvApp::new(
                cc,
                player_handle,
                catalog,
                storage,
                portal_state,
                cli.url,
                relay,
                sources,
                creds,
                creds_from_cli,
                settings,
            ))
        }),
    );
    if let Err(e) = result {
        tracing::error!("eframe exited with error: {}", e);
        return Err(anyhow::anyhow!("eframe: {}", e));
    }
    tracing::info!("{} exited cleanly", APP_NAME);
    Ok(())
}
