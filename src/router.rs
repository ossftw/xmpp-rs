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
        let sender = {
            let clients = self.clients.read().await;

            if let Some(sender) = clients.get(to) {
                Some(sender.clone())
            } else {
                let best = clients.keys()
                    .filter(|jid| {
                        let to_bare = to.split('/').next().unwrap_or(to);
                        let jid_bare = jid.split('/').next().unwrap_or(jid);
                        jid_bare == to_bare
                    })
                    .next()
                    .and_then(|jid| clients.get(jid));

                best.cloned()
            }
        };

        match sender {
            Some(s) => s.send(stanza.to_string()).map(|_| true).map_err(|e| e.to_string()),
            None => Err(format!("No route found for JID: {}", to)),
        }
    }

    pub async fn broadcast(&self, stanza: &str, exclude: Option<&str>) -> usize {
        let targets = {
            let clients = self.clients.read().await;
            clients.iter()
                .filter(|(jid, _)| Some(jid.as_str()) != exclude)
                .map(|(jid, sender)| (jid.clone(), sender.clone()))
                .collect::<Vec<_>>()
        };

        let mut count = 0;
        for (_, sender) in targets {
            if sender.send(stanza.to_string()).is_ok() {
                count += 1;
            }
        }
        count
    }

}
