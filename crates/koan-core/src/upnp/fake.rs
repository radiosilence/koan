//! A renderer in a thread, for tests: enough AVTransport, RenderingControl,
//! ConnectionManager and GENA to be driven the way a real one is.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::thread;

use parking_lot::Mutex;
use url::Url;

use super::description::{AV_TRANSPORT, CONNECTION_MANAGER, RENDERING_CONTROL, Renderer, Service};
use super::serve::read_request;
use super::xml;

#[derive(Debug, Default)]
pub struct State {
    pub transport: &'static str,
    pub uri: String,
    pub next_uri: String,
    pub position_ms: u64,
    pub volume: u8,
    /// Every action, in order, with its arguments.
    pub actions: Vec<(String, Vec<(String, String)>)>,
    /// URLs it fetched, and how many bytes came back.
    pub fetched: Vec<(String, usize)>,
    callbacks: Vec<String>,
}

pub struct FakeRenderer {
    pub state: Arc<Mutex<State>>,
    port: u16,
    pub sink: &'static str,
    pub gapless: bool,
    pub events: bool,
    /// Go on reporting the previous URI after a new one is set, as Kodi
    /// does while it opens the new one.
    pub lag: std::sync::atomic::AtomicBool,
    stale_uri: Mutex<Option<String>>,
}

impl FakeRenderer {
    pub fn start(sink: &'static str, gapless: bool, events: bool) -> Arc<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let fake = Arc::new(Self {
            state: Arc::new(Mutex::new(State {
                transport: "NO_MEDIA_PRESENT",
                volume: 20,
                ..Default::default()
            })),
            port,
            sink,
            gapless,
            events,
            lag: Default::default(),
            stale_uri: Default::default(),
        });
        let serving = fake.clone();
        thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let fake = serving.clone();
                thread::spawn(move || fake.handle(stream));
            }
        });
        fake
    }

    fn url(&self, path: &str) -> Url {
        Url::parse(&format!("http://127.0.0.1:{}{path}", self.port)).unwrap()
    }

    pub fn renderer(&self) -> Renderer {
        let service = |kind: &str, name: &str| Service {
            service_type: kind.to_string(),
            control: self.url(&format!("/{name}/control")),
            events: self.url(&format!("/{name}/event")),
            scpd: self.url(&format!("/{name}/scpd")),
        };
        Renderer {
            udn: format!("uuid:fake-{}", self.port),
            name: "Fake Amp".into(),
            manufacturer: "Test".into(),
            model: "F1".into(),
            location: self.url("/desc.xml"),
            av_transport: service(AV_TRANSPORT, "avt"),
            rendering_control: Some(service(RENDERING_CONTROL, "rc")),
            connection_manager: Some(service(CONNECTION_MANAGER, "cm")),
            gapless: self.gapless,
            openhome: false,
        }
    }

    pub fn actions(&self) -> Vec<String> {
        self.state
            .lock()
            .actions
            .iter()
            .map(|(a, _)| a.clone())
            .collect()
    }

    /// The track reaches its end, as the renderer would play it out.
    pub fn finish_track(&self) {
        {
            let mut s = self.state.lock();
            if s.next_uri.is_empty() {
                s.transport = "STOPPED";
                s.position_ms = 0;
            } else {
                s.uri = std::mem::take(&mut s.next_uri);
                s.transport = "PLAYING";
                s.position_ms = 0;
            }
        }
        self.notify_transport();
    }

    /// Someone pressed stop on the amplifier's remote.
    pub fn press_stop(&self, at_ms: u64) {
        {
            let mut s = self.state.lock();
            s.transport = "STOPPED";
            s.position_ms = at_ms;
        }
        self.notify_transport();
    }

    /// Another control point starts something on it.
    pub fn play_foreign(&self, uri: &str) {
        {
            let mut s = self.state.lock();
            s.uri = uri.to_string();
            s.next_uri.clear();
            s.transport = "PLAYING";
            s.position_ms = 0;
        }
        self.notify_transport();
    }

    pub fn set_position(&self, ms: u64) {
        self.state.lock().position_ms = ms;
    }

    fn notify_transport(&self) {
        let (state, uri) = {
            let s = self.state.lock();
            (s.transport, s.uri.clone())
        };
        self.notify(&format!(
            "<Event xmlns=\"urn:schemas-upnp-org:metadata-1-0/AVT/\"><InstanceID val=\"0\"><TransportState val=\"{state}\"/><CurrentTrackURI val=\"{}\"/></InstanceID></Event>",
            xml::escape(&uri)
        ));
    }

    fn notify(&self, event: &str) {
        if !self.events {
            return;
        }
        let body = format!(
            "<?xml version=\"1.0\"?><e:propertyset xmlns:e=\"urn:schemas-upnp-org:event-1-0\"><e:property><LastChange>{}</LastChange></e:property></e:propertyset>",
            xml::escape(event)
        );
        let callbacks = self.state.lock().callbacks.clone();
        for callback in callbacks {
            let url = Url::parse(callback.trim_matches(['<', '>'])).unwrap();
            let body = body.clone();
            thread::spawn(move || {
                let mut stream =
                    TcpStream::connect((url.host_str().unwrap(), url.port().unwrap())).unwrap();
                write!(
                    stream,
                    "NOTIFY {} HTTP/1.1\r\nHOST: x\r\nCONTENT-TYPE: text/xml\r\nNT: upnp:event\r\nNTS: upnp:propchange\r\nSID: uuid:sub\r\nSEQ: 0\r\nContent-Length: {}\r\n\r\n{body}",
                    url.path(),
                    body.len()
                )
                .unwrap();
                let mut sink = Vec::new();
                let _ = stream.read_to_end(&mut sink);
            });
        }
    }

    fn handle(&self, mut stream: TcpStream) {
        let Ok(req) = read_request(&stream) else {
            return;
        };
        match req.method.as_str() {
            "SUBSCRIBE" => {
                if let Some(cb) = req.header("CALLBACK") {
                    self.state.lock().callbacks.push(cb.to_string());
                }
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nSID: uuid:sub\r\nTIMEOUT: Second-1800\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
                drop(stream);
                if req.path.starts_with("/avt") {
                    self.notify_transport();
                }
            }
            "UNSUBSCRIBE" => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
            "POST" => {
                let soap_action = req.header("SOAPACTION").unwrap_or_default().to_string();
                let action = soap_action
                    .trim_matches('"')
                    .rsplit('#')
                    .next()
                    .unwrap_or_default()
                    .to_string();
                let envelope = xml::parse(&String::from_utf8_lossy(&req.body)).unwrap();
                let args: Vec<(String, String)> = envelope
                    .child("Body")
                    .and_then(|b| b.children.first())
                    .map(|a| {
                        a.children
                            .iter()
                            .map(|c| (c.name.clone(), c.text.clone()))
                            .collect()
                    })
                    .unwrap_or_default();
                let out = self.act(&action, &args);
                let body: String = out
                    .iter()
                    .map(|(k, v)| format!("<{k}>{}</{k}>", xml::escape(v)))
                    .collect();
                let reply = format!(
                    "<?xml version=\"1.0\"?><s:Envelope xmlns:s=\"http://schemas.xmlsoap.org/soap/envelope/\"><s:Body><u:{action}Response xmlns:u=\"x\">{body}</u:{action}Response></s:Body></s:Envelope>"
                );
                let _ = write!(
                    stream,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/xml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}",
                    reply.len()
                );
            }
            _ => {
                let _ = write!(
                    stream,
                    "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                );
            }
        }
    }

    fn act(&self, action: &str, args: &[(String, String)]) -> Vec<(String, String)> {
        let get = |name: &str| {
            args.iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.clone())
                .unwrap_or_default()
        };
        let mut s = self.state.lock();
        s.actions.push((action.to_string(), args.to_vec()));
        let mut transport_changed = false;
        let out = match action {
            "GetProtocolInfo" => vec![
                ("Source".into(), String::new()),
                ("Sink".into(), self.sink.to_string()),
            ],
            "SetAVTransportURI" => {
                if self.lag.load(std::sync::atomic::Ordering::Relaxed) && !s.uri.is_empty() {
                    *self.stale_uri.lock() = Some(s.uri.clone());
                }
                s.uri = get("CurrentURI");
                s.next_uri.clear();
                s.transport = "STOPPED";
                s.position_ms = 0;
                let uri = s.uri.clone();
                drop(s);
                self.fetch(&uri);
                s = self.state.lock();
                vec![]
            }
            "SetNextAVTransportURI" => {
                s.next_uri = get("NextURI");
                vec![]
            }
            "Play" => {
                s.transport = "PLAYING";
                transport_changed = true;
                vec![]
            }
            "Pause" => {
                s.transport = "PAUSED_PLAYBACK";
                transport_changed = true;
                vec![]
            }
            "Stop" => {
                s.transport = "STOPPED";
                transport_changed = true;
                vec![]
            }
            "Seek" => {
                s.position_ms = super::soap::parse_time(&get("Target")).unwrap_or(0);
                vec![]
            }
            "GetTransportInfo" => vec![
                ("CurrentTransportState".into(), s.transport.to_string()),
                ("CurrentTransportStatus".into(), "OK".into()),
                ("CurrentSpeed".into(), "1".into()),
            ],
            "GetPositionInfo" => vec![
                ("Track".into(), "1".into()),
                ("TrackDuration".into(), "0:00:10".into()),
                (
                    "TrackURI".into(),
                    self.stale_uri
                        .lock()
                        .clone()
                        .unwrap_or_else(|| s.uri.clone()),
                ),
                ("RelTime".into(), super::soap::format_time(s.position_ms)),
            ],
            "GetVolume" => vec![("CurrentVolume".into(), s.volume.to_string())],
            "SetVolume" => {
                s.volume = get("DesiredVolume").parse().unwrap_or(0);
                vec![]
            }
            _ => vec![],
        };
        drop(s);
        if transport_changed {
            self.notify_transport();
        }
        out
    }

    /// Fetch what it was given, as a renderer starts to.
    fn fetch(&self, uri: &str) {
        let Ok(url) = Url::parse(uri) else { return };
        let Ok(mut stream) = TcpStream::connect((url.host_str().unwrap(), url.port().unwrap()))
        else {
            return;
        };
        let _ = write!(stream, "GET {} HTTP/1.1\r\nHost: x\r\n\r\n", url.path());
        let mut out = Vec::new();
        let _ = stream.read_to_end(&mut out);
        let body = out
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map_or(0, |at| out.len() - at - 4);
        self.state.lock().fetched.push((uri.to_string(), body));
    }
}
