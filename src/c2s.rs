use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::time::Instant;
use tokio_rustls::TlsAcceptor;

use crate::auth::AuthManager;
use crate::config::Config;
use crate::muc::MucManager;
use crate::roster::RosterManager;
use crate::router::Router;
use crate::stanza;

struct RateLimiter {
    tokens: f64,
    last_refill: Instant,
    rate: f64,
    burst: f64,
}

impl RateLimiter {
    fn new(rate: f64, burst: f64) -> Self {
        Self {
            tokens: burst,
            last_refill: Instant::now(),
            rate,
            burst,
        }
    }

    fn check(&mut self) -> bool {
        let now = Instant::now();
        let elapsed = now.duration_since(self.last_refill).as_secs_f64();
        self.tokens = (self.tokens + elapsed * self.rate).min(self.burst);
        self.last_refill = now;

        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

#[derive(Clone)]
pub struct C2sHandler {
    config: Arc<Config>,
    router: Router,
    auth: AuthManager,
    roster: RosterManager,
    muc: MucManager,
}

impl C2sHandler {
    pub fn new(
        config: Arc<Config>,
        router: Router,
        auth: AuthManager,
        roster: RosterManager,
        muc: MucManager,
    ) -> Self {
        Self {
            config,
            router,
            auth,
            roster,
            muc,
        }
    }

    pub async fn start(&self, tls_acceptor: Option<TlsAcceptor>) -> anyhow::Result<()> {
        let addr = self.config.server.c2s_addr;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        log::info!("C2S listening on {}", addr);

        loop {
            let (stream, peer) = listener.accept().await?;
            let config = self.config.clone();
            let router = self.router.clone();
            let auth = self.auth.clone();
            let roster = self.roster.clone();
            let muc = self.muc.clone();
            let tls = tls_acceptor.clone();
            let domain = config.server.domain.clone();

            tokio::spawn(async move {
                log::info!("C2S connection from {}", peer);

                let mut peek_buf = [0u8; 1];
                let peeked = stream.peek(&mut peek_buf).await.unwrap_or(0);
                let is_tls = peeked > 0 && peek_buf[0] == 0x16;
                if is_tls {
                    if let Some(acceptor) = tls {
                        match acceptor.accept(stream).await {
                            Ok(tls_stream) => {
                                handle_c2s(tls_stream, domain, config, router, auth, roster, muc).await;
                            }
                            Err(e) => {
                                log::error!("C2S TLS error from {}: {}", peer, e);
                            }
                        }
                    } else {
                        handle_c2s(stream, domain, config, router, auth, roster, muc).await;
                    }
                } else {
                    if let Some(acceptor) = tls {
                        handle_starttls_c2s(stream, domain, config, router, auth, roster, muc, &acceptor).await;
                    } else {
                        handle_c2s(stream, domain, config, router, auth, roster, muc).await;
                    }
                }
            });
        }
    }
}

enum C2sState {
    StreamInit,
    AuthPending { stream_id: String },
    AuthSuccess { username: String, stream_id: String },
    BindPending { username: String, stream_id: String, resource: Option<String> },
    SessionEstablished { username: String, resource: String, full_jid: String, stream_id: String },
}

async fn handle_starttls_c2s(
    stream: TcpStream,
    domain: String,
    config: Arc<Config>,
    router: Router,
    auth: AuthManager,
    roster: RosterManager,
    muc: MucManager,
    acceptor: &TlsAcceptor,
) {
    let mut reader = tokio::io::BufReader::new(stream);
    let mut buf = String::new();

    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => return,
            Ok(_) => {
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if trimmed.starts_with("<starttls") && trimmed.contains("urn:ietf:params:xml:ns:xmpp-tls") {
                    let proceed = stanza::build_starttls_proceed();
                    if reader.get_mut().write_all(proceed.as_bytes()).await.is_err() {
                        return;
                    }
                    if reader.get_mut().flush().await.is_err() {
                        return;
                    }

                    let stream = reader.into_inner();
                    match acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            handle_c2s(tls_stream, domain, config, router, auth, roster, muc).await;
                        }
                        Err(e) => {
                            log::error!("C2S TLS error after STARTTLS: {}", e);
                        }
                    }
                    return;
                }

                if trimmed.contains("<stream:stream") || trimmed.contains("<?xml") {
                    let stream_id = uuid::Uuid::new_v4().to_string();
                    let features = stanza::build_features_with_starttls();
                    let response = format!(
                        "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='http://etherx.jabber.org/streams' id='{}' from='{}' version='1.0' xml:lang='en'>{}",
                        "jabber:client", stream_id, domain, features
                    );
                    if reader.get_mut().write_all(response.as_bytes()).await.is_err() {
                        return;
                    }
                    if reader.get_mut().flush().await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

async fn handle_c2s(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    domain: String,
    config: Arc<Config>,
    router: Router,
    auth: AuthManager,
    roster: RosterManager,
    muc: MucManager,
) {
    let (reader_half, writer_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader_half);
    let mut buf = String::new();
    let mut state = C2sState::StreamInit;

    let (stanza_tx, mut stanza_rx) = mpsc::unbounded_channel::<String>();
    let stanza_tx = Arc::new(stanza_tx);

    let mut writer = writer_half;

    let mut rate_limiter = RateLimiter::new(
        config.rate_limit.stanzas_per_second as f64,
        config.rate_limit.burst_size as f64,
    );

    let write_handle = tokio::spawn(async move {
        while let Some(stanza) = stanza_rx.recv().await {
            let to_send = format!("{}\n", stanza);
            if let Err(e) = writer.write_all(to_send.as_bytes()).await {
                log::debug!("Client write error: {}", e);
                break;
            }
            if let Err(e) = writer.flush().await {
                log::debug!("Client flush error: {}", e);
                break;
            }
        }
    });

    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue;
                }

                if !rate_limiter.check() {
                    log::warn!("Rate limit exceeded, disconnecting client");
                    break;
                }

                match stanza::parse_stream_element(trimmed) {
                    stanza::ParseResult::StreamOpen { to, .. } => {
                        if let Some(ref to_domain) = to {
                            if to_domain != &domain {
                                break;
                            }
                        }

                        let stream_id = uuid::Uuid::new_v4().to_string();

                        match state {
                            C2sState::AuthSuccess { ref username, .. } => {
                                let features = stanza::build_features_post_auth();
                                let response = format!(
                                    "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='http://etherx.jabber.org/streams' id='{}' from='{}' version='1.0' xml:lang='en'>{}",
                                    "jabber:client", stream_id, domain, features
                                );
                                let _ = write_stanza(&stanza_tx, &response).await;
                                state = C2sState::AuthSuccess {
                                    username: username.clone(),
                                    stream_id: stream_id.to_string(),
                                };
                            }
                            _ => {
                                let features = stanza::build_features(true);
                                let response = format!(
                                    "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='http://etherx.jabber.org/streams' id='{}' from='{}' version='1.0' xml:lang='en'>{}",
                                    "jabber:client", stream_id, domain, features
                                );
                                let _ = write_stanza(&stanza_tx, &response).await;
                                state = C2sState::AuthPending {
                                    stream_id: stream_id.to_string(),
                                };
                            }
                        }
                    }

                    stanza::ParseResult::Stanza(st) => {
                        let state_info = match &state {
                            C2sState::AuthPending { ref stream_id } => {
                                Some(("AuthPending", stream_id.clone(), String::new(), String::new(), String::new()))
                            }
                            C2sState::AuthSuccess { ref username, ref stream_id } => {
                                Some(("AuthSuccess", stream_id.clone(), username.clone(), String::new(), String::new()))
                            }
                            C2sState::BindPending { ref username, ref stream_id, ref resource } => {
                                Some(("BindPending", stream_id.clone(), username.clone(), resource.clone().unwrap_or_default(), String::new()))
                            }
                            C2sState::SessionEstablished { ref username, ref resource, ref full_jid, ref stream_id } => {
                                Some(("Session", stream_id.clone(), username.clone(), resource.clone(), full_jid.clone()))
                            }
                            C2sState::StreamInit => None,
                        };

                        match state_info {
                            Some(("AuthPending", ref sid, _, _, _)) => {
                                if let Some(new_state) = handle_auth_stanza(&st, sid, &auth, &stanza_tx).await {
                                    state = new_state;
                                }
                            }
                            Some(("AuthSuccess", ref sid, ref uname, _, _)) => {
                                let new_state = handle_auth_success_stanza(&st, uname, sid, &stanza_tx, &domain).await;
                                if let Some(s) = new_state {
                                    state = s;
                                }
                            }
                            Some(("BindPending", ref sid, ref uname, ref res, _)) => {
                                let new_state = handle_bind_stanza(
                                    &st, uname, sid, res, &domain, &router, &roster, &stanza_tx,
                                ).await;
                                if let Some(s) = new_state {
                                    state = s;
                                }
                            }
                            Some(("Session", ref sid, ref uname, ref res, ref fjid)) => {
                                handle_established_stanza(
                                    &st, uname, res, fjid, sid,
                                    &config, &router, &auth, &roster, &muc, &stanza_tx,
                                ).await;
                            }
                            _ => {
                                log::warn!("Stanza before stream init");
                            }
                        }
                    }

                    stanza::ParseResult::StreamClose => {
                        if let C2sState::SessionEstablished { ref full_jid, .. } = state {
                            router.unregister(full_jid).await;
                        }
                        break;
                    }

                    stanza::ParseResult::Error(e) => {
                        log::warn!("Parse error from client: {}", e);
                        break;
                    }

                    stanza::ParseResult::Incomplete => {}
                }
            }
        }
    }

    if let C2sState::SessionEstablished {
        ref full_jid,
        ref username,
        ref resource,
        ref stream_id,
        ..
    } = state
    {
        router.unregister(full_jid).await;
        roster.remove_resource(&format!("{}/{}", username, resource)).await;
        auth.remove_auth_state(stream_id).await;

        let bare_jid = format!("{}@{}", username, domain);
        let unavailable = stanza::build_presence(
            &format!("{}@{}", username, domain),
            &bare_jid,
            Some("unavailable"),
            None,
            Some("Connection closed"),
        );
        let _ = router.broadcast(&unavailable, Some(full_jid)).await;
    }

    write_handle.abort();
}

async fn write_stanza(
    writer: &mpsc::UnboundedSender<String>,
    xml: &str,
) -> Result<(), String> {
    writer.send(xml.to_string()).map_err(|e| format!("Send error: {}", e))
}

async fn handle_auth_stanza(
    st: &stanza::Stanza,
    stream_id: &str,
    auth: &AuthManager,
    writer: &mpsc::UnboundedSender<String>,
) -> Option<C2sState> {
    if st.name == "auth" && st.xmlns.as_deref() == Some("urn:ietf:params:xml:ns:xmpp-sasl") {
        let mechanism = st.attrs.get("mechanism").map(|s| s.as_str());

        let initial_data = match st.children.first() {
            Some(stanza::StanzaChild::Text(t)) => t.trim(),
            _ => "",
        };

        match mechanism {
            Some("PLAIN") => {
                match auth.authenticate_plain(stream_id, initial_data).await {
                    Ok(username) => {
                        let success = stanza::build_sasl_success(None);
                        if write_stanza(writer, &success).await.is_err() {
                            return None;
                        }
                        log::info!("User {} authenticated via PLAIN", username);
                        return Some(C2sState::AuthSuccess {
                            username,
                            stream_id: stream_id.to_string(),
                        });
                    }
                    Err(reason) => {
                        let failure = stanza::build_sasl_failure(&reason);
                        let _ = write_stanza(writer, &failure).await;
                    }
                }
            }
            Some("SCRAM-SHA-1") => {
                match auth.start_scram_sha1(stream_id, initial_data).await {
                    Ok((challenge, _username, _server_first)) => {
                        let challenge_xml = stanza::build_sasl_challenge(&challenge);
                        if write_stanza(writer, &challenge_xml).await.is_err() {
                            return None;
                        }
                    }
                    Err(reason) => {
                        let failure = stanza::build_sasl_failure(&reason);
                        let _ = write_stanza(writer, &failure).await;
                    }
                }
            }
            _ => {
                let failure = stanza::build_sasl_failure("invalid-mechanism");
                let _ = write_stanza(writer, &failure).await;
            }
        }
    }

    if st.name == "response" && st.xmlns.as_deref() == Some("urn:ietf:params:xml:ns:xmpp-sasl") {
        let response_data = match st.children.first() {
            Some(stanza::StanzaChild::Text(t)) => t.trim(),
            _ => "",
        };

        match auth.finish_scram_sha1(stream_id, response_data).await {
            Ok(success_data) => {
                let username = auth.get_username_for_stream(stream_id).await;
                let success = stanza::build_sasl_success(Some(&success_data));
                if write_stanza(writer, &success).await.is_err() {
                    return None;
                }
                if let Some(u) = username {
                    return Some(C2sState::AuthSuccess {
                        username: u,
                        stream_id: stream_id.to_string(),
                    });
                }
            }
            Err(reason) => {
                let failure = stanza::build_sasl_failure(&reason);
                let _ = write_stanza(writer, &failure).await;
            }
        }
    }
    None
}

async fn handle_auth_success_stanza(
    st: &stanza::Stanza,
    username: &str,
    stream_id: &str,
    writer: &mpsc::UnboundedSender<String>,
    domain: &str,
) -> Option<C2sState> {
    if st.is_iq() {
        if let Some(bind) = st.get_child_by_xmlns("bind", "urn:ietf:params:xml:ns:xmpp-bind") {
            let resource = match bind {
                stanza::StanzaChild::Element { children, .. } => {
                    children.iter().find_map(|c| {
                        if let stanza::StanzaChild::Element { name, children, .. } = c {
                            if name == "resource" {
                                if let Some(stanza::StanzaChild::Text(t)) = children.first() {
                                    return Some(t.clone());
                                }
                            }
                        }
                        None
                    })
                }
                _ => None,
            };

            let resource = resource.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            let full_jid = format!("{}@{}/{}", username, domain, resource);

            let bind_result = stanza::build_bind_result(&full_jid, st.id.as_deref().unwrap_or("bind_1"));
            if write_stanza(writer, &bind_result).await.is_err() {
                return None;
            }

            return Some(C2sState::BindPending {
                username: username.to_string(),
                stream_id: stream_id.to_string(),
                resource: Some(resource),
            });
        }
    }
    None
}

async fn handle_bind_stanza(
    st: &stanza::Stanza,
    username: &str,
    stream_id: &str,
    resource: &str,
    domain: &str,
    router: &Router,
    roster: &RosterManager,
    writer: &mpsc::UnboundedSender<String>,
) -> Option<C2sState> {
    if st.is_iq() && st.has_child_with_xmlns("session", "urn:ietf:params:xml:ns:xmpp-session") {
        let session_result = stanza::build_session_result(st.id.as_deref().unwrap_or("sess_1"));
        if write_stanza(writer, &session_result).await.is_err() {
            return None;
        }

        let resource = if resource.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            resource.to_string()
        };
        let full_jid = format!("{}@{}/{}", username, domain, resource);

        router.register(&full_jid, writer.clone()).await;

        let roster_items = roster.get_roster(username).await;
        if !roster_items.is_empty() {
            let roster_iq = build_roster_iq(&roster_items);
            let iq = format!(
                r#"<iq id="roster_1" type="result">{}</iq>"#,
                roster_iq
            );
            let _ = write_stanza(writer, &iq).await;
        }

        log::info!("Session established for {}", full_jid);
        return Some(C2sState::SessionEstablished {
            username: username.to_string(),
            resource,
            full_jid,
            stream_id: stream_id.to_string(),
        });
    }
    None
}

async fn handle_established_stanza(
    st: &stanza::Stanza,
    username: &str,
    resource: &str,
    full_jid: &str,
    _stream_id: &str,
    config: &Config,
    router: &Router,
    auth: &AuthManager,
    roster: &RosterManager,
    muc: &MucManager,
    writer: &mpsc::UnboundedSender<String>,
) {
    let domain = &config.server.domain;

    if st.is_message() {
        handle_message(st, username, resource, full_jid, domain, router, roster, muc, writer).await;
    } else if st.is_presence() {
        handle_presence(st, username, resource, full_jid, domain, router, roster, muc).await;
    } else if st.is_iq() {
        handle_iq(st, username, resource, full_jid, domain, config, router, auth, roster, muc, writer).await;
    }
}

async fn handle_message(
    st: &stanza::Stanza,
    _username: &str,
    _resource: &str,
    full_jid: &str,
    domain: &str,
    router: &Router,
    _roster: &RosterManager,
    muc: &MucManager,
    writer: &mpsc::UnboundedSender<String>,
) {
    if let Some(ref to) = st.to {
        let to_bare = to.split('/').next().unwrap_or(to);
        let parts: Vec<&str> = to_bare.split('@').collect();

        if parts.len() == 2 {
            let node = parts[0];
            let to_domain = parts[1];

            if to_domain == domain {
                if muc.get_room(node).await.is_some() {
                    let stanza_xml = st.to_xml_string();
                    let broadcasts = muc.broadcast_to_room(node, &stanza_xml).await;
                    for (occupant_jid, stanza_xml) in broadcasts {
                        if occupant_jid != full_jid {
                            let _ = router.route(full_jid, &occupant_jid, &stanza_xml).await;
                        }
                    }
                    return;
                }

                let stanza_xml = st.to_xml_string();
                match router.route(full_jid, to, &stanza_xml).await {
                    Ok(_) => {}
                    Err(_) => {
                        let error = stanza::build_message(
                            &format!("{}@{}", _username, domain),
                            full_jid,
                            "User is offline",
                            "error",
                        );
                        let _ = write_stanza(writer, &error).await;
                    }
                }
            } else {
                log::info!("Federation message to {} not yet implemented", to_domain);
            }
        }
    }
}

async fn handle_presence(
    st: &stanza::Stanza,
    username: &str,
    resource: &str,
    full_jid: &str,
    domain: &str,
    router: &Router,
    roster: &RosterManager,
    muc: &MucManager,
) {
    let ptype = st.r#type.as_deref().unwrap_or("available");
    let bare_jid = format!("{}@{}", username, domain);

    match ptype {
        "subscribe" => {
            if let Some(ref to) = st.to {
                roster.add_contact(username, to, None).await;
                let ack = stanza::build_presence(&bare_jid, to, Some("subscribed"), None, None);
                let _ = router.route(full_jid, to, &ack).await;
            }
        }
        "unavailable" => {
            roster.remove_resource(full_jid).await;
            let unavailable = stanza::build_presence(&bare_jid, &bare_jid, Some("unavailable"), None, None);
            let _ = router.broadcast(&unavailable, Some(full_jid)).await;
        }
        _ => {
            let show = st.get_child_text("show");
            let status = st.get_child_text("status");
            let priority = st.get_child_text("priority").unwrap_or_default().parse::<i32>().unwrap_or(0);

            roster.set_presence(username, resource, priority, show.clone(), status.clone(), true).await;

            let presence_xml = stanza::build_presence(
                &format!("{}@{}", username, domain),
                &bare_jid,
                None,
                show.as_deref(),
                status.as_deref(),
            );
            let _ = router.broadcast(&presence_xml, Some(full_jid)).await;

            if let Some(ref to) = st.to {
                if let Some(room_part) = to.strip_suffix(&format!("@{}", domain)) {
                    let parts: Vec<&str> = room_part.split('/').collect();
                    if parts.len() == 2 {
                        let room_name = parts[0];
                        let nick = parts[1];
                        if muc.get_room(room_name).await.is_some() {
                            let _ = muc.join_room(room_name, nick, full_jid, None).await;
                            let join_presence = stanza::build_presence(
                                full_jid,
                                &format!("{}@{}", room_name, domain),
                                None,
                                None,
                                None,
                            );
                            let broadcasts = muc.broadcast_to_room(room_name, &join_presence).await;
                            for (occupant_jid, stanza_xml) in &broadcasts {
                                if occupant_jid != full_jid {
                                    let _ = router.route(full_jid, occupant_jid, stanza_xml).await;
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

async fn handle_iq(
    st: &stanza::Stanza,
    username: &str,
    resource: &str,
    full_jid: &str,
    domain: &str,
    config: &Config,
    router: &Router,
    auth: &AuthManager,
    roster: &RosterManager,
    muc: &MucManager,
    writer: &mpsc::UnboundedSender<String>,
) {
    let id = st.id.as_deref().unwrap_or("id");
    let _ = resource;
    let _ = full_jid;
    let _ = router;

    if st.r#type.as_deref() == Some("get") {
        if st.has_child_with_xmlns("query", "jabber:iq:roster") {
            let items = roster.get_roster(username).await;
            let roster_xml = build_roster_iq(&items);
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, roster_xml
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("query", "http://jabber.org/protocol/disco#info") {
            let disco = format!(
                r#"<query xmlns="http://jabber.org/protocol/disco#info">
                    <identity category="server" type="im" name="{}"/>
                    <feature var="jabber:iq:roster"/>
                    <feature var="jabber:iq:register"/>
                    <feature var="urn:ietf:params:xml:ns:xmpp-bind"/>
                    <feature var="urn:ietf:params:xml:ns:xmpp-session"/>
                    <feature var="http://jabber.org/protocol/muc"/>
                    <feature var="jabber:iq:version"/>
                    <feature var="urn:xmpp:ping"/>
                </query>"#,
                config.server.name
            );
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, disco
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("query", "http://jabber.org/protocol/disco#items") {
            let rooms = muc.list_rooms().await;
            let items_xml: String = rooms
                .iter()
                .map(|room| {
                    format!(r#"<item jid="{}@{}" name="{}"/>"#, room, domain, room)
                })
                .collect();
            let disco = format!("<query xmlns='http://jabber.org/protocol/disco#items'>{}</query>", items_xml);
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, disco
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("query", "jabber:iq:version") {
            let version = format!(
                r#"<query xmlns="jabber:iq:version"><name>{}</name><version>0.1.0</version><os>Rust</os></query>"#,
                config.server.name
            );
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, version
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("query", "jabber:iq:last") {
            let last = r#"<query xmlns="jabber:iq:last" seconds="0"/>"#;
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, last
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("ping", "urn:xmpp:ping") {
            let iq = format!(
                r#"<iq id="{}" type="result"><ping xmlns="urn:xmpp:ping"/></iq>"#,
                id
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }

        if st.has_child_with_xmlns("query", "jabber:iq:register") {
            let register = r#"<query xmlns="jabber:iq:register">
                <instructions>Choose a username and password to register</instructions>
                <username/>
                <password/>
            </query>"#;
            let iq = format!(
                r#"<iq id="{}" type="result">{}</iq>"#,
                id, register
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }
    }

    if st.r#type.as_deref() == Some("set") {
        if st.has_child_with_xmlns("query", "jabber:iq:roster") {
            if let Some(query) = st.get_child_by_xmlns("query", "jabber:iq:roster") {
                if let stanza::StanzaChild::Element { children, .. } = query {
                    for child in children {
                        if let stanza::StanzaChild::Element { name, attrs, .. } = child {
                            if name == "item" {
                                if let Some(contact_jid) = attrs.get("jid") {
                                    let contact_name = attrs.get("name").map(|s| s.as_str());
                                    roster.add_contact(username, contact_jid, contact_name).await;

                                    let iq = format!(
                                        r#"<iq id="{}" type="result"><query xmlns="jabber:iq:roster"/></iq>"#,
                                        id
                                    );
                                    let _ = write_stanza(writer, &iq).await;
                                }
                            }
                        }
                    }
                }
            }
            return;
        }

        if st.has_child_with_xmlns("query", "jabber:iq:register") {
            if let Some(query) = st.get_child_by_xmlns("query", "jabber:iq:register") {
                if let stanza::StanzaChild::Element { children, .. } = query {
                    let mut reg_username = None;
                    let mut reg_password = None;
                    for child in children {
                        if let stanza::StanzaChild::Element { name, children, .. } = child {
                            let text = match children.first() {
                                Some(stanza::StanzaChild::Text(t)) => t.clone(),
                                _ => continue,
                            };
                            if name == "username" {
                                reg_username = Some(text);
                            } else if name == "password" {
                                reg_password = Some(text);
                            }
                        }
                    }

                    if let (Some(u), Some(p)) = (reg_username, reg_password) {
                        match auth.register_user(&u, &p).await {
                            Ok(_) => {
                                log::info!("Registered new user: {}", u);
                                let iq = format!(
                                    r#"<iq id="{}" type="result"><query xmlns="jabber:iq:register"/></iq>"#,
                                    id
                                );
                                let _ = write_stanza(writer, &iq).await;
                            }
                            Err(e) => {
                                let err = stanza::build_iq_error(id, "conflict", &format!("User already exists: {}", e));
                                let _ = write_stanza(writer, &err).await;
                            }
                        }
                    }
                    return;
                }
            }
        }

        if st.has_child_with_xmlns("query", "http://jabber.org/protocol/muc#admin") {
            let iq = format!(
                r#"<iq id="{}" type="result"><query xmlns="http://jabber.org/protocol/muc#admin"/></iq>"#,
                id
            );
            let _ = write_stanza(writer, &iq).await;
            return;
        }
    }

    let err = stanza::build_iq_error(id, "feature-not-implemented", "This feature is not implemented");
    let _ = write_stanza(writer, &err).await;
}

fn build_roster_iq(items: &[crate::roster::RosterItem]) -> String {
    let items_xml: String = items
        .iter()
        .map(|item| {
            let sub = "none";
            let name_attr = item
                .name
                .as_ref()
                .map(|n| format!(" name=\"{}\"", n))
                .unwrap_or_default();
            format!(
                r#"<item jid="{}" subscription="{}"{}/>"#,
                item.jid, sub, name_attr
            )
        })
        .collect::<Vec<_>>()
        .join("");

    format!("<query xmlns='jabber:iq:roster'>{}</query>", items_xml)
}
