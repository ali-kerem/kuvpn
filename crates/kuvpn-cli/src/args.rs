use clap::{Parser, ValueEnum};

#[derive(Debug, ValueEnum, Clone)]
pub enum LogLevel {
    /// No logs
    Off,
    /// Informational messages
    Info,
    /// Warning messages
    Warn,
    /// Debugging messages
    Debug,
    /// Error messages
    Error,
    /// Detailed stacktrace messages
    Trace,
}

impl From<LogLevel> for log::LevelFilter {
    fn from(level: LogLevel) -> Self {
        match level {
            LogLevel::Off => log::LevelFilter::Off,
            LogLevel::Info => log::LevelFilter::Info,
            LogLevel::Warn => log::LevelFilter::Warn,
            LogLevel::Debug => log::LevelFilter::Debug,
            LogLevel::Error => log::LevelFilter::Error,
            LogLevel::Trace => log::LevelFilter::Trace,
        }
    }
}

/// How much the login flow is automated (mirrors the GUI setting).
#[derive(Debug, ValueEnum, Clone, Default)]
pub enum LoginMode {
    /// Headless browser, fully automated — best for everyday use (default)
    #[default]
    FullAuto,
    /// Visible browser window, automation still runs — useful for debugging
    Visual,
    /// Visible browser, no automation — complete the login yourself; KUVPN
    /// waits for the DSID cookie then starts OpenConnect
    Manual,
}

impl LoginMode {
    /// Whether the browser should run headlessly.
    pub fn headless(&self) -> bool {
        matches!(self, LoginMode::FullAuto)
    }

    /// Whether automatic login handlers are disabled.
    pub fn no_auto_login(&self) -> bool {
        matches!(self, LoginMode::Manual)
    }
}

/// KUVPN CLI — automated VPN client for Koç University
#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
pub struct Args {
    /// Login mode: full-auto (default), visual, or manual
    #[arg(short, long, value_enum, default_value_t = LoginMode::FullAuto)]
    pub mode: LoginMode,

    /// The URL to the page where we will start logging in and looking for DSID
    #[arg(long, default_value = "https://vpn.ku.edu.tr")]
    pub url: String,

    /// The level of logging
    #[arg(short, long, value_enum, default_value_t = LogLevel::Error)]
    pub log: LogLevel,

    /// The Domain of the DSID found
    #[arg(long, default_value = "vpn.ku.edu.tr")]
    pub domain: String,

    /// Gives the user the dsid without running openconnect
    #[arg(short, long, default_value_t = false)]
    pub dsid: bool,

    /// Delete session information
    #[arg(short, long, default_value_t = false)]
    pub clean: bool,

    /// Command to run openconnect with (e.g., sudo, pkexec, or a custom script)
    #[arg(long)]
    pub run_command: Option<String>,

    /// Path or command name for openconnect. Defaults to 'openconnect'.
    /// Can be a relative or absolute path.
    #[arg(long, default_value = "openconnect")]
    pub openconnect_path: String,

    /// Email for login (optional)
    #[arg(short, long, default_value = None)]
    pub email: Option<String>,

    /// Name for the TUN/TAP interface created by openconnect
    #[arg(long, default_value = "kuvpn0")]
    pub interface_name: String,

    /// Print connection history and exit
    #[arg(long, default_value_t = false)]
    pub history: bool,

    /// Tunnel mode: how traffic is routed through the VPN.
    /// full: all traffic is routed through the VPN (default).
    /// split: only --split-host uses the VPN (macOS/Linux); other traffic and DNS stay local.
    /// manual: pass your own vpnc-script via --vpnc-script.
    #[arg(long, value_enum, default_value_t = CliTunnelMode::Full)]
    pub tunnel_mode: CliTunnelMode,

    /// IPv4 host routed through the VPN; required in split mode (macOS/Linux only).
    #[arg(long, required_if_eq("tunnel_mode", "split"))]
    pub split_host: Option<std::net::Ipv4Addr>,

    /// Path to a custom vpnc-script passed to openconnect via --script.
    /// Only used when --tunnel-mode manual is set.
    #[arg(long)]
    pub vpnc_script: Option<String>,
}

/// Tunnel mode choices for the CLI (mirrors `kuvpn::TunnelMode`).
#[derive(Debug, Clone, ValueEnum, Default)]
pub enum CliTunnelMode {
    /// All traffic is routed through the VPN.
    #[default]
    Full,
    /// Only --split-host uses the VPN; preserve local internet and DNS (macOS/Linux).
    Split,
    /// Use a custom vpnc-script (supply path via --vpnc-script).
    Manual,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_mode_requires_an_explicit_host() {
        let error = Args::try_parse_from(["kuvpn", "--tunnel-mode", "split"]).unwrap_err();
        assert_eq!(
            error.kind(),
            clap::error::ErrorKind::MissingRequiredArgument
        );
        assert!(error.to_string().contains("--split-host"));
        for flags in [vec!["kuvpn"], vec!["kuvpn", "--tunnel-mode", "manual"]] {
            let args = Args::try_parse_from(flags).unwrap();
            assert!(args.split_host.is_none());
        }
        let args = Args::try_parse_from(["kuvpn"]).unwrap();
        assert!(matches!(args.tunnel_mode, CliTunnelMode::Full));
    }

    #[test]
    fn split_host_accepts_only_ipv4_addresses() {
        let args = Args::try_parse_from([
            "kuvpn",
            "--tunnel-mode",
            "split",
            "--split-host",
            "203.0.113.10",
        ])
        .unwrap();
        assert!(matches!(args.tunnel_mode, CliTunnelMode::Split));
        assert_eq!(args.split_host.unwrap().to_string(), "203.0.113.10");
        for invalid in ["", "example.com", "::1", "203.0.113.0/24", "1.2.3.4;id"] {
            assert!(Args::try_parse_from([
                "kuvpn",
                "--tunnel-mode",
                "split",
                "--split-host",
                invalid,
            ])
            .is_err());
        }
    }
}
