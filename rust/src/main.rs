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

// Detach from the console window so `--no-console` end-user launches don't
// get a separate debug console. kernel32; only declared on Windows.
#[cfg(windows)]
#[link(name = "kernel32")]
extern "system" {
    fn FreeConsole() -> std::os::raw::c_int;
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

    // Console-subsystem binary: developers get a live log console by default.
    // End-user launches pass --no-console to suppress it (file logging under
    // %APPDATA%\tvplayer\log\ is unaffected).
    #[cfg(windows)]
    if cli.no_console {
        unsafe { FreeConsole() };
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
        .with_title("tvplayer");

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
