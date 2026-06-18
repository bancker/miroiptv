use clap::Parser;
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(name = "tvplayer", version, about = "Lightweight IPTV player")]
pub struct Cli {
    /// Xtream Codes portal: user:pass@host[:port]
    #[arg(long)]
    pub xtream: Option<String>,

    /// Direct stream URL (HLS/HTTP/etc.) - bypasses portal
    pub url: Option<String>,

    /// Run smoke selftest and exit
    #[arg(long, hide = true)]
    pub selftest: bool,

    /// Show a live debug console window (off by default). Logs always also go
    /// to the file under %APPDATA%\tvplayer\log\.
    #[arg(long)]
    pub console: bool,
}

#[derive(Debug, PartialEq, Eq, Clone)]
pub struct XtreamCreds {
    pub username: String,
    pub password: String,
    pub host: String,
    pub port: u16,
}

#[derive(Debug, Error)]
pub enum CredsError {
    #[error("missing '@' separator in creds")]
    NoAt,
    #[error("missing ':' separator in user:pass")]
    NoUserPass,
    #[error("invalid port number")]
    BadPort,
}

/// Path to `tvplayer.ini` next to the executable (falls back to CWD if the
/// exe path can't be resolved). This is the app's "root folder" config file.
pub fn ini_path() -> std::path::PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join("tvplayer.ini")
}

/// Read a creds string (`user:pass@host:port`) from tvplayer.ini, if present.
/// Accepts a bare line or a `key=value` form (any key); `#`/`;` lines are
/// comments. Returns the first line that looks like credentials (contains `@`).
pub fn read_ini_creds(path: &std::path::Path) -> Option<String> {
    let txt = std::fs::read_to_string(path).ok()?;
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        let val = line.split_once('=').map(|(_, v)| v.trim()).unwrap_or(line);
        if val.contains('@') {
            return Some(val.to_string());
        }
    }
    None
}

pub fn parse_xtream_creds(s: &str) -> Result<XtreamCreds, CredsError> {
    let (userpass, hostport) = s.split_once('@').ok_or(CredsError::NoAt)?;
    let (user, pass) = userpass.split_once(':').ok_or(CredsError::NoUserPass)?;
    let (host, port) = match hostport.split_once(':') {
        Some((h, p)) => (h, p.parse::<u16>().map_err(|_| CredsError::BadPort)?),
        None => (hostport, 80),
    };
    Ok(XtreamCreds {
        username: user.to_owned(),
        password: pass.to_owned(),
        host: host.to_owned(),
        port,
    })
}
