use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct MucOccupant {
    pub nick: String,
    pub jid: String,
}

#[derive(Debug, Clone)]
pub struct MucRoom {
    pub occupants: Vec<MucOccupant>,
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
                occupants: vec![],
                password: None,
            },
        );
        Self {
            rooms: Arc::new(RwLock::new(rooms)),
        }
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
        });

        Ok(())
    }

    pub async fn get_room(&self, room_name: &str) -> Option<MucRoom> {
        let rooms = self.rooms.read().await;
        rooms.get(room_name).cloned()
    }

    pub async fn list_rooms(&self) -> Vec<String> {
        let rooms = self.rooms.read().await;
        rooms.keys().cloned().collect()
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
