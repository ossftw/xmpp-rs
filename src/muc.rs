use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct MucOccupant {
    pub nick: String,
    pub jid: String,
    pub role: String,
    pub affiliation: String,
}

#[derive(Debug, Clone)]
pub struct MucRoom {
    pub name: String,
    pub title: Option<String>,
    pub occupants: Vec<MucOccupant>,
    pub subject: Option<String>,
    pub persistent: bool,
    pub password: Option<String>,
}

#[derive(Clone)]
pub struct MucManager {
    rooms: Arc<RwLock<HashMap<String, MucRoom>>>,
}

impl MucManager {
    pub fn new() -> Self {
        let mut rooms = HashMap::new();
        rooms.insert(
            "lobby".to_string(),
            MucRoom {
                name: "lobby".to_string(),
                title: Some("General Lobby".to_string()),
                occupants: vec![],
                subject: None,
                persistent: true,
                password: None,
            },
        );
        Self {
            rooms: Arc::new(RwLock::new(rooms)),
        }
    }

    pub async fn create_room(&self, name: &str, title: Option<&str>, password: Option<&str>) -> anyhow::Result<()> {
        let mut rooms = self.rooms.write().await;
        if rooms.contains_key(name) {
            return Err(anyhow::anyhow!("Room already exists"));
        }
        rooms.insert(
            name.to_string(),
            MucRoom {
                name: name.to_string(),
                title: title.map(|s| s.to_string()),
                occupants: vec![],
                subject: None,
                persistent: false,
                password: password.map(|s| s.to_string()),
            },
        );
        Ok(())
    }

    pub async fn join_room(&self, room_name: &str, nick: &str, jid: &str, password: Option<&str>) -> anyhow::Result<()> {
        let mut rooms = self.rooms.write().await;
        let room = rooms.get_mut(room_name).ok_or_else(|| anyhow::anyhow!("Room not found"))?;

        if let Some(ref pw) = room.password {
            if password != Some(pw.as_str()) && password != Some(pw.as_str()) {
                return Err(anyhow::anyhow!("Incorrect password"));
            }
        }

        if room.occupants.iter().any(|o| o.nick == nick) {
            return Err(anyhow::anyhow!("Nick already taken"));
        }

        room.occupants.push(MucOccupant {
            nick: nick.to_string(),
            jid: jid.to_string(),
            role: "participant".to_string(),
            affiliation: "none".to_string(),
        });

        Ok(())
    }

    pub async fn leave_room(&self, room_name: &str, nick: &str) {
        let mut rooms = self.rooms.write().await;
        if let Some(room) = rooms.get_mut(room_name) {
            room.occupants.retain(|o| o.nick != nick);
        }
    }

    pub async fn get_room(&self, room_name: &str) -> Option<MucRoom> {
        let rooms = self.rooms.read().await;
        rooms.get(room_name).cloned()
    }

    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms.keys().cloned().collect()
    }

    pub async fn get_occupant_jids(&self, room_name: &str) -> Vec<String> {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            room.occupants.iter().map(|o| o.jid.clone()).collect()
        } else {
            vec![]
        }
    }

    pub async fn get_occupant_nicks(&self, room_name: &str) -> Vec<String> {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            room.occupants.iter().map(|o| o.nick.clone()).collect()
        } else {
            vec![]
        }
    }

    pub async fn broadcast_to_room(&self, room_name: &str, stanza: &str) -> Vec<(String, String)> {
        let rooms = self.rooms.read().await;
        if let Some(room) = rooms.get(room_name) {
            room.occupants.iter().map(|o| (o.jid.clone(), stanza.to_string())).collect()
        } else {
            vec![]
        }
    }
}
