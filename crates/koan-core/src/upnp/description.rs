//! A renderer's device description and the service descriptions it links to.

use url::Url;

use super::xml::{self, Element};

pub const AV_TRANSPORT: &str = "urn:schemas-upnp-org:service:AVTransport:1";
pub const RENDERING_CONTROL: &str = "urn:schemas-upnp-org:service:RenderingControl:1";
pub const CONNECTION_MANAGER: &str = "urn:schemas-upnp-org:service:ConnectionManager:1";

/// A MediaRenderer, as its description describes it.
#[derive(Debug, Clone, PartialEq)]
pub struct Renderer {
    /// `uuid:…`, the one identity a device keeps across addresses and reboots.
    pub udn: String,
    pub name: String,
    pub manufacturer: String,
    pub model: String,
    /// Where the description was fetched from.
    pub location: Url,
    pub av_transport: Service,
    pub rendering_control: Option<Service>,
    pub connection_manager: Option<Service>,
    /// The AVTransport SCPD lists `SetNextAVTransportURI`, so the renderer can
    /// be handed the next track before this one ends.
    pub gapless: bool,
    /// Advertises OpenHome services, which hold a playlist on the renderer.
    /// Recorded for later; koan drives AVTransport either way.
    pub openhome: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Service {
    pub service_type: String,
    pub control: Url,
    pub events: Url,
    pub scpd: Url,
}

/// Read a device description fetched from `location`. `None` when it
/// describes no device with an AVTransport service.
pub fn parse_device(doc: &str, location: &Url) -> Result<Option<Renderer>, String> {
    let root = xml::parse(doc)?;
    let base = root
        .child_text("URLBase")
        .filter(|b| !b.is_empty())
        .and_then(|b| Url::parse(b).ok())
        .unwrap_or_else(|| location.clone());
    let openhome = mentions_openhome(&root);
    let Some(device) = root.child("device").and_then(renderer_device) else {
        return Ok(None);
    };
    let service = |kind: &str| -> Option<Service> {
        // A `:2` renderer still answers `:1` requests; match on the name.
        let prefix = kind.trim_end_matches(":1");
        device
            .path(&["serviceList"])?
            .children_named("service")
            .find(|s| {
                s.child_text("serviceType")
                    .is_some_and(|t| t.starts_with(prefix))
            })
            .and_then(|s| {
                Some(Service {
                    service_type: s.child_text("serviceType")?.to_string(),
                    control: base.join(s.child_text("controlURL")?).ok()?,
                    events: base.join(s.child_text("eventSubURL")?).ok()?,
                    scpd: base.join(s.child_text("SCPDURL")?).ok()?,
                })
            })
    };
    let Some(av_transport) = service(AV_TRANSPORT) else {
        return Ok(None);
    };
    Ok(Some(Renderer {
        udn: device.child_text("UDN").unwrap_or_default().to_string(),
        name: device
            .child_text("friendlyName")
            .filter(|n| !n.is_empty())
            .unwrap_or("Renderer")
            .to_string(),
        manufacturer: device
            .child_text("manufacturer")
            .unwrap_or_default()
            .to_string(),
        model: device
            .child_text("modelName")
            .unwrap_or_default()
            .to_string(),
        location: location.clone(),
        av_transport,
        rendering_control: service(RENDERING_CONTROL),
        connection_manager: service(CONNECTION_MANAGER),
        gapless: false,
        openhome,
    }))
}

/// The device carrying AVTransport: the root itself, or one embedded in it.
fn renderer_device(device: &Element) -> Option<&Element> {
    let has_transport = device.path(&["serviceList"]).is_some_and(|list| {
        list.children_named("service").any(|s| {
            s.child_text("serviceType")
                .is_some_and(|t| t.contains(":service:AVTransport:"))
        })
    });
    if has_transport {
        return Some(device);
    }
    device
        .path(&["deviceList"])?
        .children_named("device")
        .find_map(renderer_device)
}

fn mentions_openhome(el: &Element) -> bool {
    (el.name == "serviceType" && el.text.contains("av-openhome-org"))
        || el.children.iter().any(mentions_openhome)
}

/// The action names a service description lists.
pub fn parse_actions(doc: &str) -> Result<Vec<String>, String> {
    let root = xml::parse(doc)?;
    Ok(root
        .child("actionList")
        .map(|list| {
            list.children_named("action")
                .filter_map(|a| a.child_text("name").map(str::to_string))
                .collect()
        })
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    const KODI: &str = include_str!("fixtures/kodi-device.xml");
    const EMBEDDED: &str = include_str!("fixtures/embedded-device.xml");
    const AVT_SCPD: &str = include_str!("fixtures/avtransport-scpd.xml");

    #[test]
    fn a_root_renderer_resolves_its_urls_against_the_location() {
        let location = Url::parse("http://192.168.1.20:1597/").unwrap();
        let r = parse_device(KODI, &location).unwrap().unwrap();
        assert_eq!(r.name, "Kodi (mac)");
        assert_eq!(r.udn, "uuid:bc9d8c87-6b5c-4b62-b4c5-4b7d2c3a6f1e");
        assert_eq!(
            r.av_transport.control.as_str(),
            "http://192.168.1.20:1597/AVTransport/bc9d8c87/control.xml"
        );
        assert!(r.rendering_control.is_some());
        assert!(r.connection_manager.is_some());
        assert!(!r.openhome);
    }

    #[test]
    fn an_embedded_renderer_is_found_and_url_base_wins() {
        let location = Url::parse("http://10.0.0.5:49152/description.xml").unwrap();
        let r = parse_device(EMBEDDED, &location).unwrap().unwrap();
        assert_eq!(r.name, "Living Room Amp");
        assert_eq!(
            r.av_transport.control.as_str(),
            "http://10.0.0.5:8080/upnp/control/avt"
        );
        assert!(r.openhome);
    }

    #[test]
    fn a_device_without_av_transport_is_not_a_renderer() {
        let location = Url::parse("http://10.0.0.9/").unwrap();
        let doc = r#"<root><device><deviceType>urn:schemas-upnp-org:device:MediaServer:1</deviceType><UDN>uuid:x</UDN><serviceList><service><serviceType>urn:schemas-upnp-org:service:ContentDirectory:1</serviceType><controlURL>/c</controlURL><eventSubURL>/e</eventSubURL><SCPDURL>/s</SCPDURL></service></serviceList></device></root>"#;
        assert!(parse_device(doc, &location).unwrap().is_none());
    }

    #[test]
    fn scpd_actions_include_set_next() {
        let actions = parse_actions(AVT_SCPD).unwrap();
        assert!(actions.iter().any(|a| a == "SetNextAVTransportURI"));
        assert!(actions.iter().any(|a| a == "GetPositionInfo"));
    }
}
