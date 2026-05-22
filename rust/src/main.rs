use clap::Parser;
use std::sync::Arc;
use tokio::runtime::Runtime;
use tracing_subscriber::EnvFilter;
use tvplayer::app::TvApp;
use tvplayer::args::{parse_xtream_creds, Cli, XtreamCreds};
use tvplayer::catalog::CatalogStore;
use tvplayer::player;
use tvplayer::portal::{xtream::XtreamPortal, Portal};
use tvplayer::storage::Storage;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("tvplayer=info,reqwest=warn,hyper=warn")),
        )
        .with_target(false)
        .init();

    let cli = Cli::parse();

    if cli.selftest {
        println!("selftest: args parsed OK");
        return Ok(());
    }

    let rt = Runtime::new()?;
    let _rt_guard = rt.enter();

    let storage = Storage::standard()?;
    storage.ensure_config_dir()?;

    let portal: Arc<dyn Portal> = if let Some(s) = cli.xtream.as_deref() {
        let creds =
            parse_xtream_creds(s).map_err(|e| anyhow::anyhow!("--xtream parse: {}", e))?;
        Arc::new(XtreamPortal::new(creds))
    } else {
        Arc::new(XtreamPortal::new(XtreamCreds {
            username: "anon".into(),
            password: "anon".into(),
            host: "127.0.0.1".into(),
            port: 1,
        }))
    };
    let catalog = Arc::new(CatalogStore::new(portal));

    let player_handle = player::spawn(1280, 720)?;

    if let Some(u) = cli.url.clone() {
        let _ = player_handle.cmd_tx.send(player::Cmd::LoadUrl(u));
    }

    let mut options = eframe::NativeOptions::default();
    options.viewport = options
        .viewport
        .with_inner_size([1280.0, 720.0])
        .with_min_inner_size([640.0, 360.0])
        .with_title("tvplayer");

    eframe::run_native(
        "tvplayer",
        options,
        Box::new(move |cc| Box::new(TvApp::new(cc, player_handle, catalog, storage))),
    )
    .map_err(|e| anyhow::anyhow!("eframe: {}", e))?;
    Ok(())
}
