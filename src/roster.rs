use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct RosterItem {
    pub jid: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PresenceInfo {
    pub resource: String,
    pub priority: i32,
    pub show: Option<String>,
    pub status: Option<String>,
    pub available: bool,
}

#[derive(Clone)]
pub struct RosterManager {
    rosters: Arc<RwLock<HashMap<String, Vec<RosterItem>>>>,
    presence: Arc<RwLock<HashMap<String, Vec<PresenceInfo>>>>,
}

impl RosterManager {
    pub fn new() -> Self {
        Self {
            rosters: Arc::new(RwLock::new(HashMap::new())),
            presence: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn add_contact(&self, username: &str, contact_jid: &str, name: Option<&str>) {
        let mut rosters = self.rosters.write().await;
        let roster = rosters.entry(username.to_string()).or_default();

        if !roster.iter().any(|r| r.jid == contact_jid) {
            roster.push(RosterItem {
                jid: contact_jid.to_string(),
                name: name.map(|s| s.to_string()),
            });
        }
    }

    pub async fn get_roster(&self, username: &str) -> Vec<RosterItem> {
        let rosters = self.rosters.read().await;
        rosters.get(username).cloned().unwrap_or_default()
    }

    pub async fn set_presence(
        &self,
        username: &str,
        resource: &str,
        priority: i32,
        show: Option<String>,
        status: Option<String>,
        available: bool,
    ) {
        let mut presence = self.presence.write().await;
        let user_presence = presence.entry(format!("{}/{}", username, resource)).or_default();

        if let Some(existing) = user_presence.iter_mut().find(|p| p.resource == resource) {
            existing.priority = priority;
            existing.show = show.clone();
            existing.status = status.clone();
            existing.available = available;
        } else {
            user_presence.push(PresenceInfo {
                resource: resource.to_string(),
                priority,
                show,
                status,
                available,
            });
        }
    }

    pub async fn remove_resource(&self, full_jid: &str) {
        let mut presence = self.presence.write().await;
        presence.remove(full_jid);
    }
}
