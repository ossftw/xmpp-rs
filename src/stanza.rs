use quick_xml::events::{BytesStart, Event};
use quick_xml::Reader;
use std::collections::HashMap;

#[derive(Debug, Clone, PartialEq)]
pub enum StanzaKind {
    Message,
    Presence,
    Iq,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Stanza {
    pub name: String,
    pub kind: StanzaKind,
    pub id: Option<String>,
    pub from: Option<String>,
    pub to: Option<String>,
    pub r#type: Option<String>,
    pub xmlns: Option<String>,
    pub lang: Option<String>,
    pub children: Vec<StanzaChild>,
    pub attrs: HashMap<String, String>,
    pub raw: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum StanzaChild {
    Element {
        name: String,
        attrs: HashMap<String, String>,
        children: Vec<StanzaChild>,
        text: Option<String>,
        xmlns: Option<String>,
    },
    Text(String),
}

impl Stanza {
    pub fn new(name: &str, kind: StanzaKind) -> Self {
        Self {
            name: name.to_string(),
            kind,
            id: None,
            from: None,
            to: None,
            r#type: None,
            xmlns: None,
            lang: None,
            children: Vec::new(),
            attrs: HashMap::new(),
            raw: None,
        }
    }

    pub fn is_message(&self) -> bool {
        self.kind == StanzaKind::Message
    }

    pub fn is_presence(&self) -> bool {
        self.kind == StanzaKind::Presence
    }

    pub fn is_iq(&self) -> bool {
        self.kind == StanzaKind::Iq
    }

    pub fn get_child_text(&self, name: &str) -> Option<String> {
        for child in &self.children {
            if let StanzaChild::Element {
                name: n,
                children,
                ..
            } = child
            {
                if n == name {
                    if let Some(StanzaChild::Text(t)) = children.first() {
                        return Some(t.clone());
                    }
                }
            }
        }
        None
    }

    pub fn get_child_by_xmlns(&self, name: &str, xmlns: &str) -> Option<&StanzaChild> {
        for child in &self.children {
            if let StanzaChild::Element {
                name: n,
                xmlns: x,
                ..
            } = child
            {
                if n == name && x.as_deref() == Some(xmlns) {
                    return Some(child);
                }
            }
        }
        None
    }

    pub fn has_child_with_xmlns(&self, name: &str, xmlns: &str) -> bool {
        self.get_child_by_xmlns(name, xmlns).is_some()
    }

    pub fn to_xml_string(&self) -> String {
        let tag = match self.kind {
            StanzaKind::Message => "message",
            StanzaKind::Presence => "presence",
            StanzaKind::Iq => "iq",
        };

        let mut xml = format!("<{}", tag);

        if let Some(ref id) = self.id {
            xml.push_str(&format!(" id=\"{}\"", escape_xml(id)));
        }
        if let Some(ref from) = self.from {
            xml.push_str(&format!(" from=\"{}\"", escape_xml(from)));
        }
        if let Some(ref to) = self.to {
            xml.push_str(&format!(" to=\"{}\"", escape_xml(to)));
        }
        if let Some(ref t) = self.r#type {
            xml.push_str(&format!(" type=\"{}\"", escape_xml(t)));
        }
        if let Some(ref xmlns) = self.xmlns {
            xml.push_str(&format!(" xmlns=\"{}\"", escape_xml(xmlns)));
        }
        if let Some(ref lang) = self.lang {
            xml.push_str(&format!(" xml:lang=\"{}\"", escape_xml(lang)));
        }
        for (k, v) in &self.attrs {
            xml.push_str(&format!(" {}=\"{}\"", escape_xml(k), escape_xml(v)));
        }

        if self.children.is_empty() {
            xml.push_str("/>");
        } else {
            xml.push('>');
            for child in &self.children {
                xml.push_str(&child_to_xml(child));
            }
            xml.push_str(&format!("</{}>", tag));
        }

        xml
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_xml_string().into_bytes()
    }
}

fn child_to_xml(child: &StanzaChild) -> String {
    match child {
        StanzaChild::Text(t) => escape_xml(t),
        StanzaChild::Element {
            name,
            attrs,
            children,
            text,
            xmlns,
        } => {
            let mut xml = format!("<{}", name);
            if let Some(ref xmlns) = xmlns {
                xml.push_str(&format!(" xmlns=\"{}\"", escape_xml(xmlns)));
            }
            for (k, v) in attrs {
                xml.push_str(&format!(" {}=\"{}\"", escape_xml(k), escape_xml(v)));
            }
            if children.is_empty() && text.is_none() {
                xml.push_str("/>");
            } else {
                xml.push('>');
                if let Some(ref t) = text {
                    xml.push_str(&escape_xml(t));
                }
                for child in children {
                    xml.push_str(&child_to_xml(child));
                }
                xml.push_str(&format!("</{}>", name));
            }
            xml
        }
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

pub enum ParseResult {
    Stanza(Stanza),
    StreamOpen { xmlns: String, to: Option<String>, from: Option<String>, id: Option<String>, version: Option<String> },
    StreamClose,
    Error(String),
    Incomplete,
}

pub fn parse_stream_element(xml: &str) -> ParseResult {
    let trimmed = xml.trim();
    if trimmed.is_empty() {
        return ParseResult::Incomplete;
    }

    if trimmed.starts_with("</stream:stream") || trimmed.starts_with("</stream") {
        return ParseResult::StreamClose;
    }

    if trimmed.starts_with("<stream:stream") || trimmed.starts_with("<stream ") || trimmed.starts_with("<?xml") {
        if trimmed.contains("<stream:stream") || trimmed.contains("<stream ") {
            let mut reader = Reader::from_str(trimmed);
            let mut xmlns = String::new();
            let mut to = None;
            let mut from = None;
            let mut id = None;
            let mut version = None;

            if let Ok(Event::Start(e)) = reader.read_event_into(&mut Vec::new()) {
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    let val = String::from_utf8_lossy(&attr.value).to_string();
                    if key == "xmlns" {
                        xmlns = val;
                    } else if key == "to" {
                        to = Some(val);
                    } else if key == "from" {
                        from = Some(val);
                    } else if key == "id" {
                        id = Some(val);
                    } else if key == "version" || key == "xmlns:stream" {
                        if key == "version" {
                            version = Some(val);
                        }
                    }
                }
            }

            return ParseResult::StreamOpen { xmlns, to, from, id, version };
        }
        return ParseResult::Incomplete;
    }

    let mut reader = Reader::from_str(trimmed);
    reader.config_mut().trim_text(true);

    match reader.read_event_into(&mut Vec::new()) {
        Ok(Event::Start(start)) | Ok(Event::Empty(start)) => {
            let name = String::from_utf8_lossy(start.name().as_ref()).to_string();

            let kind = match name.as_str() {
                "message" => StanzaKind::Message,
                "presence" => StanzaKind::Presence,
                "iq" => StanzaKind::Iq,
                _ => StanzaKind::Iq,
            };

            let (stanza, _) = parse_stanza_from_events(trimmed, kind, &start);
            ParseResult::Stanza(stanza)
        }
        Ok(Event::Eof) => ParseResult::Incomplete,
        Err(e) => ParseResult::Error(format!("XML parse error: {}", e)),
        _ => ParseResult::Incomplete,
    }
}

fn parse_stanza_from_events(xml: &str, kind: StanzaKind, root: &BytesStart) -> (Stanza, usize) {
    let name = String::from_utf8_lossy(root.name().as_ref()).to_string();
    let mut stanza = Stanza::new(&name, kind);
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);

    for attr in root.attributes().flatten() {
        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
        let val = String::from_utf8_lossy(&attr.value).to_string();
        match key.as_str() {
            "id" => stanza.id = Some(val),
            "from" => stanza.from = Some(val),
            "to" => stanza.to = Some(val),
            "type" => stanza.r#type = Some(val),
            "xmlns" => stanza.xmlns = Some(val),
            "xml:lang" => stanza.lang = Some(val),
            _ => {
                stanza.attrs.insert(key, val);
            }
        }
    }

    let mut buf = Vec::<u8>::new();
    let mut depth = 0u32;

    loop {
        match reader.read_event_into(&mut buf) {
            Ok(Event::Start(e)) => {
                depth += 1;
                if depth > 1 {
                    let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                    let mut attrs = HashMap::new();
                    let mut element_xmlns = None;
                    for attr in e.attributes().flatten() {
                        let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                        let val = String::from_utf8_lossy(&attr.value).to_string();
                        if key == "xmlns" {
                            element_xmlns = Some(val.clone());
                        }
                        attrs.insert(key, val);
                    }

                    let child = StanzaChild::Element {
                        name,
                        attrs,
                        children: Vec::new(),
                        text: None,
                        xmlns: element_xmlns,
                    };

                    stanza.children.push(child);
                }
            }
            Ok(Event::Empty(e)) => {
                let name = String::from_utf8_lossy(e.name().as_ref()).to_string();
                let mut attrs = HashMap::new();
                let mut element_xmlns = None;
                for attr in e.attributes().flatten() {
                    let key = String::from_utf8_lossy(attr.key.as_ref()).to_string();
                    let val = String::from_utf8_lossy(&attr.value).to_string();
                    if key == "xmlns" {
                        element_xmlns = Some(val.clone());
                    }
                    attrs.insert(key, val);
                }

                stanza.children.push(StanzaChild::Element {
                    name,
                    attrs,
                    children: Vec::new(),
                    text: None,
                    xmlns: element_xmlns,
                });
            }
            Ok(Event::Text(e)) => {
                let text = e.unescape().unwrap_or_default().to_string();
                if !text.trim().is_empty() && depth == 1 {
                    stanza.children.push(StanzaChild::Text(text));
                }
            }
            Ok(Event::End(_)) => {
                if depth == 0 {
                    break;
                }
                depth -= 1;
                if depth == 0 {
                    break;
                }
            }
            Ok(Event::Eof) => break,
            Err(_) => break,
            _ => {}
        }
        buf.clear();
    }

    stanza.raw = Some(xml.to_string());
    (stanza, 0)
}

pub fn build_stream_open(domain: &str, xmlns: &str, id: &str) -> String {
    format!(
        "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='http://etherx.jabber.org/streams' id='{}' from='{}' version='1.0' xml:lang='en'>",
        xmlns, id, domain
    )
}

pub fn build_stream_close() -> String {
    "</stream:stream>".to_string()
}

pub fn build_features_post_auth() -> String {
    r#"<stream:features>
  <bind xmlns="urn:ietf:params:xml:ns:xmpp-bind"/>
  <session xmlns="urn:ietf:params:xml:ns:xmpp-session"/>
</stream:features>"#.to_string()
}

pub fn build_features(xmlns_c2s: bool) -> String {
    if xmlns_c2s {
        r#"<stream:features>
  <mechanisms xmlns="urn:ietf:params:xml:ns:xmpp-sasl">
    <mechanism>PLAIN</mechanism>
    <mechanism>SCRAM-SHA-1</mechanism>
  </mechanisms>
  <c xmlns="http://jabber.org/protocol/caps" node="http://ossftw.com/xmpp/server" ver="1.0" hash="sha-1"/>
  <register xmlns="http://jabber.org/features/iq-register"/>
  <ver xmlns="urn:ietf:params:xml:ns:xmpp-stanzas"/>
</stream:features>"#.to_string()
    } else {
        r#"<stream:features>
  <mechanisms xmlns="urn:ietf:params:xml:ns:xmpp-sasl">
    <mechanism>EXTERNAL</mechanism>
  </mechanisms>
</stream:features>"#.to_string()
    }
}

pub fn build_sasl_challenge(challenge: &str) -> String {
    format!("<challenge xmlns='urn:ietf:params:xml:ns:xmpp-sasl'>{}</challenge>", challenge)
}

pub fn build_sasl_success(additional: Option<&str>) -> String {
    match additional {
        Some(data) => format!("<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'>{}</success>", data),
        None => "<success xmlns='urn:ietf:params:xml:ns:xmpp-sasl'/>".to_string(),
    }
}

pub fn build_sasl_failure(reason: &str) -> String {
    format!("<failure xmlns='urn:ietf:params:xml:ns:xmpp-sasl'><{}/></failure>", reason)
}

pub fn build_bind_result(jid: &str, sid: &str) -> String {
    format!(
        r#"<iq id="{sid}" type="result"><bind xmlns="urn:ietf:params:xml:ns:xmpp-bind"><jid>{jid}</jid></bind></iq>"#,
        sid = sid,
        jid = jid
    )
}

pub fn build_session_result(sid: &str) -> String {
    format!(
        r#"<iq id="{sid}" type="result"><session xmlns="urn:ietf:params:xml:ns:xmpp-session"/></iq>"#,
        sid = sid
    )
}

pub fn build_iq_result(id: &str, xmlns: &str, child_xml: &str) -> String {
    format!(
        r#"<iq id="{id}" type="result" xmlns="{xmlns}">{child_xml}</iq>"#,
        id = id,
        xmlns = xmlns,
        child_xml = child_xml
    )
}

pub fn build_iq_error(id: &str, condition: &str, text: &str) -> String {
    format!(
        r#"<iq id="{id}" type="error"><error type='cancel'><{condition} xmlns='urn:ietf:params:xml:ns:xmpp-stanzas'/><text xmlns='urn:ietf:params:xml:ns:xmpp-stanzas'>{text}</text></error></iq>"#,
        id = id,
        condition = condition,
        text = escape_xml(text)
    )
}

pub fn build_message(from: &str, to: &str, body: &str, kind: &str) -> String {
    format!(
        r#"<message from="{from}" to="{to}" type="{kind}"><body>{body}</body></message>"#,
        from = escape_xml(from),
        to = escape_xml(to),
        kind = kind,
        body = escape_xml(body)
    )
}

pub fn build_presence(from: &str, to: &str, ptype: Option<&str>, show: Option<&str>, status: Option<&str>) -> String {
    let mut xml = format!(r#"<presence from="{from}" to="{to}""#, from = escape_xml(from), to = escape_xml(to));
    if let Some(t) = ptype {
        xml.push_str(&format!(" type=\"{}\"", t));
    }
    xml.push('>');
    if let Some(s) = show {
        xml.push_str(&format!("<show>{}</show>", s));
    }
    if let Some(s) = status {
        xml.push_str(&format!("<status>{}</status>", escape_xml(s)));
    }
    xml.push_str("</presence>");
    xml
}

pub fn extract_stanza_name(xml: &str) -> Option<String> {
    let trimmed = xml.trim();
    let rest = if trimmed.starts_with('<') { &trimmed[1..] } else { return None };
    let end = rest.find(|c: char| c.is_whitespace() || c == '>' || c == '/')?;
    Some(rest[..end].to_string())
}
