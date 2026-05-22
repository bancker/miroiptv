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
