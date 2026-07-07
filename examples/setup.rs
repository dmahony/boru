use std::{
    env,
    path::{Path, PathBuf},
    str::FromStr,
};

use iroh::{endpoint::presets, protocol::Router, Endpoint, SecretKey};
#[cfg(feature = "tor-transport")]
use iroh_gossip::tor_transport::TorTransport;
use iroh_gossip::{net::Gossip, ALPN};
use n0_error::{Result, StdResultExt};

fn get_data_dir() -> PathBuf {
    if let Ok(val) = env::var("IROH_GOSSIP_CHAT_DATA_DIR") {
        return PathBuf::from(val);
    }
    if let Some(val) = env::var_os("XDG_DATA_HOME") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    if let Some(val) = env::var_os("HOME") {
        return PathBuf::from(val)
            .join(".local")
            .join("share")
            .join("iroh-gossip-chat");
    }
    if let Some(val) = env::var_os("LOCALAPPDATA") {
        return PathBuf::from(val).join("iroh-gossip-chat");
    }
    // Fallback
    std::env::current_dir()
        .unwrap_or_default()
        .join(".iroh-gossip-chat")
}

fn load_or_generate_secret_key() -> Result<(SecretKey, PathBuf)> {
    load_or_generate_secret_key_at(&get_data_dir())
}

fn load_or_generate_secret_key_at(data_dir: &Path) -> Result<(SecretKey, PathBuf)> {
    let key_path = data_dir.join("secret_key.txt");

    if key_path.exists() {
        let key_str =
            std::fs::read_to_string(&key_path).std_context("failed to read secret key file")?;
        let key_str = key_str.trim();
        let key =
            SecretKey::from_str(key_str).std_context("failed to parse secret key from file")?;
        println!("> loaded identity from: {}", key_path.display());
        Ok((key, key_path))
    } else {
        let key = SecretKey::generate();
        let key_str = data_encoding::HEXLOWER.encode(&key.to_bytes());
        std::fs::create_dir_all(data_dir).std_context("failed to create data directory")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(data_dir, std::fs::Permissions::from_mode(0o700));
        }

        std::fs::write(&key_path, format!("{key_str}\n"))
            .std_context("failed to write secret key file")?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
        }

        println!("> generated new identity, saved to: {}", key_path.display());
        Ok((key, key_path))
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // load or generate persistent secret key
    let (secret_key, key_path) = load_or_generate_secret_key()?;
    println!("> our public key: {}", secret_key.public());
    println!("> identity file: {}", key_path.display());

    // create an iroh endpoint with the persistent identity
    let endpoint = Endpoint::builder(presets::N0)
        .secret_key(secret_key.clone())
        .bind()
        .await?;

    println!("> our endpoint id: {}", endpoint.id());

    // build gossip protocol
    let gossip = Gossip::builder().spawn(endpoint.clone());

    // setup router
    let router = Router::builder(endpoint.clone())
        .accept(ALPN, gossip.clone())
        .spawn();

    // do fun stuff with the gossip protocol
    router.shutdown().await.std_context("shutdown router")?;
    Ok(())
}
