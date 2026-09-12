use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use chacha20poly1305::aead::{Aead, Generate, KeyInit};
use chacha20poly1305::{Key, XChaCha20Poly1305, XNonce};
use serde::{Deserialize, Serialize};

use crate::{ConfigError, ConfigResult};

const SECRETS_FILE: &str = "secrets.json";
const KEY_FILE: &str = "secret.key";
const SECRETS_VERSION: u32 = 1;
const KEY_LEN: usize = 32;
const NONCE_LEN: usize = 24;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SecretsFile {
    version: u32,
    #[serde(default)]
    secrets: BTreeMap<String, String>,
}

pub(crate) fn load(root: &Path) -> ConfigResult<BTreeMap<String, String>> {
    let path = root.join(SECRETS_FILE);
    if !path.exists() {
        return Ok(BTreeMap::new());
    }

    let contents = fs::read_to_string(path)?;
    let file: SecretsFile = serde_json::from_str(&contents)?;
    let cipher = cipher_for(root)?;

    let mut secrets = BTreeMap::new();
    for (id, encoded) in file.secrets {
        if let Some(password) = decrypt(&cipher, &encoded) {
            secrets.insert(id, password);
        }
    }

    Ok(secrets)
}

pub(crate) fn save(root: &Path, secrets: &BTreeMap<String, String>) -> ConfigResult<()> {
    fs::create_dir_all(root)?;
    let cipher = cipher_for(root)?;

    let mut encoded = BTreeMap::new();
    for (id, password) in secrets {
        encoded.insert(id.clone(), encrypt(&cipher, password)?);
    }

    let file = SecretsFile {
        version: SECRETS_VERSION,
        secrets: encoded,
    };
    let contents = serde_json::to_string_pretty(&file)?;
    fs::write(root.join(SECRETS_FILE), contents)?;
    Ok(())
}

fn cipher_for(root: &Path) -> ConfigResult<XChaCha20Poly1305> {
    let key = load_or_create_key(root)?;
    XChaCha20Poly1305::new_from_slice(&key).map_err(|_| ConfigError::Crypto)
}

fn load_or_create_key(root: &Path) -> ConfigResult<Vec<u8>> {
    let path = root.join(KEY_FILE);
    if path.exists() {
        let bytes = fs::read(&path)?;
        if bytes.len() != KEY_LEN {
            return Err(ConfigError::Crypto);
        }
        return Ok(bytes);
    }

    fs::create_dir_all(root)?;
    let key = Key::generate();
    let bytes = key.as_slice().to_vec();
    fs::write(&path, &bytes)?;
    restrict_permissions(&path);
    Ok(bytes)
}

fn encrypt(cipher: &XChaCha20Poly1305, plaintext: &str) -> ConfigResult<String> {
    let nonce = XNonce::generate();
    let mut blob = nonce.as_slice().to_vec();
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| ConfigError::Crypto)?;
    blob.extend_from_slice(&ciphertext);
    Ok(STANDARD.encode(blob))
}

fn decrypt(cipher: &XChaCha20Poly1305, encoded: &str) -> Option<String> {
    let blob = STANDARD.decode(encoded).ok()?;
    if blob.len() <= NONCE_LEN {
        return None;
    }

    let nonce = XNonce::try_from(&blob[..NONCE_LEN]).ok()?;
    let plaintext = cipher.decrypt(&nonce, &blob[NONCE_LEN..]).ok()?;
    String::from_utf8(plaintext).ok()
}

#[cfg(unix)]
fn restrict_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt as _;

    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_secrets_and_encrypts_plaintext() {
        let root = std::env::temp_dir().join(format!("navidog-secrets-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        let mut secrets = BTreeMap::new();
        secrets.insert("conn-1".to_string(), "s3cr3t".to_string());

        save(&root, &secrets).expect("save");
        let raw = fs::read_to_string(root.join(SECRETS_FILE)).expect("read");
        assert!(!raw.contains("s3cr3t"));

        let loaded = load(&root).expect("load");
        assert_eq!(loaded, secrets);

        let _ = fs::remove_dir_all(&root);
    }
}
