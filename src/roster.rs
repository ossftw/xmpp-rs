use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct RosterItem {
    pub jid: String,
    pub name: Option<String>,
    pub subscription: SubscriptionState,
    pub groups: Vec<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SubscriptionState {
    None,
    To,
    From,
    Both,
    PendingOut,
    PendingIn,
}

#[derive(Debug, Clone)]
pub struct PresenceInfo {
    pub jid: String,
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
                subscription: SubscriptionState::None,
                groups: vec![],
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

        let jid = format!("{}@{}", username, "ossftw.com");
        if let Some(existing) = user_presence.iter_mut().find(|p| p.resource == resource) {
            existing.priority = priority;
            existing.show = show.clone();
            existing.status = status.clone();
            existing.available = available;
        } else {
            user_presence.push(PresenceInfo {
                jid: jid.clone(),
                resource: resource.to_string(),
                priority,
                show,
                status,
                available,
            });
        }
    }

    pub async fn get_user_presence(&self, bare_jid: &str) -> Vec<PresenceInfo> {
        let presence = self.presence.read().await;
        presence.values()
            .flatten()
            .filter(|p| p.jid == bare_jid)
            .cloned()
            .collect()
    }

    pub async fn remove_resource(&self, full_jid: &str) {
        let mut presence = self.presence.write().await;
        presence.remove(full_jid);
    }

    pub async fn get_online_users(&self) -> Vec<String> {
        let presence = self.presence.read().await;
        let mut users: HashSet<String> = HashSet::new();
        for resources in presence.values() {
            for p in resources {
                if p.available {
                    users.insert(p.jid.clone());
                }
            }
        }
        users.into_iter().collect()
    }

    pub async fn is_user_online(&self, bare_jid: &str) -> bool {
        let presence = self.presence.read().await;
        presence.values()
            .flatten()
            .any(|p| p.jid == bare_jid && p.available)
    }

    pub async fn get_best_resource(&self, bare_jid: &str) -> Option<String> {
        let presence = self.presence.read().await;
        let best = presence.values()
            .flatten()
            .filter(|p| p.jid == bare_jid && p.available)
            .max_by_key(|p| p.priority);
        best.map(|p| p.resource.clone())
    }
}
