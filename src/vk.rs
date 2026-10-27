//! VK: отправка сообщений, уведомления, Long Poll.
use crate::config::Config;
use crate::models::Thread;
use serde_json::{json, Value};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

const V: &str = "5.199";

static KEYBOARDS_OK: AtomicBool = AtomicBool::new(true);

#[derive(Clone)]
pub struct Vk {
    http: reqwest::Client,
    cfg: Config,
}

impl Vk {
    pub fn new(cfg: Config) -> Self {
        Self {
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .unwrap_or_default(),
            cfg,
        }
    }

    pub async fn call(&self, method: &str, mut params: Vec<(&str, String)>) -> Option<Value> {
        params.push(("access_token", self.cfg.vk_token.clone()));
        params.push(("v", V.to_string()));
        let url = format!("https://api.vk.com/method/{method}");
        let resp = self
            .http
            .post(&url)
            .form(&params)
            .send()
            .await
            .ok()?
            .json::<Value>()
            .await
            .ok()?;
        if let Some(err) = resp.get("error") {
            let code = err.get("error_code").and_then(|v| v.as_i64()).unwrap_or(0);
            let msg = err.get("error_msg").and_then(|v| v.as_str()).unwrap_or("");
            if code == 912 || msg.to_lowercase().contains("chat bot feature") {
                KEYBOARDS_OK.store(false, Ordering::Relaxed);
                return Some(json!({"__keyboard_error__": true}));
            }
            log::error!("VK {method} error: {msg}");
            return None;
        }
        resp.get("response").cloned()
    }

    pub async fn send_message(&self, peer_id: i64, text: &str, keyboard: Option<Value>) -> bool {
        let mut params = vec![
            ("peer_id", peer_id.to_string()),
            ("message", text.to_string()),
            ("random_id", rand::random::<u32>().to_string()),
        ];
        let use_kb = keyboard.is_some() && KEYBOARDS_OK.load(Ordering::Relaxed);
        if use_kb {
            params.push(("keyboard", keyboard.unwrap().to_string()));
        }
        let res = self.call("messages.send", params).await;
        if let Some(v) = &res {
            if v.get("__keyboard_error__").is_some() {
                log::warn!("VK: кнопки недоступны (912). Отправляю без кнопок.");
                let mut p2 = vec![
                    ("peer_id", peer_id.to_string()),
                    ("message", text.to_string()),
                    ("random_id", rand::random::<u32>().to_string()),
                ];
                p2.push(("v", V.to_string()));
                p2.push(("access_token", self.cfg.vk_token.clone()));
                let r2 = self.http.post("https://api.vk.com/method/messages.send")
                    .form(&p2).send().await
                    .ok().and_then(|_| Some(()));
                return r2.is_some();
            }
        }
        match &res {
            Some(v) => {
                let ok = v.as_i64().is_some() || v.as_str().is_some();
                if !ok {
                    log::warn!("VK send: неожиданный ответ: {}", v);
                }
                ok
            }
            None => {
                log::warn!("VK send: пустой ответ (ошибка выше)");
                false
            }
        }
    }

    fn thread_keyboard(tid: &str) -> Value {
        json!({
            "inline": true,
            "buttons": [[{
                "action": {"type": "callback", "label": " Ответить",
                           "payload": json!({"cmd":"reply","tid":tid}).to_string()},
                "color": "primary"
            }]]
        })
    }

    pub async fn notify_thread(&self, t: &Thread) -> bool {
        let ids = self.cfg.recipients();
        let text = format!(
            " Новая тема на форуме\n\n {}\n Автор: {}\n Раздел: {}\n {}",
            t.title, t.author, t.section, t.url
        );
        let kb = Self::thread_keyboard(&t.id);
        let mut any = false;
        for uid in ids {
            if self.send_message(uid, &text, Some(kb.clone())).await {
                any = true;
            }
        }
        any
    }

    pub async fn send_to_all(&self, text: &str) -> Vec<(i64, bool)> {
        let mut out = Vec::new();
        for uid in self.cfg.recipients() {
            let ok = self.send_message(uid, text, None).await;
            out.push((uid, ok));
        }
        out
    }

    /// Long Poll цикл. Каждое событие отдаётся в callback.
    pub async fn long_poll<F, Fut>(&self, mut on_event: F)
    where
        F: FnMut(Value) -> Fut,
        Fut: std::future::Future<Output = ()>,
    {
        loop {
            let server = match self
                .call(
                    "groups.getLongPollServer",
                    vec![("group_id", self.cfg.vk_group_id.to_string())],
                )
                .await
            {
                Some(v) => v,
                None => {
                    log::error!("VK: не удалось получить Long Poll сервер");
                    tokio::time::sleep(Duration::from_secs(5)).await;
                    continue;
                }
            };
            let mut key = server["key"].as_str().unwrap_or_default().to_string();
            let mut ts = server["ts"].as_str().unwrap_or_default().to_string();
            let srv = server["server"].as_str().unwrap_or_default().to_string();

            loop {
                let resp = self
                    .http
                    .get(&srv)
                    .query(&[("act", "a_check"), ("key", &key), ("ts", &ts), ("wait", "25")])
                    .timeout(Duration::from_secs(35))
                    .send()
                    .await;
                let Ok(resp) = resp else {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                    break;
                };
                let Ok(v) = resp.json::<Value>().await else { break };
                if v.get("failed").is_some() {
                    break;
                }
                if let Some(new_ts) = v.get("ts").and_then(|t| t.as_str()) {
                    ts = new_ts.to_string();
                }
                if let Some(updates) = v.get("updates").and_then(|u| u.as_array()) {
                    for upd in updates {
                        on_event(upd.clone()).await;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            let _ = &mut key;
        }
    }
}
