// No console window by default - clean for end users. Pass --console for a
// live debug console; logs always also go to the file under %APPDATA%.
#![cfg_attr(windows, windows_subsystem = "windows")]

use clap::Parser;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing_subscriber::{fmt, prelude::*, EnvFilter};
use tvplayer::app::{PortalState, TvApp};
use tvplayer::args::{ini_path, parse_xtream_creds, read_ini_creds, Cli, XtreamCreds};
use tvplayer::catalog::CatalogStore;
use tvplayer::player;
use tvplayer::portal::{xtream::XtreamPortal, Portal};
use tvplayer::storage::Storage;

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

fn is_placeholder_creds(creds: &XtreamCreds) -> bool {
    let host = creds.host.to_lowercase();
    host == "host.example.com"
        || host == "example.com"
        || host.is_empty()
        || (creds.username == "user" && creds.password == "pass")
}

/// Initialize tracing: stderr + rolling daily file under %APPDATA%\tvplayer\log\.
/// Returns the log directory path so callers can show it in the UI / panic hook.
fn init_logging(storage: &Storage) -> std::path::PathBuf {
    let log_dir = storage.config_dir().join("log");
    std::fs::create_dir_all(&log_dir).ok();

    // Sync rolling appender: writes are line-flushed so a panic-abort still
    // leaves the last few seconds of context on disk.
    let file_appender = tracing_appender::rolling::daily(&log_dir, "tvplayer.log");

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("tvplayer=debug,reqwest=warn,hyper=warn"));

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
    // logs (the file log under %APPDATA%\tvplayer\log\ is always written).
    #[cfg(windows)]
    if cli.console {
        win_console::alloc();
    }

    let storage = Storage::standard()?;
    storage.ensure_config_dir()?;
    let log_dir = init_logging(&storage);
    install_panic_hook(log_dir.clone());

    tracing::info!("tvplayer v{} starting", env!("CARGO_PKG_VERSION"));
    tracing::info!("config dir: {}", storage.config_dir().display());
    tracing::info!("log dir:    {}", log_dir.display());

    let rt = Runtime::new()?;
    let _rt_guard = rt.enter();

    // Resolve portal:
    //   1. --xtream provided AND not placeholder -> real portal
    //   2. otherwise -> "no portal" sentinel; UI shows configure-portal screen
    // Credentials come from --xtream (explicit) or, failing that, tvplayer.ini
    // in the app folder. If neither exists the app shows a first-run prompt
    // that writes tvplayer.ini.
    let creds_str = cli
        .xtream
        .clone()
        .or_else(|| read_ini_creds(&ini_path()));
    let (portal, portal_state): (Arc<dyn Portal>, PortalState) = if let Some(s) =
        creds_str.as_deref()
    {
        match parse_xtream_creds(s) {
            Ok(creds) if !is_placeholder_creds(&creds) => {
                tracing::info!(
                    "portal: xtream {}@{}:{}",
                    creds.username,
                    creds.host,
                    creds.port
                );
                (Arc::new(XtreamPortal::new(creds)), PortalState::Configured)
            }
            Ok(creds) => {
                tracing::warn!("portal: placeholder credentials detected ({:?}@{}:{}). Edit run.bat or pass --xtream user:pass@host:port.", creds.username, creds.host, creds.port);
                (
                    Arc::new(XtreamPortal::new(XtreamCreds {
                        username: "anon".into(),
                        password: "anon".into(),
                        host: "127.0.0.1".into(),
                        port: 1,
                    })),
                    PortalState::Placeholder,
                )
            }
            Err(e) => {
                tracing::error!("portal: --xtream parse failed: {}", e);
                anyhow::bail!("invalid --xtream credentials: {}", e);
            }
        }
    } else {
        tracing::warn!("portal: no credentials (--xtream or tvplayer.ini). Showing first-run setup prompt.");
        (
            Arc::new(XtreamPortal::new(XtreamCreds {
                username: "anon".into(),
                password: "anon".into(),
                host: "127.0.0.1".into(),
                port: 1,
            })),
            PortalState::Missing,
        )
    };
    let catalog = Arc::new(CatalogStore::new(portal));

    let player_handle = player::spawn(1280, 720)?;

    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_inner_size([1280.0, 720.0])
        .with_min_inner_size([640.0, 360.0])
        .with_title(format!("tvplayer v{}", env!("CARGO_PKG_VERSION")));

    let result = eframe::run_native(
        "tvplayer",
        options,
        Box::new(move |cc| {
            Box::new(TvApp::new(
                cc,
                player_handle,
                catalog,
                storage,
                portal_state,
                cli.url,
            ))
        }),
    );
    if let Err(e) = result {
        tracing::error!("eframe exited with error: {}", e);
        return Err(anyhow::anyhow!("eframe: {}", e));
    }
    tracing::info!("tvplayer exited cleanly");
    Ok(())
}
