use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio_rustls::TlsAcceptor;

use crate::config::Config;
use crate::router::Router;
use crate::stanza;

#[derive(Clone)]
pub struct FederationHandler {
    config: Arc<Config>,
    router: Router,
}

impl FederationHandler {
    pub fn new(config: Arc<Config>, router: Router) -> Self {
        Self { config, router }
    }

    pub async fn start(&self, tls_acceptor: Option<TlsAcceptor>) -> anyhow::Result<()> {
        let addr = self.config.server.s2s_addr;
        let listener = tokio::net::TcpListener::bind(addr).await?;
        log::info!("S2S federation listening on {}", addr);

        loop {
            let (stream, peer) = listener.accept().await?;
            let config = self.config.clone();
            let router = self.router.clone();
            let tls = tls_acceptor.clone();
            let domain = config.server.domain.clone();

            tokio::spawn(async move {
                log::info!("S2S connection from {}", peer);

                if let Some(acceptor) = tls {
                    match acceptor.accept(stream).await {
                        Ok(tls_stream) => {
                            handle_s2s_connection(tls_stream, domain, router).await;
                        }
                        Err(e) => {
                            log::error!("S2S TLS error from {}: {}", peer, e);
                        }
                    }
                } else {
                    handle_s2s_connection(stream, domain, router).await;
                }
            });
        }
    }
}

async fn handle_s2s_connection(
    stream: impl tokio::io::AsyncRead + tokio::io::AsyncWrite + Unpin + Send + 'static,
    domain: String,
    router: Router,
) {
    let (reader_half, mut writer_half) = tokio::io::split(stream);
    let mut reader = BufReader::new(reader_half);
    let mut buf = String::new();
    let mut remote_domain = String::new();

    loop {
        buf.clear();
        match reader.read_line(&mut buf).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {
                let trimmed = buf.trim();
                if trimmed.is_empty() {
                    continue;
                }

                match stanza::parse_stream_element(trimmed) {
                    stanza::ParseResult::StreamOpen { xmlns: _, to, from, id: _, version: _ } => {
                        if let Some(ref from_domain) = from {
                            remote_domain = from_domain.clone();
                        }
                        if let Some(ref to_domain) = to {
                            if to_domain != &domain {
                                log::warn!("S2S connection to wrong domain: {} != {}", to_domain, domain);
                                let _ = writer_half.write_all(b"</stream:stream>").await;
                                break;
                            }
                        }

                        let stream_id = uuid::Uuid::new_v4().to_string();
                        let response = format!(
                            "<?xml version='1.0'?><stream:stream xmlns='{}' xmlns:stream='http://etherx.jabber.org/streams' id='{}' from='{}' version='1.0'/>",
                            "jabber:server", stream_id, domain
                        );

                        if let Err(e) = writer_half.write_all(response.as_bytes()).await {
                            log::error!("S2S send error: {}", e);
                            break;
                        }
                        if let Err(e) = writer_half.flush().await {
                            log::error!("S2S flush error: {}", e);
                            break;
                        }
                    }
                    stanza::ParseResult::Stanza(st) => {
                        if let Some(ref to) = st.to {
                            let stanza_xml = st.to_xml_string();
                            match router.route(&remote_domain, to, &stanza_xml).await {
                                Ok(delivered) => {
                                    if !delivered {
                                        log::warn!("S2S stanza undeliverable to {}", to);
                                    }
                                }
                                Err(e) => {
                                    log::warn!("S2S route error for {}: {}", to, e);
                                }
                            }
                        }
                    }
                    stanza::ParseResult::StreamClose => break,
                    stanza::ParseResult::Error(e) => {
                        log::warn!("S2S parse error: {}", e);
                        break;
                    }
                    stanza::ParseResult::Incomplete => {}
                }
            }
        }
    }

    log::info!("S2S connection from {} closed", remote_domain);
}
