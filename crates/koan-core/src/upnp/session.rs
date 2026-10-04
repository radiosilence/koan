//! Driving one renderer: its transport, its volume, and hearing what it does.
//!
//! An event from the renderer is a reason to look, never a reading. Events
//! arrive late and out of order with respect to the commands koan sends, and
//! AVTransport leaves position out of them altogether, so each one is
//! answered by asking the renderer where it is (`GetTransportInfo` and
//! `GetPositionInfo`) and handing the player that answer.
//!
//! Every answer carries the command epoch it was asked under. The epoch moves
//! once a command has been acknowledged, so an answer asked before the
//! renderer had taken the last command is recognisably stale and dropped.
//!
//! A renderer that refuses a subscription, or sends no initial event after
//! one (GENA requires it), is asked once a second instead, and only while it
//! is playing.

use std::net::IpAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
use parking_lot::Mutex;

use super::description::{Renderer, Service};
use super::serve::{Listener, Served};
use super::soap::{self, SoapError, arg};
use super::{didl, xml};

const SUBSCRIPTION_SECONDS: u64 = 1800;
/// How long the initial event may take before the renderer is taken to send
/// none.
const INITIAL_EVENT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_secs(1);
const UNKNOWN_VOLUME: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transport {
    Playing,
    Paused,
    Stopped,
    /// Between states: loading, buffering, seeking.
    Transitioning,
    NoMedia,
}

impl Transport {
    fn parse(s: &str) -> Self {
        match s.trim() {
            "PLAYING" => Self::Playing,
            "PAUSED_PLAYBACK" | "PAUSED_RECORDING" => Self::Paused,
            "TRANSITIONING" => Self::Transitioning,
            "NO_MEDIA_PRESENT" => Self::NoMedia,
            _ => Self::Stopped,
        }
    }
}

/// Where the renderer is, as it answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub epoch: u64,
    pub transport: Transport,
    pub track_uri: String,
    pub position_ms: Option<u64>,
    pub duration_ms: Option<u64>,
    /// When the answer came back, to extrapolate the position from.
    pub at: Instant,
}

#[derive(Debug, Clone)]
pub enum Event {
    Snapshot(Snapshot),
    Volume(u8),
    /// The renderer stopped answering, or said goodbye on the network.
    Gone,
}

struct Subscription {
    service: Service,
    sid: String,
    renew_at: Instant,
}

struct Shared {
    renderer: Renderer,
    http: reqwest::blocking::Client,
    epoch: AtomicU64,
    look: Sender<()>,
    polling: AtomicBool,
    playing: AtomicBool,
    /// Its volume as last read or heard, `UNKNOWN_VOLUME` until then.
    volume: Arc<AtomicU16>,
    subscriptions: Mutex<Vec<Subscription>>,
    trust: Arc<Trust>,
    callback: String,
    closed: AtomicBool,
    unreachable: AtomicBool,
    on_event: Arc<dyn Fn(Event) + Send + Sync>,
}

/// Which `NOTIFY`s to believe. The listener is open to the whole network,
/// so only events carrying a SID this session subscribed under are read, and
/// while a subscription is being made, before its SID is known, only as a
/// reason to look.
#[derive(Default)]
struct Trust {
    sids: Mutex<Vec<String>>,
    subscribing: AtomicBool,
}

pub struct Session {
    shared: Arc<Shared>,
    listener: Listener,
    /// `http://ip:port`, as the renderer reaches the listener.
    base: String,
    sink: Vec<Option<String>>,
    stop: Sender<()>,
    gone: u64,
}

impl Session {
    /// Open a session with `renderer`: start the listener, read what it
    /// plays, and subscribe to its events.
    pub fn open(
        renderer: Renderer,
        on_event: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Self, String> {
        let host: IpAddr = renderer
            .location
            .host_str()
            .and_then(|h| h.trim_matches(['[', ']']).parse().ok())
            .ok_or_else(|| format!("{} has no address", renderer.name))?;
        let ip = super::serve::local_ip_towards(host)
            .ok_or_else(|| format!("no route to {}", renderer.name))?;

        let on_event: Arc<dyn Fn(Event) + Send + Sync> = Arc::new(on_event);
        let (look_tx, look_rx) = crossbeam_channel::bounded::<()>(1);
        let seen = Arc::new(AtomicBool::new(false));
        let trust = Arc::new(Trust::default());
        let volume = Arc::new(AtomicU16::new(UNKNOWN_VOLUME));
        let listener = {
            let look = look_tx.clone();
            let on_event = on_event.clone();
            let seen = seen.clone();
            let trust = trust.clone();
            let heard = volume.clone();
            Listener::start(Box::new(move |sid, body| {
                let known = trust.sids.lock().iter().any(|s| s == sid);
                if !known && !trust.subscribing.load(Ordering::Acquire) {
                    return;
                }
                seen.store(true, Ordering::Release);
                let change = parse_notify(&body);
                if let Some(volume) = change.volume.filter(|_| known) {
                    heard.store(u16::from(volume), Ordering::Release);
                    on_event(Event::Volume(volume));
                }
                if change.transport {
                    let _ = look.try_send(());
                }
            }))
            .map_err(|e| format!("cannot listen for {}: {e}", renderer.name))?
        };
        let base = match ip {
            IpAddr::V6(v6) => format!("http://[{v6}]:{}", listener.port()),
            IpAddr::V4(v4) => format!("http://{v4}:{}", listener.port()),
        };

        let http = soap::client();
        let sink = renderer
            .connection_manager
            .as_ref()
            .and_then(|cm| soap::call(&http, cm, "GetProtocolInfo", &[]).ok())
            .and_then(|args| arg(&args, "Sink").map(didl::sink_mimes))
            .unwrap_or_default();
        log::info!(
            "upnp: {} plays {}",
            renderer.name,
            if sink.is_empty() {
                "anything it is given (no protocol info)".to_string()
            } else {
                sink.iter()
                    .map(|m| m.as_deref().unwrap_or("*"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        );

        let shared = Arc::new(Shared {
            renderer,
            http,
            epoch: AtomicU64::new(0),
            look: look_tx,
            polling: AtomicBool::new(false),
            playing: AtomicBool::new(false),
            volume: volume.clone(),
            subscriptions: Mutex::new(Vec::new()),
            trust,
            callback: format!("<{base}/events/>"),
            closed: AtomicBool::new(false),
            unreachable: AtomicBool::new(false),
            on_event: on_event.clone(),
        });

        let services: Vec<Service> = std::iter::once(shared.renderer.av_transport.clone())
            .chain(shared.renderer.rendering_control.clone())
            .collect();
        shared.trust.subscribing.store(true, Ordering::Release);
        for (i, service) in services.into_iter().enumerate() {
            match shared.subscribe(&service) {
                Ok(sub) => shared.subscriptions.lock().push(sub),
                Err(e) => {
                    log::info!("upnp: {} refused a subscription: {e}", shared.renderer.name);
                    if i == 0 {
                        shared.polling.store(true, Ordering::Release);
                    }
                }
            }
        }
        shared.trust.subscribing.store(false, Ordering::Release);

        // A renderer saying goodbye is gone, whether or not anything was
        // being sent to it.
        let gone = {
            let weak = Arc::downgrade(&shared);
            super::discovery::on_gone(&shared.renderer.udn, move || {
                if let Some(shared) = weak.upgrade() {
                    shared.lost();
                }
            })
        };

        // Kept rather than sent: nothing plays to this renderer yet, and an
        // event now would land on whatever output the player has before it.
        if let Some(v) = shared.volume() {
            volume.store(u16::from(v), Ordering::Release);
        }

        let (stop_tx, stop_rx) = crossbeam_channel::bounded::<()>(1);
        spawn_watcher(shared.clone(), look_rx, seen, on_event, stop_rx.clone());
        spawn_renewer(shared.clone(), stop_rx);

        Ok(Self {
            shared,
            listener,
            base,
            sink,
            stop: stop_tx,
            gone,
        })
    }

    pub fn renderer(&self) -> &Renderer {
        &self.shared.renderer
    }

    /// The MIME type to serve a file with this extension as, or `None` when
    /// the renderer says it cannot play it.
    pub fn mime_for(&self, extension: &str) -> Option<&'static str> {
        didl::choose_mime(extension, &self.sink)
    }

    /// Serve `path` and return its token, its URL and its cover's URL.
    pub fn serve(
        &self,
        path: &std::path::Path,
        mime: &str,
        extension: &str,
    ) -> (String, String, String) {
        let token = self.listener.add(Served {
            path: path.to_path_buf(),
            mime: mime.to_string(),
        });
        let url = format!("{}/t/{token}.{extension}", self.base);
        let art = format!("{}/art/{token}", self.base);
        (token, url, art)
    }

    /// Stop serving every token but these.
    pub fn retain(&self, tokens: &[&str]) {
        self.listener.retain(tokens);
    }

    /// The token in a URL this session handed out.
    pub fn token_of<'a>(&self, uri: &'a str) -> Option<&'a str> {
        let rest = uri.strip_prefix(&self.base)?.strip_prefix("/t/")?;
        rest.split('.').next()
    }

    pub fn epoch(&self) -> u64 {
        self.shared.epoch.load(Ordering::Acquire)
    }

    /// Ask the renderer where it is, and hand the answer to the player.
    pub fn look(&self) {
        let _ = self.shared.look.try_send(());
    }

    pub fn set_uri(&self, uri: &str, metadata: &str) -> Result<(), SoapError> {
        self.transport(
            "SetAVTransportURI",
            &[("CurrentURI", uri), ("CurrentURIMetaData", metadata)],
        )
    }

    pub fn set_next(&self, uri: &str, metadata: &str) -> Result<(), SoapError> {
        self.transport(
            "SetNextAVTransportURI",
            &[("NextURI", uri), ("NextURIMetaData", metadata)],
        )
    }

    pub fn play(&self) -> Result<(), SoapError> {
        self.shared.playing.store(true, Ordering::Release);
        self.transport("Play", &[("Speed", "1")])
    }

    pub fn pause(&self) -> Result<(), SoapError> {
        self.shared.playing.store(false, Ordering::Release);
        self.transport("Pause", &[])
    }

    pub fn stop(&self) -> Result<(), SoapError> {
        self.shared.playing.store(false, Ordering::Release);
        self.transport("Stop", &[])
    }

    pub fn seek(&self, position_ms: u64) -> Result<(), SoapError> {
        self.transport(
            "Seek",
            &[
                ("Unit", "REL_TIME"),
                ("Target", &soap::format_time(position_ms)),
            ],
        )
    }

    /// Its volume, 0–100, as last read or heard; `None` when it has no
    /// volume control or has not said.
    pub fn volume(&self) -> Option<u8> {
        match self.shared.volume.load(Ordering::Acquire) {
            UNKNOWN_VOLUME => None,
            v => Some(v as u8),
        }
    }

    pub fn set_volume(&self, volume: u8) -> Result<(), SoapError> {
        let Some(rc) = &self.shared.renderer.rendering_control else {
            return Ok(());
        };
        let volume = volume.min(100);
        self.shared
            .volume
            .store(u16::from(volume), Ordering::Release);
        soap::call(
            &self.shared.http,
            rc,
            "SetVolume",
            &[
                ("InstanceID", "0"),
                ("Channel", "Master"),
                ("DesiredVolume", &volume.min(100).to_string()),
            ],
        )
        .map(|_| ())
    }

    pub fn has_volume(&self) -> bool {
        self.shared.renderer.rendering_control.is_some()
    }

    /// Run an AVTransport action, and move the epoch once the renderer has
    /// taken it.
    ///
    /// Once the renderer has failed to answer at all, nothing more is sent:
    /// each attempt would hold the player for a timeout, and a skip is three
    /// of them.
    fn transport(&self, action: &str, args: &[(&str, &str)]) -> Result<(), SoapError> {
        if self.shared.unreachable.load(Ordering::Acquire) {
            return Err(SoapError::Unreachable(format!(
                "{} stopped answering",
                self.shared.renderer.name
            )));
        }
        let mut full = vec![("InstanceID", "0")];
        full.extend_from_slice(args);
        let result = soap::call(
            &self.shared.http,
            &self.shared.renderer.av_transport,
            action,
            &full,
        );
        self.shared.epoch.fetch_add(1, Ordering::AcqRel);
        if let Err(e) = &result {
            log::warn!("upnp: {}: {e}", self.shared.renderer.name);
            if matches!(e, SoapError::Unreachable(_)) {
                self.shared.lost();
            }
        }
        result.map(|_| ())
    }
}

impl std::fmt::Debug for Session {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Session")
            .field("renderer", &self.shared.renderer.name)
            .field("base", &self.base)
            .finish()
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.shared.closed.store(true, Ordering::Release);
        let _ = self.stop.try_send(());
        super::discovery::forget_gone(self.gone);
        // Unsubscribe off this thread: the renderer may be gone, and whoever
        // closed the session is not waiting on its answer.
        let shared = self.shared.clone();
        let _ = thread::Builder::new()
            .name("koan-upnp-close".into())
            .spawn(move || {
                for sub in shared.subscriptions.lock().drain(..) {
                    let _ = shared
                        .http
                        .request(method("UNSUBSCRIBE"), sub.service.events.clone())
                        .header("SID", &sub.sid)
                        .send();
                }
            });
    }
}

fn method(name: &str) -> reqwest::Method {
    reqwest::Method::from_bytes(name.as_bytes()).expect("valid method")
}

impl Shared {
    /// Say once that the renderer is gone.
    fn lost(&self) {
        if !self.unreachable.swap(true, Ordering::AcqRel) && !self.closed.load(Ordering::Acquire) {
            log::info!("upnp: {} is gone", self.renderer.name);
            (self.on_event)(Event::Gone);
        }
    }

    fn subscribe(&self, service: &Service) -> Result<Subscription, String> {
        let response = self
            .http
            .request(method("SUBSCRIBE"), service.events.clone())
            .header("CALLBACK", &self.callback)
            .header("NT", "upnp:event")
            .header("TIMEOUT", format!("Second-{SUBSCRIPTION_SECONDS}"))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let sub = subscription(service, &response)?;
        self.trust.sids.lock().push(sub.sid.clone());
        Ok(sub)
    }

    fn renew(&self, sub: &Subscription) -> Result<Subscription, String> {
        let response = self
            .http
            .request(method("SUBSCRIBE"), sub.service.events.clone())
            .header("SID", &sub.sid)
            .header("TIMEOUT", format!("Second-{SUBSCRIPTION_SECONDS}"))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let renewed = subscription(&sub.service, &response)?;
        self.trust.sids.lock().push(renewed.sid.clone());
        Ok(renewed)
    }

    fn snapshot(&self) -> Result<Snapshot, SoapError> {
        let epoch = self.epoch.load(Ordering::Acquire);
        let avt = &self.renderer.av_transport;
        let id = [("InstanceID", "0")];
        let info = soap::call(&self.http, avt, "GetTransportInfo", &id)?;
        let position = soap::call(&self.http, avt, "GetPositionInfo", &id)?;
        Ok(Snapshot {
            epoch,
            transport: Transport::parse(arg(&info, "CurrentTransportState").unwrap_or_default()),
            track_uri: arg(&position, "TrackURI").unwrap_or_default().to_string(),
            position_ms: arg(&position, "RelTime").and_then(soap::parse_time),
            duration_ms: arg(&position, "TrackDuration")
                .and_then(soap::parse_time)
                .filter(|d| *d > 0),
            at: Instant::now(),
        })
    }

    fn volume(&self) -> Option<u8> {
        let rc = self.renderer.rendering_control.as_ref()?;
        let args = soap::call(
            &self.http,
            rc,
            "GetVolume",
            &[("InstanceID", "0"), ("Channel", "Master")],
        )
        .ok()?;
        arg(&args, "CurrentVolume")?
            .parse::<u32>()
            .ok()
            .map(|v| v.min(100) as u8)
    }
}

fn subscription(
    service: &Service,
    response: &reqwest::blocking::Response,
) -> Result<Subscription, String> {
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .map(str::to_string)
    };
    let sid = header("SID").ok_or("no SID in the reply")?;
    let seconds = header("TIMEOUT")
        .and_then(|t| {
            t.trim()
                .strip_prefix("Second-")
                .and_then(|s| s.parse().ok())
        })
        .unwrap_or(SUBSCRIPTION_SECONDS)
        .max(30);
    Ok(Subscription {
        service: service.clone(),
        sid,
        // Renewed at half its lifetime, so one failed attempt leaves time
        // for another.
        renew_at: Instant::now() + Duration::from_secs(seconds / 2),
    })
}

fn spawn_watcher(
    shared: Arc<Shared>,
    look: Receiver<()>,
    seen: Arc<AtomicBool>,
    on_event: Arc<dyn Fn(Event) + Send + Sync>,
    stop: Receiver<()>,
) {
    let spawned = thread::Builder::new()
        .name("koan-upnp-watch".into())
        .spawn(move || {
            let started = Instant::now();
            loop {
                let polling = shared.polling.load(Ordering::Acquire);
                let wait = if polling {
                    shared.playing.load(Ordering::Acquire).then_some(POLL)
                } else if !seen.load(Ordering::Acquire) {
                    Some(INITIAL_EVENT.saturating_sub(started.elapsed()))
                } else {
                    None
                };
                let woke = crossbeam_channel::select! {
                    recv(look) -> r => r.is_ok(),
                    recv(stop) -> _ => return,
                    default(wait.unwrap_or(Duration::from_secs(3600))) => wait.is_some(),
                };
                if shared.closed.load(Ordering::Acquire) {
                    return;
                }
                if !woke {
                    continue;
                }
                if !polling && !seen.load(Ordering::Acquire) && started.elapsed() >= INITIAL_EVENT {
                    log::info!(
                        "upnp: {} sent no events; asking it once a second while playing",
                        shared.renderer.name
                    );
                    shared.polling.store(true, Ordering::Release);
                }
                match shared.snapshot() {
                    Ok(snapshot) => on_event(Event::Snapshot(snapshot)),
                    Err(e) => log::debug!("upnp: {}: {e}", shared.renderer.name),
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("upnp: could not start the watcher: {e}");
    }
}

fn spawn_renewer(shared: Arc<Shared>, stop: Receiver<()>) {
    let spawned = thread::Builder::new()
        .name("koan-upnp-renew".into())
        .spawn(move || {
            loop {
                let next = shared.subscriptions.lock().iter().map(|s| s.renew_at).min();
                let wait = next
                    .map(|at| at.saturating_duration_since(Instant::now()))
                    .unwrap_or(Duration::from_secs(3600));
                match stop.recv_timeout(wait) {
                    Err(RecvTimeoutError::Timeout) => {}
                    _ => return,
                }
                let now = Instant::now();
                let due: Vec<Subscription> = {
                    let mut subs = shared.subscriptions.lock();
                    let (due, keep) = subs.drain(..).partition(|s| s.renew_at <= now);
                    *subs = keep;
                    due
                };
                for sub in due {
                    // A renderer that rebooted has forgotten the SID; a fresh
                    // subscription is the only way back.
                    let renewed = shared
                        .renew(&sub)
                        .or_else(|_| shared.subscribe(&sub.service));
                    match renewed {
                        Ok(s) => shared.subscriptions.lock().push(s),
                        Err(e) => {
                            log::info!("upnp: lost events from {}: {e}", shared.renderer.name);
                            if sub.service == shared.renderer.av_transport {
                                shared.polling.store(true, Ordering::Release);
                                let _ = shared.look.try_send(());
                            }
                        }
                    }
                }
            }
        });
    if let Err(e) = spawned {
        log::warn!("upnp: could not start subscription renewal: {e}");
    }
}

#[derive(Debug, Default, PartialEq)]
pub(crate) struct Change {
    /// Something about the transport moved: the state or the track.
    pub transport: bool,
    pub volume: Option<u8>,
}

/// Read a GENA `NOTIFY` body: a property set whose `LastChange` holds an
/// escaped `<Event>` document.
pub(crate) fn parse_notify(body: &str) -> Change {
    let mut change = Change::default();
    let Ok(set) = xml::parse(body) else {
        return change;
    };
    for property in set.children_named("property") {
        let Some(last) = property.child("LastChange") else {
            continue;
        };
        let Ok(event) = xml::parse(last.text.trim()) else {
            continue;
        };
        for instance in event.children_named("InstanceID") {
            for var in &instance.children {
                match var.name.as_str() {
                    "TransportState" | "CurrentTrackURI" | "AVTransportURI" => {
                        change.transport = true;
                    }
                    "Volume" if var.attr("channel").is_none_or(|c| c == "Master") => {
                        change.volume = var
                            .attr("val")
                            .and_then(|v| v.parse::<u32>().ok())
                            .map(|v| v.min(100) as u8);
                    }
                    _ => {}
                }
            }
        }
    }
    change
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wrap(event: &str) -> String {
        format!(
            "<?xml version=\"1.0\"?><e:propertyset xmlns:e=\"urn:schemas-upnp-org:event-1-0\"><e:property><LastChange>{}</LastChange></e:property></e:propertyset>",
            xml::escape(event)
        )
    }

    #[test]
    fn a_transport_change_asks_for_a_look() {
        let change = parse_notify(&wrap(include_str!("fixtures/lastchange-avt.xml")));
        assert_eq!(
            change,
            Change {
                transport: true,
                volume: None
            }
        );
    }

    #[test]
    fn a_volume_change_carries_the_master_volume() {
        let change = parse_notify(&wrap(include_str!("fixtures/lastchange-rc.xml")));
        assert_eq!(
            change,
            Change {
                transport: false,
                volume: Some(37)
            }
        );
    }

    #[test]
    fn garbage_is_no_change() {
        assert_eq!(parse_notify("not xml <"), Change::default());
    }

    #[test]
    fn transport_states_parse() {
        assert_eq!(Transport::parse("PLAYING"), Transport::Playing);
        assert_eq!(Transport::parse("PAUSED_PLAYBACK"), Transport::Paused);
        assert_eq!(Transport::parse("STOPPED"), Transport::Stopped);
        assert_eq!(Transport::parse("NO_MEDIA_PRESENT"), Transport::NoMedia);
    }
}
