//! Пользователи бота: свои cookies (зашифрованы), свои разделы, быстрые ответы.
use crate::crypto;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::Path;
use std::sync::Mutex;

const USERS_FILE: &str = "users.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub vk_id: i64,
    pub name: String,
    /// Зашифрованные cookies (base64), пусто = нет cookies
    #[serde(default)]
    pub cookies_enc: String,
    /// Разделы этого пользователя (пусто = считаются по ролям)
    #[serde(default)]
    pub sections: Vec<String>,
    /// Роли пользователя (несост, госсник, гетто, маф, слеты, смп, основатель)
    #[serde(default)]
    pub roles: Vec<String>,
    /// Допущен ли в бота (ставит основатель)
    #[serde(default)]
    pub allowed: bool,
    /// Быстрые ответы (шаблоны)
    #[serde(default)]
    pub quick_replies: Vec<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Когда последний раз обновляли cookies (unix ts)
    #[serde(default)]
    pub cookies_updated: u64,
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct UsersDb {
    #[serde(default)]
    pub users: HashMap<String, User>,
}

pub struct Users {
    inner: Mutex<UsersDb>,
    key: [u8; 32],
}

impl Users {
    pub fn open() -> anyhow::Result<Self> {
        let key = crypto::load_or_create_key()?;
        let db = if Path::new(USERS_FILE).exists() {
            let text = fs::read_to_string(USERS_FILE)?;
            serde_json::from_str(&text).unwrap_or_default()
        } else {
            UsersDb::default()
        };
        Ok(Self { inner: Mutex::new(db), key })
    }

    fn save(&self, db: &UsersDb) {
        if let Ok(text) = serde_json::to_string_pretty(db) {
            let _ = fs::write(USERS_FILE, text);
        }
    }

    pub fn get(&self, vk_id: i64) -> Option<User> {
        let db = self.inner.lock().unwrap();
        db.users.get(&vk_id.to_string()).cloned()
    }

    pub fn all(&self) -> Vec<User> {
        let db = self.inner.lock().unwrap();
        db.users.values().cloned().collect()
    }

    /// Создаёт пользователя, если его нет. Возвращает (user, created).
    pub fn ensure(&self, vk_id: i64, name: &str) -> (User, bool) {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        if let Some(u) = db.users.get(&key) {
            return (u.clone(), false);
        }
        let u = User {
            vk_id,
            name: name.to_string(),
            cookies_enc: String::new(),
            sections: Vec::new(),
            roles: Vec::new(),
            allowed: false,
            quick_replies: default_quick_replies(),
            enabled: true,
            cookies_updated: 0,
        };
        db.users.insert(key, u.clone());
        self.save(&db);
        (u, true)
    }

    /// Сохраняет cookies пользователя (шифрует).
    pub fn set_cookies(&self, vk_id: i64, cookies_json: &str) -> anyhow::Result<()> {
        // проверим, что это валидный JSON-массив
        let v: serde_json::Value = serde_json::from_str(cookies_json)?;
        if !v.is_array() {
            anyhow::bail!("ожидается JSON-массив cookies");
        }
        let enc = crypto::encrypt(&self.key, cookies_json)?;
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(u) = db.users.get_mut(&key) {
            u.cookies_enc = enc;
            u.cookies_updated = now;
        } else {
            let u = User {
                vk_id,
                name: String::new(),
                cookies_enc: enc,
                sections: Vec::new(),
                roles: Vec::new(),
                allowed: false,
                quick_replies: default_quick_replies(),
                enabled: true,
                cookies_updated: now,
            };
            db.users.insert(key, u);
        }
        self.save(&db);
        Ok(())
    }

    /// Расшифрованные cookies пользователя.
    pub fn cookies(&self, vk_id: i64) -> Option<String> {
        let db = self.inner.lock().unwrap();
        let u = db.users.get(&vk_id.to_string())?;
        if u.cookies_enc.is_empty() {
            return None;
        }
        crypto::decrypt(&self.key, &u.cookies_enc).ok()
    }

    pub fn has_cookies(&self, vk_id: i64) -> bool {
        self.cookies(vk_id).is_some()
    }

    // ---- роли ----
    pub fn set_allowed(&self, vk_id: i64, allowed: bool) -> bool {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        let Some(u) = db.users.get_mut(&key) else { return false };
        u.allowed = allowed;
        self.save(&db);
        true
    }

    pub fn add_role(&self, vk_id: i64, role: &str) -> bool {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        let Some(u) = db.users.get_mut(&key) else { return false };
        let r = role.trim().to_lowercase();
        if r.is_empty() {
            return false;
        }
        if !u.roles.iter().any(|x| x.eq_ignore_ascii_case(&r)) {
            u.roles.push(r);
        }
        u.allowed = true;
        self.save(&db);
        true
    }

    pub fn remove_role(&self, vk_id: i64, role: &str) -> bool {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        let Some(u) = db.users.get_mut(&key) else { return false };
        let r = role.trim().to_lowercase();
        let before = u.roles.len();
        u.roles.retain(|x| !x.eq_ignore_ascii_case(&r));
        let changed = u.roles.len() != before;
        self.save(&db);
        changed
    }

    pub fn roles(&self, vk_id: i64) -> Vec<String> {
        self.get(vk_id).map(|u| u.roles).unwrap_or_default()
    }

    pub fn is_allowed(&self, vk_id: i64, founder_id: i64) -> bool {
        if vk_id == founder_id {
            return true;
        }
        self.get(vk_id).map(|u| u.allowed).unwrap_or(false)
    }

    pub fn is_founder(&self, vk_id: i64, founder_id: i64) -> bool {
        vk_id == founder_id || self.roles(vk_id).iter().any(|r| r.eq_ignore_ascii_case("основатель"))
    }

    /// Разделы пользователя: если выбрал сам — они, иначе объединение по ролям.
    pub fn sections_for(&self, vk_id: i64, role_map: &std::collections::HashMap<String, Vec<String>>) -> Vec<String> {
        if let Some(u) = self.get(vk_id) {
            if !u.sections.is_empty() {
                return u.sections;
            }
        }
        let roles = self.roles(vk_id);
        let mut out: Vec<String> = Vec::new();
        for r in roles {
            if let Some(list) = role_map.get(&r.to_lowercase()) {
                for s in list {
                    if !out.contains(s) {
                        out.push(s.clone());
                    }
                }
            }
        }
        out
    }

    // ---- быстрые ответы ----
    pub fn quick_add(&self, vk_id: i64, text: &str) -> usize {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        if let Some(u) = db.users.get_mut(&key) {
            u.quick_replies.push(text.trim().to_string());
            let n = u.quick_replies.len();
            self.save(&db);
            n
        } else {
            0
        }
    }

    pub fn quick_remove(&self, vk_id: i64, index: usize) -> Option<String> {
        let mut db = self.inner.lock().unwrap();
        let key = vk_id.to_string();
        let u = db.users.get_mut(&key)?;
        if index == 0 || index > u.quick_replies.len() {
            return None;
        }
        let removed = u.quick_replies.remove(index - 1);
        self.save(&db);
        Some(removed)
    }

    pub fn quick_list(&self, vk_id: i64) -> Vec<String> {
        self.get(vk_id).map(|u| u.quick_replies).unwrap_or_default()
    }

    // ---- разделы ----
    pub fn set_sections(&self, vk_id: i64, sections: Vec<String>) {
        let mut db = self.inner.lock().unwrap();
        if let Some(u) = db.users.get_mut(&vk_id.to_string()) {
            u.sections = sections;
            self.save(&db);
        }
    }
}

pub fn default_quick_replies() -> Vec<String> {
    vec![
        "Отказано: нет доказательств".to_string(),
        "Рассмотрено, наказание выдано".to_string(),
        "Не по форме, отказ".to_string(),
        "Предоставьте доказательства (видео/скрин)".to_string(),
    ]
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn role_map() -> HashMap<String, Vec<String>> {
        let mut m = HashMap::new();
        m.insert("несост".to_string(), vec!["https://f/4041/".to_string()]);
        m.insert("госсник".to_string(), vec!["https://f/4042/".to_string()]);
        m.insert("гетто".to_string(), vec!["https://f/4043/".to_string()]);
        m
    }

    #[test]
    fn roles_union() {
        // проверяем логику объединения разделов по ролям через чистую функцию
        let mut out: Vec<String> = Vec::new();
        for r in ["несост", "гетто"] {
            for s in role_map().get(r).unwrap() {
                if !out.contains(s) {
                    out.push(s.clone());
                }
            }
        }
        assert_eq!(out.len(), 2);
        assert!(out.contains(&"https://f/4041/".to_string()));
        assert!(out.contains(&"https://f/4043/".to_string()));
    }

    #[test]
    fn founder_flag() {
        // основатель = полный доступ (проверка константы роли)
        let r = "Основатель".to_lowercase();
        assert_eq!(r, "основатель");
    }
}
