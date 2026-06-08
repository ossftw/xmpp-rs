use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub server: ServerConfig,
    pub tls: TlsConfig,
    pub storage: StorageConfig,
    pub federation: FederationConfig,
    pub muc: MucConfig,
    pub rate_limit: RateLimitConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    pub domain: String,
    pub name: String,
    pub c2s_addr: SocketAddr,
    pub s2s_addr: SocketAddr,
    pub max_stanza_size: usize,
    pub max_connections: usize,
    pub idle_timeout_secs: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TlsConfig {
    pub cert_path: Option<PathBuf>,
    pub key_path: Option<PathBuf>,
    pub generate_self_signed: bool,
    pub self_signed_days: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    pub data_dir: PathBuf,
    pub users_file: PathBuf,
    pub muc_logs_dir: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FederationConfig {
    pub enabled: bool,
    pub allowed_servers: Vec<String>,
    pub blacklist: Vec<String>,
    pub dialback_secret: String,
    pub s2s_timeout_secs: u64,
    pub max_federation_stanza_size: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MucConfig {
    pub default_room: String,
    pub max_room_occupants: usize,
    pub persistent_rooms: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RateLimitConfig {
    pub stanzas_per_second: u32,
    pub burst_size: u32,
    pub connections_per_ip: u32,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            server: ServerConfig {
                domain: "ossftw.com".to_string(),
                name: "OSS FTW XMPP Server".to_string(),
                c2s_addr: "0.0.0.0:5222".parse().unwrap(),
                s2s_addr: "0.0.0.0:5269".parse().unwrap(),
                max_stanza_size: 100_000,
                max_connections: 10_000,
                idle_timeout_secs: 300,
            },
            tls: TlsConfig {
                cert_path: None,
                key_path: None,
                generate_self_signed: true,
                self_signed_days: 365,
            },
            storage: StorageConfig {
                data_dir: PathBuf::from("/data"),
                users_file: PathBuf::from("/data/users.json"),
                muc_logs_dir: PathBuf::from("/data/muc_logs"),
            },
            federation: FederationConfig {
                enabled: true,
                allowed_servers: vec![],
                blacklist: vec![],
                dialback_secret: "change-me-to-a-random-secret".to_string(),
                s2s_timeout_secs: 30,
                max_federation_stanza_size: 500_000,
            },
            muc: MucConfig {
                default_room: "lobby".to_string(),
                max_room_occupants: 100,
                persistent_rooms: true,
            },
            rate_limit: RateLimitConfig {
                stanzas_per_second: 50,
                burst_size: 100,
                connections_per_ip: 20,
            },
        }
    }
}

impl Config {
    pub fn load(path: &str) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read config file: {}", e))?;
        let config: Config = toml::from_str(&content)
            .map_err(|e| anyhow::anyhow!("Failed to parse config file: {}", e))?;
        Ok(config)
    }

    pub fn save_default(path: &str) -> anyhow::Result<()> {
        let config = Config::default();
        let content = toml::to_string_pretty(&config)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}
