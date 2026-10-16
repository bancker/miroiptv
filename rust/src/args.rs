use clap::Parser;
use thiserror::Error;

/// The command line. `Debug` for the tests only: `--xtream` holds the password.
#[derive(Parser)]
#[cfg_attr(test, derive(Debug))]
#[command(
    name = "BongoIPTVPlayer",
    display_name = "Bongo IPTV Player",
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
    /// to the file under %APPDATA%\BongoIPTVPlayer\config\log\.
    #[arg(long)]
    pub console: bool,
}

/// `Debug` for the tests only: it would print the password.
#[derive(PartialEq, Eq, Clone)]
#[cfg_attr(test, derive(Debug))]
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

/// Path to `BongoIPTVPlayer.ini` next to the executable (falls back to CWD if the
/// exe path can't be resolved). This is the app's "root folder" config file.
///
/// `BONGO_INI` points it elsewhere. So does `BONGO_CONFIG_DIR` (the test
/// override, see `Storage::standard`) when `BONGO_INI` is not set: the ini is
/// then `BongoIPTVPlayer.ini` in that folder, never the file next to the exe, which
/// may hold the developer's real portal and which a Save in the settings
/// overwrites.
pub fn ini_path() -> std::path::PathBuf {
    if let Some(p) = std::env::var_os("BONGO_INI") {
        return std::path::PathBuf::from(p);
    }
    if let Some(dir) = std::env::var_os("BONGO_CONFIG_DIR") {
        return std::path::PathBuf::from(dir).join(format!("{}.ini", crate::APP_ID));
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

/// Save credentials as the ini's only line, `portal=user:pass@host:port`; the
/// key keeps an `=` in the user name from being read as one. Written to a temp
/// file beside the ini, then renamed over it, so a failed write leaves the old
/// ini as it was.
pub fn write_ini_creds(path: &std::path::Path, creds: &str) -> std::io::Result<()> {
    let tmp = path.with_extension("ini.tmp");
    let saved = std::fs::write(&tmp, format!("portal={creds}\n"))
        .and_then(|()| std::fs::rename(&tmp, path));
    if saved.is_err() {
        // The temp file holds the password: do not leave it behind.
        let _ = std::fs::remove_file(&tmp);
    }
    saved
}

/// `text` without CR and LF. egui keeps the ones that come with a paste in a
/// single-line field, where they would end up inside a user name, a password or
/// a server.
pub fn without_newlines(text: &str) -> String {
    text.chars().filter(|c| !matches!(c, '\r' | '\n')).collect()
}

/// The `user:pass@server` line that the settings and the setup prompt hand to
/// the connection test and the ini, made of what was typed or pasted. CR and LF
/// go from all three. The user name and the server lose the spaces around them,
/// and the server a leading `http://` or `https://` and its trailing `/` (a
/// pasted address). The password stays as typed, spaces included.
pub fn credentials_line(user: &str, password: &str, server: &str) -> String {
    let (user, server) = (without_newlines(user), without_newlines(server));
    let server = server.trim();
    let server = ["https://", "http://"]
        .iter()
        .find_map(|scheme| {
            let head = server.get(..scheme.len())?;
            head.eq_ignore_ascii_case(scheme)
                .then(|| &server[scheme.len()..])
        })
        .unwrap_or(server);
    let server = server.trim_end_matches('/');
    format!("{}:{}@{}", user.trim(), without_newlines(password), server)
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

/// Which of the two places usable credentials were read from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CredsOrigin {
    /// `--xtream`, which wins over the ini on every start.
    CommandLine,
    /// `BongoIPTVPlayer.ini`.
    Ini,
}

/// Where the portal credentials come from at startup. `Debug` for the tests
/// only, as for [`XtreamCreds`], which it holds.
#[derive(PartialEq, Eq)]
#[cfg_attr(test, derive(Debug))]
pub enum CredsSource {
    /// Usable credentials, and which place they came from: `--xtream` if it has
    /// real values, else the ini.
    Configured(XtreamCreds, CredsOrigin),
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
    let places = [(cli, CredsOrigin::CommandLine), (ini, CredsOrigin::Ini)];
    for (s, origin) in places.into_iter().filter_map(|(s, o)| Some((s?, o))) {
        match parse_xtream_creds(s) {
            Ok(c) if !is_placeholder_creds(&c) => return CredsSource::Configured(c, origin),
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
