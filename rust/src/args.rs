use clap::Parser;
use thiserror::Error;

#[derive(Parser, Debug)]
#[command(
    name = "BongoPlayer",
    display_name = "Bongo Player",
    version,
    about = "Lightweight IPTV player"
)]
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
    /// to the file under %APPDATA%\BongoPlayer\config\log\.
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

/// Path to `BongoPlayer.ini` next to the executable (falls back to CWD if the
/// exe path can't be resolved). This is the app's "root folder" config file.
pub fn ini_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("BONGO_INI") {
        return std::path::PathBuf::from(p);
    }
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        .unwrap_or_else(|| std::path::PathBuf::from("."))
        .join(format!("{}.ini", crate::APP_ID))
}

/// Read a creds string (`user:pass@host:port`) from the ini, if present.
/// Accepts a bare line or a `key=value` form (a key holds no `@` or `:`, so a
/// bare line may hold `=` in its password); `#`/`;` lines are comments and a
/// leading BOM is ignored. Returns the first line that looks like credentials
/// (contains `@`).
pub fn read_ini_creds(path: &std::path::Path) -> Option<String> {
    let txt = std::fs::read_to_string(path).ok()?;
    // Notepad's "UTF-8 with BOM" puts U+FEFF before the first line.
    let txt = txt.strip_prefix('\u{feff}').unwrap_or(&txt);
    for line in txt.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        // `key=value` only when the part before '=' looks like a key: a bare
        // credentials line may hold '=' inside its password.
        let val = match line.split_once('=') {
            Some((k, v)) if !k.contains(['@', ':']) => v.trim(),
            _ => line,
        };
        if val.contains('@') {
            return Some(val.to_string());
        }
    }
    None
}

pub fn parse_xtream_creds(s: &str) -> Result<XtreamCreds, CredsError> {
    // The LAST '@' separates the server: a password may contain '@', a host
    // never does.
    let (userpass, hostport) = s.rsplit_once('@').ok_or(CredsError::NoAt)?;
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

/// Whether these are the stand-in values the zip's `run.bat` ships with (or no
/// host at all) rather than a real account.
pub fn is_placeholder_creds(creds: &XtreamCreds) -> bool {
    let host = creds.host.to_lowercase();
    host == "host.example.com"
        || host == "example.com"
        || host.is_empty()
        || (creds.username == "user" && creds.password == "pass")
}

/// Where the portal credentials come from at startup.
#[derive(Debug, PartialEq, Eq)]
pub enum CredsSource {
    /// Usable credentials: `--xtream` if it has real values, else the ini.
    Configured(XtreamCreds),
    /// Only placeholder values (the zip's run.bat): show the setup prompt.
    Placeholder,
    /// None at all: show the setup prompt.
    Missing,
    /// Unreadable (e.g. a hand-edited ini): show the setup prompt.
    Invalid(String),
}

/// `--xtream` with real values wins, then the ini. Placeholder or unreadable
/// values never stop the app: it shows the setup prompt instead.
pub fn resolve_creds(cli: Option<&str>, ini: Option<&str>) -> CredsSource {
    let mut placeholder = false;
    let mut invalid = None;
    for s in [cli, ini].into_iter().flatten() {
        match parse_xtream_creds(s) {
            Ok(c) if !is_placeholder_creds(&c) => return CredsSource::Configured(c),
            Ok(_) => placeholder = true,
            Err(e) => invalid = invalid.or(Some(e.to_string())),
        }
    }
    match invalid {
        Some(e) => CredsSource::Invalid(e),
        None if placeholder => CredsSource::Placeholder,
        None => CredsSource::Missing,
    }
}
