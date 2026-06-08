mod auth;
mod c2s;
mod config;
mod muc;
mod roster;
mod router;
mod s2s;
mod stanza;
mod tls;

use std::sync::Arc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .init();

    let args: Vec<String> = std::env::args().collect();

    let config_path = std::env::var("CONFIG_PATH").unwrap_or_else(|_| "config.toml".to_string());
    let config = if std::path::Path::new(&config_path).exists() {
        config::Config::load(&config_path)?
    } else {
        log::warn!("No config.toml found, creating default");
        config::Config::save_default(&config_path)?;
        config::Config::load(&config_path)?
    };

    if args.len() >= 3 && args[1] == "register" {
        let username = &args[2];
        let password = if args.len() >= 4 { &args[3] } else { "changeme123" };
        let users_path = config.storage.users_file.to_str().unwrap_or("/tmp/xmpp-data/users.json");
        let auth = auth::AuthManager::new();
        auth.load_users(users_path).await?;
        if auth.user_exists(username).await {
            eprintln!("User '{}' already exists", username);
            std::process::exit(1);
        }
        auth.register_user(username, password).await?;
        auth.save_users(users_path).await?;
        println!("User '{}' registered successfully", username);
        return Ok(());
    }

    if args.len() >= 2 && args[1] == "help" {
        println!("Usage:");
        println!("  xmpp-server                    Start the server");
        println!("  xmpp-server register USER [PASS]  Register a user (default pass: changeme123)");
        println!("  xmpp-server list               List registered users");
        return Ok(());
    }

    if args.len() >= 2 && args[1] == "list" {
        let users_path = config.storage.users_file.to_str().unwrap_or("/tmp/xmpp-data/users.json");
        let auth = auth::AuthManager::new();
        auth.load_users(users_path).await?;
        let users = auth.list_users().await;
        if users.is_empty() {
            println!("No users registered");
        } else {
            println!("Registered users:");
            for u in users {
                println!("  {}", u);
            }
        }
        return Ok(());
    }

    log::info!("Starting XMPP Server for {}", config.server.domain);

    std::fs::create_dir_all(&config.storage.data_dir)?;
    std::fs::create_dir_all(&config.storage.muc_logs_dir)?;

    let router = router::Router::new();
    let auth = auth::AuthManager::new();
    let roster = roster::RosterManager::new();
    let muc = muc::MucManager::new();

    auth.load_users(
        config.storage.users_file.to_str().unwrap_or("/data/users.json"),
    )
    .await?;
    log::info!("Loaded users");

    let users = auth.list_users().await;
    if users.is_empty() {
        auth.register_user("admin", "changeme123").await?;
        log::warn!("Created default admin user: admin / changeme123");
        log::warn!("CHANGE THE DEFAULT PASSWORD IMMEDIATELY!");
    }

    let config = Arc::new(config);

    let tls_acceptor = tls::build_tls_acceptor(&config)?;
    let tls_acceptor_c2s = tls_acceptor.clone();
    let tls_acceptor_s2s = tls_acceptor.clone();

    let c2s_handler = c2s::C2sHandler::new(config.clone(), router.clone(), auth.clone(), roster.clone(), muc.clone());
    let c2s_handle = tokio::spawn(async move {
        if let Err(e) = c2s_handler.start(tls_acceptor_c2s).await {
            log::error!("C2S handler error: {}", e);
        }
    });

    let s2s_handle = if config.federation.enabled {
        let s2s_handler = s2s::FederationHandler::new(config.clone(), router.clone());
        Some(tokio::spawn(async move {
            if let Err(e) = s2s_handler.start(tls_acceptor_s2s).await {
                log::error!("S2S handler error: {}", e);
            }
        }))
    } else {
        None
    };

    let auth_save = auth.clone();
    let users_path = config.storage.users_file.to_str().unwrap_or("/data/users.json").to_string();
    let save_handle = tokio::spawn(async move {
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
            if let Err(e) = auth_save.save_users(&users_path).await {
                log::error!("Failed to save users: {}", e);
            }
        }
    });

    log::info!("XMPP Server is running!");
    log::info!("  C2S: {}", config.server.c2s_addr);
    log::info!("  S2S: {}", config.server.s2s_addr);
    log::info!("  Domain: {}", config.server.domain);
    log::info!("  Federation: {}", if config.federation.enabled { "enabled" } else { "disabled" });
    log::info!("  TLS: {}", if tls_acceptor.is_some() { "configured" } else { "disabled" });

    tokio::select! {
        _ = c2s_handle => {},
        _ = save_handle => {},
    }

    if let Some(handle) = s2s_handle {
        let _ = handle.await;
    }

    Ok(())
}
