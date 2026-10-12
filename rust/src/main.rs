// No console window by default - clean for end users. Pass --console for a
// live debug console; logs always also go to the file under %APPDATA%.
#![cfg_attr(windows, windows_subsystem = "windows")]

use bongo_player::app::{PortalState, TvApp};
use bongo_player::args::{
    ini_path, read_ini_creds, resolve_creds, Cli, CredsOrigin, CredsSource, XtreamCreds,
};
use bongo_player::catalog::CatalogStore;
use bongo_player::legacy;
use bongo_player::player;
use bongo_player::portal::{xtream::XtreamPortal, Portal};
use bongo_player::relay::{Relay, RelayConfig};
use bongo_player::settings::Settings;
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
/// %APPDATA%\BongoPlayer\config\log\. Returns the log directory path so callers
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
    // logs (the file log under %APPDATA%\BongoPlayer\config\log\ is always
    // written).
    #[cfg(windows)]
    if cli.console {
        win_console::alloc();
    }

    let storage = Storage::standard()?;
    // First start after the tvplayer -> Bongo Player rename: carry over the old
    // data and ini. Must run before ensure_config_dir (an existing config dir
    // means "already migrated"); results are logged once logging is up.
    // Under the test override the old tvplayer data must not be copied in.
    let isolated = std::env::var_os("BONGO_CONFIG_DIR").is_some();
    let migrated_config = legacy::legacy_config_dir()
        .filter(|_| !isolated)
        .map(|old| legacy::migrate_config_dir(&old, storage.config_dir()));
    let migrated_ini = if isolated {
        Ok(false)
    } else {
        legacy::migrate_file(&legacy::legacy_ini_path(), &ini_path())
    };
    storage.ensure_config_dir()?;
    let log_dir = init_logging(&storage);
    install_panic_hook(log_dir.clone());

    tracing::info!("{} v{} starting", APP_NAME, env!("CARGO_PKG_VERSION"));
    tracing::info!("config dir: {}", storage.config_dir().display());
    tracing::info!("log dir:    {}", log_dir.display());
    match migrated_config {
        Some(Ok(n)) if n > 0 => tracing::info!("migrated {} files from the tvplayer config dir", n),
        Some(Err(e)) => tracing::warn!("could not migrate the tvplayer config dir: {}", e),
        _ => {}
    }
    match migrated_ini {
        Ok(true) => tracing::info!("renamed tvplayer.ini to {}", ini_path().display()),
        Ok(false) => {}
        Err(e) => tracing::warn!("could not rename tvplayer.ini: {}", e),
    }

    let rt = Runtime::new()?;
    let _rt_guard = rt.enter();

    // Resolve portal (see args::resolve_creds): --xtream with real values wins,
    // then BongoPlayer.ini in the app folder. Placeholder, missing or unreadable
    // credentials never stop the app: it shows the setup prompt, which writes
    // BongoPlayer.ini.
    let ini_creds = read_ini_creds(&ini_path());
    let anon = || -> Arc<dyn Portal> {
        Arc::new(XtreamPortal::new(XtreamCreds {
            username: "anon".into(),
            password: "anon".into(),
            host: "127.0.0.1".into(),
            port: 1,
        }))
    };
    let resolved = resolve_creds(cli.xtream.as_deref(), ini_creds.as_deref());
    // The settings tell the user that what they save there does not count
    // while this --xtream is given.
    let creds_from_cli = matches!(
        resolved,
        CredsSource::Configured(_, CredsOrigin::CommandLine)
    );
    let (portal, portal_state, creds): (Arc<dyn Portal>, PortalState, Option<XtreamCreds>) =
        match resolved {
            CredsSource::Configured(c, _) => {
                tracing::info!("portal: xtream <user>@{}:{}", c.host, c.port);
                if creds_from_cli {
                    tracing::info!(
                        "portal: from --xtream, which beats {APP_ID}.ini on every start"
                    );
                }
                (Arc::new(XtreamPortal::new(c.clone())), PortalState::Configured, Some(c))
            }
            CredsSource::Placeholder => {
                tracing::warn!("portal: placeholder credentials - showing the setup prompt");
                (anon(), PortalState::Placeholder, None)
            }
            CredsSource::Missing => {
                tracing::warn!("portal: no credentials (--xtream or {APP_ID}.ini) - showing the setup prompt");
                (anon(), PortalState::Missing, None)
            }
            CredsSource::Invalid(e) => {
                tracing::warn!("portal: unreadable credentials ({e}) - showing the setup prompt");
                (anon(), PortalState::Missing, None)
            }
        };
    let catalog = Arc::new(CatalogStore::new(portal));

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
