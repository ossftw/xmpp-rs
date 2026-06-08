use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

pub type StanzaSender = mpsc::UnboundedSender<String>;

#[derive(Clone)]
pub struct Router {
    clients: Arc<tokio::sync::RwLock<HashMap<String, StanzaSender>>>,
}

impl Router {
    pub fn new() -> Self {
        Self {
            clients: Arc::new(tokio::sync::RwLock::new(HashMap::new())),
        }
    }

    pub async fn register(&self, jid: &str, sender: StanzaSender) {
        let mut clients = self.clients.write().await;
        clients.insert(jid.to_string(), sender);
        log::info!("Registered client: {}", jid);
    }

    pub async fn unregister(&self, jid: &str) {
        let mut clients = self.clients.write().await;
        clients.remove(jid);
        log::info!("Unregistered client: {}", jid);
    }

    pub async fn route(&self, _from: &str, to: &str, stanza: &str) -> Result<bool, String> {
        let clients = self.clients.read().await;

        if let Some(sender) = clients.get(to) {
            return sender.send(stanza.to_string()).map(|_| true).map_err(|e| e.to_string());
        }

        let matching: Vec<&String> = clients.keys()
            .filter(|jid| jid.starts_with(&format!("{}", to)) || {
                let to_bare = to.split('/').next().unwrap_or(to);
                let jid_bare = jid.split('/').next().unwrap_or(jid);
                jid_bare == to_bare
            })
            .collect();

        if let Some(best) = matching.first() {
            if let Some(sender) = clients.get(*best) {
                return sender.send(stanza.to_string()).map(|_| true).map_err(|e| e.to_string());
            }
        }

        Err(format!("No route found for JID: {}", to))
    }

    pub async fn broadcast(&self, stanza: &str, exclude: Option<&str>) -> usize {
        let clients = self.clients.read().await;
        let mut count = 0;
        for (jid, sender) in clients.iter() {
            if Some(jid.as_str()) != exclude {
                if sender.send(stanza.to_string()).is_ok() {
                    count += 1;
                }
            }
        }
        count
    }

    pub async fn online_count(&self) -> usize {
        let clients = self.clients.read().await;
        clients.len()
    }

    pub async fn connected_users(&self) -> Vec<String> {
        let clients = self.clients.read().await;
        clients.keys().cloned().collect()
    }
}
