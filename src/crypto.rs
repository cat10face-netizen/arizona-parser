//! Шифрование cookies пользователей (AES-256-GCM).
//! Ключ хранится в secret.key (создаётся автоматически, права 600).
use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{Aes256Gcm, Nonce};
use aes_gcm::aead::rand_core::RngCore;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use std::fs;
use std::path::Path;

const KEY_FILE: &str = "secret.key";
const NONCE_LEN: usize = 12;

/// Читает ключ или создаёт новый.
pub fn load_or_create_key() -> anyhow::Result<[u8; 32]> {
    if Path::new(KEY_FILE).exists() {
        let raw = fs::read_to_string(KEY_FILE)?;
        let bytes = B64.decode(raw.trim())?;
        if bytes.len() != 32 {
            anyhow::bail!("secret.key повреждён (ожидается 32 байта)");
        }
        let mut key = [0u8; 32];
        key.copy_from_slice(&bytes);
        return Ok(key);
    }
    let mut key = [0u8; 32];
    OsRng.fill_bytes(&mut key);
    fs::write(KEY_FILE, B64.encode(key))?;
    // на unix ограничим права
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(KEY_FILE, fs::Permissions::from_mode(0o600));
    }
    log::warn!("Создан новый secret.key  храни его отдельно, без него cookies не расшифровать!");
    Ok(key)
}

/// Шифрует строку -> base64(nonce || ciphertext).
pub fn encrypt(key: &[u8; 32], plaintext: &str) -> anyhow::Result<String> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    let mut nonce_bytes = [0u8; NONCE_LEN];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ct = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;
    let mut out = Vec::with_capacity(NONCE_LEN + ct.len());
    out.extend_from_slice(&nonce_bytes);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

/// Расшифровывает base64(nonce || ciphertext).
pub fn decrypt(key: &[u8; 32], encoded: &str) -> anyhow::Result<String> {
    let data = B64.decode(encoded)?;
    if data.len() <= NONCE_LEN {
        anyhow::bail!("ciphertext слишком короткий");
    }
    let (nonce_bytes, ct) = data.split_at(NONCE_LEN);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let pt = cipher
        .decrypt(nonce, ct)
        .map_err(|_| anyhow::anyhow!("не удалось расшифровать (неверный ключ?)"))?;
    Ok(String::from_utf8(pt)?)
}
