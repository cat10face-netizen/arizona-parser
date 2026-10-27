//! Модели данных.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Thread {
    pub id: String,
    pub title: String,
    pub url: String,
    pub author: String,
    pub created: String,
    pub last_post: String,
    pub replies: String,
    pub views: String,
    pub flags: String,
    pub section: String,
}

impl Thread {
    pub fn from_url(url: &str) -> Option<String> {
        // /threads/12345/ или /threads/some-slug.12345/
        let after = url.split("/threads/").nth(1)?;
        let first = after.split('/').next()?;
        if let Some(dot) = first.rfind('.') {
            let tail = &first[dot + 1..];
            if tail.chars().all(|c| c.is_ascii_digit()) {
                return Some(tail.to_string());
            }
        }
        if first.chars().all(|c| c.is_ascii_digit()) {
            Some(first.to_string())
        } else {
            None
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplyResult {
    pub ok: bool,
    pub error: Option<String>,
}
