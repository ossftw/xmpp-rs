use base64::{Engine as _, engine::general_purpose};
use hmac::{Hmac, Mac};
use rand::Rng;
use serde::{Deserialize, Serialize};
use sha1::Digest as Sha1Digest;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;

type HmacSha1 = Hmac<sha1::Sha1>;

#[derive(Debug, Clone)]
pub struct User {
    pub username: String,
    pub password_hash: String,
    pub salt: String,
    pub iterations: u32,
}

#[derive(Debug, Clone)]
pub struct ScramCredentials {
    pub salt: String,
    pub iterations: u32,
    pub stored_key: String,
    pub server_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AuthMechanism {
    Plain,
    ScramSha1,
}

#[derive(Debug)]
pub struct AuthState {
    pub mechanism: AuthMechanism,
    pub username: Option<String>,
    pub step: u32,
    pub client_nonce: Option<String>,
    pub server_nonce: Option<String>,
    pub auth_message: Option<String>,
    pub server_first_msg: Option<String>,
}

#[derive(Clone)]
pub struct AuthManager {
    users: Arc<RwLock<HashMap<String, UserData>>>,
    pending_auth: Arc<RwLock<HashMap<String, AuthState>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserData {
    pub username: String,
    pub password: String,
    pub password_hash: String,
    pub salt: String,
    pub stored_key: String,
    pub server_key: String,
}

impl AuthManager {
    pub fn new() -> Self {
        Self {
            users: Arc::new(RwLock::new(HashMap::new())),
            pending_auth: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn register_user(&self, username: &str, password: &str) -> anyhow::Result<()> {
        let salt = generate_salt();
        let iterations = 4096;
        let salted_password = hi(password.as_bytes(), salt.as_bytes(), iterations);
        let client_key = hmac_sha1(&salted_password, b"Client Key");
        let stored_key = sha1_hex(&client_key);
        let server_key = hmac_sha1(&salted_password, b"Server Key");
        let server_key_hex = hex::encode(server_key);

        let user = UserData {
            username: username.to_string(),
            password: password.to_string(),
            password_hash: sha256_hex(password),
            salt: salt.clone(),
            stored_key,
            server_key: server_key_hex,
        };

        let mut users = self.users.write().await;
        if users.contains_key(username) {
            return Err(anyhow::anyhow!("User already exists"));
        }
        users.insert(username.to_string(), user);
        Ok(())
    }

    pub async fn authenticate_plain(&self, _stream_id: &str, auth_data: &str) -> Result<String, String> {
        let decoded = general_purpose::STANDARD.decode(auth_data).map_err(|_| "invalid-base64".to_string())?;
        let parts: Vec<&[u8]> = decoded.split(|b| *b == 0).collect();
        if parts.len() < 3 {
            return Err("bad-protocol".to_string());
        }
        let username = String::from_utf8_lossy(parts[1]).to_string();
        let password = String::from_utf8_lossy(parts[2]).to_string();

        let users = self.users.read().await;
        let user = users.get(&username).ok_or("not-authorized")?;

        if user.password == password || user.password_hash == sha256_hex(&password) {
            Ok(username)
        } else {
            Err("not-authorized".to_string())
        }
    }

    pub async fn start_scram_sha1(&self, stream_id: &str, initial_data: &str) -> Result<(String, String, String), String> {
        let decoded = general_purpose::STANDARD.decode(initial_data).map_err(|_| "invalid-base64".to_string())?;
        let client_first = String::from_utf8_lossy(&decoded).to_string();

        let username = client_first
            .split(',')
            .find_map(|s| s.strip_prefix("n="))
            .map(|s| s.to_string())
            .ok_or("invalid-username")?;

        let client_nonce = client_first
            .split(',')
            .find_map(|s| s.strip_prefix("r="))
            .map(|s| s.to_string())
            .ok_or("invalid-nonce")?;

        let server_nonce = format!("{}{}", client_nonce, generate_nonce());
        let users = self.users.read().await;
        let user = users.get(&username).ok_or("not-authorized")?;

        let server_first = format!(
            "r={},s={},i={}",
            server_nonce, user.salt, 4096
        );

        let state = AuthState {
            mechanism: AuthMechanism::ScramSha1,
            username: Some(username.clone()),
            step: 1,
            client_nonce: Some(client_nonce),
            server_nonce: Some(server_nonce),
            auth_message: None,
            server_first_msg: Some(server_first.clone()),
        };

        let mut pending = self.pending_auth.write().await;
        pending.insert(stream_id.to_string(), state);

        let challenge = general_purpose::STANDARD.encode(server_first.as_bytes());
        Ok((challenge, username, server_first))
    }

    pub async fn finish_scram_sha1(&self, stream_id: &str, response: &str) -> Result<String, String> {
        let decoded = general_purpose::STANDARD.decode(response).map_err(|_| "invalid-base64".to_string())?;
        let client_final = String::from_utf8_lossy(&decoded).to_string();

        let mut pending = self.pending_auth.write().await;
        let state = pending.get(stream_id).ok_or("not-authorized")?;

        let username = state.username.as_ref().ok_or("not-authorized")?.clone();
        let server_first = state.server_first_msg.as_ref().ok_or("not-authorized")?.clone();
        let _client_nonce = state.client_nonce.as_ref().ok_or("not-authorized")?.clone();

        let _gs2_header = "n,,";
        let _channel_binding = client_final.split(',').find(|s| s.starts_with("c=")).unwrap_or("");
        let _client_final_r_nonce = client_final.split(',').find(|s| s.starts_with("r=")).unwrap_or("");

        let (stored_key, server_key_str) = {
            let users = self.users.read().await;
            let user = users.get(&username).ok_or("not-authorized")?;
            (user.stored_key.clone(), user.server_key.clone())
        };

        let expected_proof = client_final
            .split(',')
            .find_map(|s| s.strip_prefix("p="))
            .ok_or("invalid-proof")?;

        let client_final_without_proof = client_final
            .split(',')
            .filter(|s| !s.starts_with("p="))
            .collect::<Vec<_>>()
            .join(",");

        let auth_msg = format!("{},{}", server_first, client_final_without_proof);

        let client_key = hex::decode(&stored_key).map_err(|_| "invalid-key")?;

        let _client_signature = hmac_sha1(&client_key, auth_msg.as_bytes());
        let client_proof_bytes = hex_decode(expected_proof).map_err(|_| "invalid-proof-encoding")?;
        let stored_client_key = xor_bytes(&client_key, &client_proof_bytes);

        if hex::encode(sha1_raw(&stored_client_key)) != stored_key.replace('-', "") {
            return Err("not-authorized".to_string());
        }

        let server_key = hex::decode(&server_key_str).map_err(|_| "invalid-server-key")?;
        let server_signature = hmac_sha1(&server_key, auth_msg.as_bytes());
        let server_signature_b64 = general_purpose::STANDARD.encode(&server_signature);

        pending.remove(stream_id);

        let success_data = general_purpose::STANDARD.encode(format!("v={}", server_signature_b64));
        Ok(success_data)
    }

    pub async fn get_username_for_stream(&self, stream_id: &str) -> Option<String> {
        let pending = self.pending_auth.read().await;
        pending.get(stream_id).and_then(|s| s.username.clone())
    }

    pub async fn remove_auth_state(&self, stream_id: &str) {
        let mut pending = self.pending_auth.write().await;
        pending.remove(stream_id);
    }

    pub async fn user_exists(&self, username: &str) -> bool {
        let users = self.users.read().await;
        users.contains_key(username)
    }

    pub async fn get_user(&self, username: &str) -> Option<UserData> {
        let users = self.users.read().await;
        users.get(username).cloned()
    }

    pub async fn save_users(&self, path: &str) -> anyhow::Result<()> {
        let users = self.users.read().await;
        let users_vec: Vec<&UserData> = users.values().collect();
        let json = serde_json::to_string_pretty(&users_vec)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    pub async fn load_users(&self, path: &str) -> anyhow::Result<()> {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return Ok(()),
        };
        let users_vec: Vec<UserData> = serde_json::from_str(&content)?;
        let mut users = self.users.write().await;
        for user in users_vec {
            users.insert(user.username.clone(), user);
        }
        Ok(())
    }

    pub async fn list_users(&self) -> Vec<String> {
        let users = self.users.read().await;
        users.keys().cloned().collect()
    }
}

fn client_first_part(client_nonce: &str) -> String {
    format!("n,,n=,r={}", client_nonce)
}

fn generate_salt() -> String {
    let mut rng = rand::thread_rng();
    let salt: [u8; 16] = rng.gen();
    general_purpose::STANDARD.encode(salt)
}

fn generate_nonce() -> String {
    let mut rng = rand::thread_rng();
    let nonce: [u8; 16] = rng.gen();
    hex::encode(nonce)
}

fn hi(password: &[u8], salt: &[u8], iterations: u32) -> Vec<u8> {
    let mut mac = HmacSha1::new_from_slice(password).expect("HMAC can take key of any size");
    mac.update(salt);
    mac.update(&[0, 0, 0, 1]);
    let mut u = mac.finalize().into_bytes().to_vec();

    let mut result = u.clone();
    for _ in 1..iterations {
        let mut mac = HmacSha1::new_from_slice(password).expect("HMAC can take key of any size");
        mac.update(&u);
        u = mac.finalize().into_bytes().to_vec();
        for (a, b) in result.iter_mut().zip(u.iter()) {
            *a ^= b;
        }
    }
    result
}

fn hmac_sha1(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha1::new_from_slice(key).expect("HMAC can take key of any size");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

fn sha1_raw(data: &[u8]) -> Vec<u8> {
    let mut hasher = sha1::Sha1::new();
    hasher.update(data);
    hasher.finalize().to_vec()
}

fn sha1_hex(data: &[u8]) -> String {
    hex::encode(sha1_raw(data))
}

fn sha256_hex(data: &str) -> String {
    let mut hasher = sha2::Sha256::new();
    hasher.update(data.as_bytes());
    hex::encode(hasher.finalize())
}

fn xor_bytes(a: &[u8], b: &[u8]) -> Vec<u8> {
    a.iter().zip(b.iter()).map(|(x, y)| x ^ y).collect()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    let decoded = general_purpose::STANDARD.decode(s).map_err(|_| ())?;
    Ok(decoded)
}
