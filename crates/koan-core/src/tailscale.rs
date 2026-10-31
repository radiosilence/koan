//! Tailscale on this machine, through its CLI: whether it is running, the
//! machine's name on the tailnet, and Funnel, which publishes a local port on
//! the internet at that name with a certificate Tailscale provides. What a
//! device sharing its library uses to be reached away from home.

use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The ports Funnel publishes on, the one a URL needs no port for first.
const FUNNEL_PORTS: [u16; 3] = [443, 8443, 10000];

/// How long a Funnel change may take. One the tailnet has not allowed asks
/// for approval in a browser and waits; this stops waiting.
const COMMAND_TIMEOUT: Duration = Duration::from_secs(20);

/// Where the tailnet's admin turns Funnel on.
const FUNNEL_ADMIN: &str = "https://login.tailscale.com/admin/acls/file";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tailscale {
    /// The machine's name on the tailnet, `mac.tailnet.ts.net`.
    pub name: String,
    /// The tailnet lets this machine use Funnel and HTTPS certificates.
    pub funnel_allowed: bool,
}

impl Tailscale {
    /// The address Funnel publishes at on `port`.
    pub fn url_on(&self, port: u16) -> String {
        match port {
            443 => format!("https://{}", self.name),
            port => format!("https://{}:{port}", self.name),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FunnelError {
    #[error("Tailscale is not running on this machine")]
    NotRunning,
    #[error("Funnel is not turned on for this tailnet. An admin turns it on at {FUNNEL_ADMIN}")]
    NotAllowed,
    #[error("Tailscale already serves something else on each of Funnel's ports (443, 8443, 10000)")]
    NoFreePort,
    #[error("Tailscale did not finish: {0}")]
    Failed(String),
}

/// The CLI: inside the Mac app, where the App Store and standalone builds
/// both keep it, else on `PATH`.
fn cli() -> Option<PathBuf> {
    let bundled = PathBuf::from("/Applications/Tailscale.app/Contents/MacOS/Tailscale");
    if cfg!(target_os = "macos") && bundled.exists() {
        return Some(bundled);
    }
    [
        "/usr/local/bin/tailscale",
        "/opt/homebrew/bin/tailscale",
        "/usr/bin/tailscale",
    ]
    .into_iter()
    .map(PathBuf::from)
    .find(|p| p.exists())
}

/// Run the CLI with `args`, giving up after `COMMAND_TIMEOUT`. Its output,
/// stdout then stderr, or why it failed.
fn run(args: &[&str]) -> Result<String, FunnelError> {
    let cli = cli().ok_or(FunnelError::NotRunning)?;
    let mut child = Command::new(cli)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| FunnelError::Failed(e.to_string()))?;
    let deadline = Instant::now() + COMMAND_TIMEOUT;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(50)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(FunnelError::Failed("it did not answer in time".into()));
            }
            Err(e) => return Err(FunnelError::Failed(e.to_string())),
        }
    };
    let mut out = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut out);
    }
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut out);
    }
    if status.success() {
        Ok(out)
    } else {
        Err(FunnelError::Failed(out.trim().to_owned()))
    }
}

/// Tailscale on this machine, while it is running and signed in.
pub fn status() -> Option<Tailscale> {
    let out = run(&["status", "--json", "--peers=false"]).ok()?;
    let json: serde_json::Value = serde_json::from_str(&out).ok()?;
    if json["BackendState"] != "Running" {
        return None;
    }
    let me = &json["Self"];
    let name = me["DNSName"].as_str()?.trim_end_matches('.').to_owned();
    if name.is_empty() {
        return None;
    }
    let caps = me["CapMap"].as_object();
    let has = |cap: &str| caps.is_some_and(|c| c.contains_key(cap));
    Some(Tailscale {
        name,
        funnel_allowed: has("funnel") && has("https"),
    })
}

/// What Tailscale serves on each port, as `funnel status --json` reports it:
/// the port, whether Funnel opens it to the internet, and where its root
/// proxies to.
struct Served {
    port: u16,
    public: bool,
    root: Option<String>,
}

/// An error when Tailscale could not say, which is not the same as serving
/// nothing.
fn served() -> Result<Vec<Served>, FunnelError> {
    let out = run(&["funnel", "status", "--json"])?;
    let json = serde_json::from_str::<serde_json::Value>(&out)
        .map_err(|e| FunnelError::Failed(format!("unreadable status: {e}")))?;
    Ok(parse_served(&json))
}

fn parse_served(json: &serde_json::Value) -> Vec<Served> {
    let port_of = |key: &str| {
        key.rsplit_once(':')
            .and_then(|(_, p)| p.parse::<u16>().ok())
    };
    let public = |port: u16| {
        json["AllowFunnel"].as_object().is_some_and(|hosts| {
            hosts
                .iter()
                .any(|(k, on)| port_of(k) == Some(port) && on == true)
        })
    };
    let mut ports: Vec<u16> = json["TCP"]
        .as_object()
        .map(|tcp| tcp.keys().filter_map(|k| k.parse().ok()).collect())
        .unwrap_or_default();
    ports.extend(
        json["Web"]
            .as_object()
            .into_iter()
            .flat_map(|web| web.keys().filter_map(|k| port_of(k))),
    );
    ports.sort_unstable();
    ports.dedup();
    ports
        .into_iter()
        .map(|port| Served {
            port,
            public: public(port),
            root: json["Web"].as_object().and_then(|web| {
                web.iter()
                    .find(|(k, _)| port_of(k) == Some(port))
                    .and_then(|(_, host)| {
                        host["Handlers"]["/"]["Proxy"].as_str().map(str::to_owned)
                    })
            }),
        })
        .collect()
}

/// Whether Funnel's `funnel_port` publishes local `port` to the internet.
fn ours(served: &[Served], funnel_port: u16, port: u16) -> bool {
    served
        .iter()
        .any(|s| s.port == funnel_port && s.public && proxies_to(s.root.as_deref(), port))
}

/// Whether a proxy target is local `port`.
fn proxies_to(root: Option<&str>, port: u16) -> bool {
    root.is_some_and(|r| {
        r.trim_end_matches('/')
            .ends_with(&format!("127.0.0.1:{port}"))
    })
}

/// The Funnel port publishing local `port` to the internet, if one does.
pub fn funnel_port(port: u16) -> Option<u16> {
    served()
        .ok()?
        .into_iter()
        .find(|s| s.public && proxies_to(s.root.as_deref(), port))
        .map(|s| s.port)
}

/// Publish local `port` on the internet at this machine's tailnet name, over
/// HTTPS. Kept where it already is; otherwise on the first of Funnel's ports
/// nothing is served on, so whatever else this machine publishes stays.
/// Returns the address and Funnel's port.
pub fn funnel(port: u16) -> Result<(String, u16), FunnelError> {
    let tailscale = status().ok_or(FunnelError::NotRunning)?;
    if !tailscale.funnel_allowed {
        return Err(FunnelError::NotAllowed);
    }
    let served = served()?;
    if let Some(on) = served
        .iter()
        .find(|s| s.public && proxies_to(s.root.as_deref(), port))
    {
        return Ok((tailscale.url_on(on.port), on.port));
    }
    let free = FUNNEL_PORTS
        .into_iter()
        .find(|p| !served.iter().any(|s| s.port == *p))
        .ok_or(FunnelError::NoFreePort)?;
    run(&[
        "funnel",
        "--bg",
        "--yes",
        &format!("--https={free}"),
        &format!("http://127.0.0.1:{port}"),
    ])?;
    Ok((tailscale.url_on(free), free))
}

/// Take down the Funnel on `funnel_port` proxying to local `port`: the one
/// kōan published. Anything else Tailscale serves stays, on that port or any
/// other. Refused when Tailscale is installed but cannot say what it
/// serves, since a Funnel kept in a stopped Tailscale's preferences comes
/// back with it; nothing to do when it is not installed at all.
pub fn funnel_off(funnel_port: u16, port: u16) -> Result<(), FunnelError> {
    if cli().is_none() {
        return Ok(());
    }
    if !ours(&served()?, funnel_port, port) {
        return Ok(());
    }
    match run(&["funnel", &format!("--https={funnel_port}"), "off"]) {
        Ok(_) => Ok(()),
        // Gone already.
        Err(FunnelError::Failed(why)) if why.contains("does not exist") => Ok(()),
        Err(e) => Err(e),
    }
}

/// Start taking down the Funnel on `funnel_port` without waiting: for an app
/// quitting, which a hung Tailscale must not hold up.
pub fn funnel_off_detached(funnel_port: u16) {
    if let Some(cli) = cli() {
        let _ = Command::new(cli)
            .args(["funnel", &format!("--https={funnel_port}"), "off"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `tailscale funnel status --json` with kōan on Funnel's 443 and the
    /// person's own tailnet-only serve of the same port on 8443.
    const STATUS: &str = r#"{
        "TCP": {"443": {"HTTPS": true}, "8443": {"HTTPS": true}},
        "Web": {
            "mac.tailnet.ts.net:443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4000"}}},
            "mac.tailnet.ts.net:8443": {"Handlers": {"/": {"Proxy": "http://127.0.0.1:4000"}}}
        },
        "AllowFunnel": {"mac.tailnet.ts.net:443": true}
    }"#;

    #[test]
    fn only_the_funnel_published_is_taken_down() {
        let served = parse_served(&serde_json::from_str(STATUS).unwrap());
        assert_eq!(served.len(), 2);
        assert!(ours(&served, 443, 4000));
        // Served to the tailnet only, by hand: not Funnel's.
        assert!(!ours(&served, 8443, 4000));
        // Another local port's.
        assert!(!ours(&served, 443, 4001));
    }

    #[test]
    fn nothing_served_is_nothing_published() {
        let served = parse_served(&serde_json::from_str("{}").unwrap());
        assert!(served.is_empty());
        assert!(!ours(&served, 443, 4000));
    }
}
