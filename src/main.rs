//! Arizona RP парсер-бот (Rust): мультипользовательский.
//! - у каждого пользователя свои cookies (зашифрованы) и свои разделы
//! - быстрые ответы (шаблоны) с кнопками в VK
//! - уведомления только владельцу cookies
mod browser;
mod config;
mod crypto;
mod html_parser;
mod models;
mod nick;
mod storage;
mod users;
mod vk;

use browser::Forum;
use config::Config;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use storage::Storage;
use tokio::sync::Mutex;
use users::Users;
use vk::Vk;

/// Активная сессия пользователя (браузер с его cookies).
struct Session {
    forum: Arc<Forum>,
    forum_user: String,
}

type Sessions = Arc<Mutex<HashMap<i64, Arc<Session>>>>;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .init();

    let cfg = Arc::new(Config::load());
    log::info!("Разделов по умолчанию: {}", cfg.sections().len());
    log::info!("Админ-раздел: {:?} | ники: {:?}", cfg.admin_section, cfg.admin_nicks);

    let storage = Arc::new(Storage::open(&cfg.db_file)?);
    let users = Arc::new(Users::open()?);
    let vk = Arc::new(Vk::new((*cfg).clone()));
    let sessions: Sessions = Arc::new(Mutex::new(HashMap::new()));

    // гарантируем админа в списке
    let (admin, _) = users.ensure(cfg.vk_admin_id, "Основатель");
    users.add_role(cfg.vk_admin_id, "основатель");
    users.set_allowed(cfg.vk_admin_id, true);
    log::info!("Пользователей в базе: {}", users.all().len());
    log::info!("Админ: {} (cookies: {})", admin.name, users.has_cookies(admin.vk_id));

    // ---- VK Long Poll ----
    {
        let vk2 = vk.clone();
        let cfg2 = cfg.clone();
        let storage2 = storage.clone();
        let users2 = users.clone();
        let sessions2 = sessions.clone();
        tokio::spawn(async move {
            log::info!("VK Long Poll запущен");
            let vk_lp = vk2.clone();
            vk_lp
                .long_poll(move |upd| {
                    let vk3 = vk2.clone();
                    let cfg3 = cfg2.clone();
                    let storage3 = storage2.clone();
                    let users3 = users2.clone();
                    let sessions3 = sessions2.clone();
                    async move {
                        handle_vk(upd, vk3, cfg3, storage3, users3, sessions3).await;
                    }
                })
                .await;
        });
    }

    // ---- Telegram: приём команд ----
    {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(40))
            .build()
            .unwrap_or_default();
        let cfg_tg = cfg.clone();
        let users_tg = users.clone();
        let storage_tg = storage.clone();
        let vk_tg = vk.clone();
        let sessions_tg = sessions.clone();
        tokio::spawn(async move {
            tg_poll(http, cfg_tg, users_tg, storage_tg, vk_tg, sessions_tg).await;
        });
    }

    // ---- периодический парсинг: парсим ОДИН раз, рассылаем ВСЕМ ----
    {
        let cfg2 = cfg.clone();
        let storage2 = storage.clone();
        let users2 = users.clone();
        let vk2 = vk.clone();
        let sessions2 = sessions.clone();
        tokio::spawn(async move {
            let http = reqwest::Client::builder()
                .timeout(Duration::from_secs(10))
                .user_agent("Mozilla/5.0")
                .build()
                .unwrap_or_default();
            // первый круг всегда "тихий", чтобы не заспамить после обновления
            let mut first_round = true;
            loop {
                let owner = users2
                    .cookies(cfg2.vk_admin_id)
                    .map(|c| (cfg2.vk_admin_id, c))
                    .or_else(|| {
                        users2.all().into_iter()
                            .find_map(|u| users2.cookies(u.vk_id).map(|c| (u.vk_id, c)))
                    });

                let Some((owner_id, cookies)) = owner else {
                    log::warn!("нет пользователей с cookies — парсинг невозможен");
                    tokio::time::sleep(Duration::from_secs(cfg2.check_interval_secs)).await;
                    continue;
                };

                let Some(forum) = get_session(&sessions2, &cfg2, owner_id, &cookies, "owner").await else {
                    tokio::time::sleep(Duration::from_secs(cfg2.check_interval_secs)).await;
                    continue;
                };

                // --- жалобы на администратора ---
                if !cfg2.admin_section.is_empty() {
                    if let Ok(threads) = forum.forum.parse_section(&cfg2, &cfg2.admin_section).await {
                        let mut checked = 0usize;
                        for t in threads {
                            if checked >= 12 { break; }
                            let key = format!("admin|{}", t.url);
                            if storage2.is_seen(&key) { continue; }
                            checked += 1;
                            if let Ok((body, author)) = forum.forum.read_first_post(&t.url).await {
                                storage2.add_key(&key, &t);
                                if nick::contains_admin_nick(&body, &cfg2.admin_nicks).is_some() {
                                    let snippet: String = body.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(600).collect();
                                    let msg = format!(
                                        "⚠️ Жалоба на администратора\n\n📌 {}\n👤 Автор: {}\n\n📝 {}\n\n🔗 {}",
                                        t.title, author, snippet, t.url
                                    );
                                    let kb = reply_keyboard(&t.id);
                                    for u in users2.all().iter().filter(|u| u.enabled) {
                                        vk2.send_message(u.vk_id, &msg, Some(kb.clone())).await;
                                    }
                                    tg_send(&http, &cfg2, &msg).await;
                                    log::info!("[admin-alert] {}", t.title);
                                    storage2.mark_notified_key(&key);
                                }
                            }
                        }
                    }
                }

                // --- обычные разделы ---
                for sec in cfg2.sections() {
                    let seed_section = !storage2.has_section(&sec);
                    match forum.forum.parse_section(&cfg2, &sec).await {
                        Ok(threads) => {
                            log::info!("раздел {}: {} тем", sec, threads.len());
                            for t in threads {
                                let key = format!("all|{}", t.url);
                                if storage2.is_seen(&key) { continue; }
                                storage2.add_key(&key, &t);
                                if first_round || seed_section { continue; }
                                let sec_name = cfg2.section_title(&sec);
                                // читаем первый пост: ссылки на доказательства
                                let mut evidence = String::new();
                                if let Ok((body, _author)) = forum.forum.read_first_post(&t.url).await {
                                    let links = extract_links(&body);
                                    if !links.is_empty() {
                                        let (ok, bad) = check_links(&http, &links).await;
                                        evidence = if bad == 0 {
                                            format!("\n🧾 Доказательства: ✅ {} сс. доступны", ok)
                                        } else {
                                            format!("\n🧾 Доказательства: ⚠️ {} ок, {} битых", ok, bad)
                                        };
                                    } else {
                                        evidence = "\n🧾 Доказательства: ❌ не приложены".to_string();
                                    }
                                }
                                let msg = format!(
                                    "🆕 Новая тема\n🗂 {}\n\n📌 {}\n👤 Автор: {}\n🔗 {}{}",
                                    sec_name, t.title, t.author, t.url, evidence
                                );
                                let kb = reply_keyboard(&t.id);
                                tg_send_kb(&http, &cfg2, &msg, &t.id).await;
                                let mut sent = 0;
                                for u in users2.all().iter().filter(|u| u.enabled) {
                                    // если человек выбрал разделы — шлём только по ним; иначе всё
                                    let want = if u.sections.is_empty() {
                                        true
                                    } else {
                                        u.sections.iter().any(|s| {
                                            s.trim_end_matches('/') == sec.trim_end_matches('/')
                                        })
                                    };
                                    if want && vk2.send_message(u.vk_id, &msg, Some(kb.clone())).await {
                                        sent += 1;
                                    }
                                }
                                if sent > 0 { log::info!("уведомление: {} ({} чел.)", t.title, sent); }
                                storage2.mark_notified_key(&key);
                                let d = cfg2.min_delay + rand::random::<f64>() * (cfg2.max_delay - cfg2.min_delay);
                                tokio::time::sleep(Duration::from_secs_f64(d)).await;
                            }
                        }
                        Err(e) => log::error!("раздел {}: {}", sec, e),
                    }
                }

                first_round = false;
                tokio::time::sleep(Duration::from_secs(cfg2.check_interval_secs)).await;
            }
        });
    }

    // ---- консоль ----
    console_loop(cfg, vk, storage, users).await;
    Ok(())
}


fn admin_panel_keyboard() -> Value {
    json!({
        "inline": true,
        "buttons": [
            [
                {"action": {"type": "callback", "label": "👥 Люди", "payload": "{\"cmd\":\"adm\",\"a\":\"users\"}"}, "color": "secondary"},
                {"action": {"type": "callback", "label": "🎭 Роли", "payload": "{\"cmd\":\"adm\",\"a\":\"roles\"}"}, "color": "secondary"}
            ],
            [
                {"action": {"type": "callback", "label": "📊 Статус", "payload": "{\"cmd\":\"adm\",\"a\":\"status\"}"}, "color": "primary"}
            ]
        ]
    })
}


/// Вытаскивает ссылки из текста.

/// Обработка команд в Telegram (только для админа).
async fn tg_poll(
    http: reqwest::Client,
    cfg: Arc<Config>,
    users: Arc<Users>,
    storage: Arc<Storage>,
    vk: Arc<Vk>,
    sessions: Sessions,
) {
    if cfg.tg_token.is_empty() || cfg.tg_chat_id.is_empty() {
        return;
    }
    log::info!("Telegram-бот: приём команд запущен");
    let mut offset: i64 = 0;
    loop {
        let url = format!("https://api.telegram.org/bot{}/getUpdates", cfg.tg_token);
        let body = serde_json::json!({"offset": offset, "timeout": 25, "allowed_updates": ["message", "callback_query"]});
        let resp = http.post(&url).json(&body).send().await;
        let Ok(resp) = resp else {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        let Ok(v) = resp.json::<serde_json::Value>().await else {
            tokio::time::sleep(Duration::from_secs(5)).await;
            continue;
        };
        if let Some(upds) = v.get("result").and_then(|x| x.as_array()) {
            for u in upds {
                if let Some(id) = u.get("update_id").and_then(|x| x.as_i64()) {
                    offset = id + 1;
                }
                // --- нажатие кнопки ---
                if let Some(cq) = u.get("callback_query") {
                    let data = cq.get("data").and_then(|d| d.as_str()).unwrap_or("").to_string();
                    let cb_id = cq.get("id").and_then(|d| d.as_str()).unwrap_or("").to_string();
                    let _ = http.post(format!("https://api.telegram.org/bot{}/answerCallbackQuery", cfg.tg_token))
                        .json(&serde_json::json!({"callback_query_id": cb_id})).send().await;
                    if let Some((kind, tid)) = data.split_once(':') {
                        let url = format!("{}/threads/{}/", cfg.forum_base, tid);
                        let owner = users.cookies(cfg.vk_admin_id);
                        if let Some(cookies) = owner {
                            if let Some(forum) = get_session(&sessions, &cfg, cfg.vk_admin_id, &cookies, "tg").await {
                                if kind == "c" {
                                    if let Ok((body, author)) = forum.forum.read_first_post(&url).await {
                                        let snippet: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
                                        let snippet: String = snippet.chars().take(3800).collect();
                                        tg_send(&http, &cfg, &format!("📄 Содержимое темы\n\n👤 {}\n\n{}", author, snippet)).await;
                                    }
                                } else if kind == "r" {
                                    storage.set_pending(cfg.vk_admin_id, tid, &url);
                                    tg_send(&http, &cfg, "✍️ Пришли текст ответа одним сообщением.").await;
                                }
                            }
                        }
                    }
                    continue;
                }
                let Some(msg) = u.get("message") else { continue };
                let chat_id = msg.get("chat").and_then(|c| c.get("id")).and_then(|x| x.as_i64()).unwrap_or(0);
                let text = msg.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
                if chat_id.to_string() != cfg.tg_chat_id {
                    continue;
                }
                let low = text.to_lowercase();
                if matches!(low.as_str(), "/start" | "начать" | "/help" | "help") {
                    tg_send(&http, &cfg, "🤖 Парсер Arizona RP\n\n/status — статистика\n/users — пользователи\n/msg текст — рассылка в ВК\n/sections — разделы").await;
                } else if low == "/status" {
                    let t = format!(
                        "📊 Статистика\n\nТем в базе: {}\nПользователей: {}\nРазделов: {}",
                        storage.count(), users.all().len(), cfg.sections().len()
                    );
                    tg_send(&http, &cfg, &t).await;
                } else if low == "/users" || low == "люди" {
                    let mut t = String::from("👥 Пользователи:\n");
                    for u in users.all() {
                        let ck = if u.cookies_enc.is_empty() { "нет" } else { "есть" };
                        t.push_str(&format!("\n• {} | cookies: {} | разделов: {}\n", u.vk_id, ck, u.sections.len()));
                    }
                    tg_send(&http, &cfg, &t).await;
                } else if let Some(rest) = text.strip_prefix("/msg ") {
                    let mut sent = 0;
                    for u in users.all().iter().filter(|u| u.enabled) {
                        if vk.send_message(u.vk_id, rest.trim(), None).await {
                            sent += 1;
                        }
                    }
                    tg_send(&http, &cfg, &format!("✅ Отправлено в ВК: {}", sent)).await;
                } else if low == "/sections" {
                    let mut t = String::from("📋 Разделы:\n");
                    for (i, url) in cfg.sections().iter().enumerate() {
                        t.push_str(&format!("{}. {}\n", i + 1, cfg.section_title(url)));
                    }
                    tg_send(&http, &cfg, &t).await;
                } else if let Some((tid, url)) = storage.get_pending(cfg.vk_admin_id) {
                    storage.clear_pending(cfg.vk_admin_id);
                    if let Some(cookies) = users.cookies(cfg.vk_admin_id) {
                        if let Some(forum) = get_session(&sessions, &cfg, cfg.vk_admin_id, &cookies, "tg").await {
                            let _ = tid;
                            match forum.forum.post_reply(&url, &text).await {
                                Ok(()) => tg_send(&http, &cfg, &format!("✅ Ответ опубликован:\n{}", url)).await,
                                Err(e) => tg_send(&http, &cfg, &format!("❌ Не удалось: {}", e)).await,
                            }
                        }
                    }
                } else if !text.is_empty() {
                    tg_send(&http, &cfg, "Не понял. /help").await;
                }
            }
        }
    }
}

fn extract_links(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for raw in text.split_whitespace() {
        let t = raw.trim_matches(|c: char| c == '.' || c == ',' || c == ')' || c == '(' || c == ']' || c == '[' || c == ';' || c == '"' || c == '\'');
        if (t.starts_with("http://") || t.starts_with("https://")) && t.len() > 12 {
            if !out.iter().any(|x| x == t) {
                out.push(t.to_string());
            }
        }
    }
    out.truncate(4);
    out
}

/// Проверяет доступность ссылок: (ok, bad).
async fn check_links(http: &reqwest::Client, links: &[String]) -> (usize, usize) {
    let mut ok = 0usize;
    let mut bad = 0usize;
    for l in links {
        match http.head(l).send().await {
            Ok(r) => {
                if r.status().is_success() || r.status().is_redirection() { ok += 1; } else { bad += 1; }
            }
            Err(_) => bad += 1,
        }
    }
    (ok, bad)
}

/// Отправка в Telegram с кнопками.
async fn tg_send_kb(http: &reqwest::Client, cfg: &Config, text: &str, tid: &str) {
    if cfg.tg_token.is_empty() || cfg.tg_chat_id.is_empty() {
        return;
    }
    let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.tg_token);
    let body = serde_json::json!({
        "chat_id": cfg.tg_chat_id,
        "text": text,
        "disable_web_page_preview": true,
        "reply_markup": {
            "inline_keyboard": [[
                {"text": "📄 Содержимое", "callback_data": format!("c:{}", tid)},
                {"text": "✍️ Ответить", "callback_data": format!("r:{}", tid)}
            ]]
        }
    });
    let _ = http.post(&url).json(&body).send().await;
}

/// Дублирует сообщение в Telegram (если задан токен).
async fn tg_send(http: &reqwest::Client, cfg: &Config, text: &str) {
    if cfg.tg_token.is_empty() || cfg.tg_chat_id.is_empty() {
        return;
    }
    let url = format!("https://api.telegram.org/bot{}/sendMessage", cfg.tg_token);
    let body = serde_json::json!({
        "chat_id": cfg.tg_chat_id,
        "text": text,
        "disable_web_page_preview": true
    });
    let _ = http.post(&url).json(&body).send().await;
}

fn chat_peer() -> Option<i64> {
    std::fs::read_to_string("chat_peer.txt").ok()
        .and_then(|t| t.trim().parse::<i64>().ok())
        .filter(|id| *id >= 2_000_000_000)
}

fn save_chat_peer(peer_id: i64) {
    if peer_id >= 2_000_000_000 {
        let _ = std::fs::write("chat_peer.txt", peer_id.to_string());
    }
}

fn sections_help(cfg: &Config) -> String {
    let mut t = String::from("📋 Доступные разделы:\n\n");
    for (i, url) in cfg.sections().iter().enumerate() {
        t.push_str(&format!("{}. {}\n", i + 1, cfg.section_title(url)));
    }
    t.push_str("\nВыбери номера через запятую, например:\n/setsections 1,3,5");
    t
}

fn split_message(text: &str, limit: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for line in text.lines() {
        if cur.len() + line.len() + 1 > limit {
            if !cur.is_empty() { out.push(std::mem::take(&mut cur)); }
        }
        cur.push_str(line);
        cur.push('\n');
    }
    if !cur.is_empty() { out.push(cur); }
    out
}

fn reply_keyboard(tid: &str) -> Value {
    json!({
        "inline": true,
        "buttons": [[
            {"action": {"type": "callback", "label": "📄 Содержимое",
                       "payload": json!({"cmd":"content","tid":tid}).to_string()},
             "color": "secondary"},
            {"action": {"type": "callback", "label": "✍️ Ответить",
                       "payload": json!({"cmd":"reply","tid":tid}).to_string()},
             "color": "primary"}
        ]]
    })
}

/// Клавиатура с быстрыми ответами.
fn quick_keyboard(replies: &[String], tid: &str) -> Value {
    let mut rows: Vec<Value> = Vec::new();
    for (i, r) in replies.iter().enumerate() {
        let label: String = if r.chars().count() > 38 {
            r.chars().take(35).collect::<String>() + "..."
        } else {
            r.clone()
        };
        rows.push(json!([{
            "action": {"type": "callback", "label": label,
                       "payload": json!({"cmd":"quick","tid":tid,"i":i}).to_string()},
            "color": "secondary"
        }]));
    }
    rows.push(json!([{
        "action": {"type": "callback", "label": " Свой текст",
                   "payload": json!({"cmd":"custom","tid":tid}).to_string()},
        "color": "primary"
    }]));
    json!({"inline": true, "buttons": rows})
}

async fn get_session(
    sessions: &Sessions,
    cfg: &Config,
    vk_id: i64,
    cookies: &str,
    name: &str,
) -> Option<Arc<Session>> {
    let mut map = sessions.lock().await;
    if let Some(s) = map.get(&vk_id) {
        return Some(s.clone());
    }
    let profile = format!("profiles/vk{}-{}", vk_id, rand::random::<u32>());
    match Forum::start(cfg, cookies, Some(&profile)).await {
        Ok(f) => {
            let logged = f.is_logged_in(&cfg.forum_base).await;
            let fuser = if logged {
                f.current_user(&cfg.forum_base).await.unwrap_or_default()
            } else {
                String::new()
            };
            log::info!("[{}] сессия: форумный аккаунт = {:?}", name, fuser);
            let s = Arc::new(Session { forum: Arc::new(f), forum_user: fuser });
            map.insert(vk_id, s.clone());
            Some(s)
        }
        Err(e) => {
            log::error!("[{}] браузер: {}", name, e);
            None
        }
    }
}

async fn handle_vk(
    upd: Value,
    vk: Arc<Vk>,
    cfg: Arc<Config>,
    storage: Arc<Storage>,
    users: Arc<Users>,
    sessions: Sessions,
) {
    let typ = upd.get("type").and_then(|t| t.as_str()).unwrap_or("");
    let obj = upd.get("object").cloned().unwrap_or(Value::Null);

    if typ == "message_event" {
        on_callback(obj, vk, cfg, users, storage, sessions).await;
    } else if typ == "message_new" {
        let msg = obj.get("message").cloned().unwrap_or(Value::Null);
        on_message(msg, vk, cfg, users, storage, sessions).await;
    }
}

async fn on_callback(
    obj: Value,
    vk: Arc<Vk>,
    cfg: Arc<Config>,
    users: Arc<Users>,
    storage: Arc<Storage>,
    sessions: Sessions,
) {
    let uid = obj.get("user_id").and_then(|v| v.as_i64()).unwrap_or(0);
    if uid == 0 || !users.is_allowed(uid, cfg.vk_admin_id) {
        return;
    }
    let event_id = obj.get("event_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let peer_id = obj.get("peer_id").and_then(|v| v.as_i64()).unwrap_or(uid);
    let payload = match obj.get("payload") {
        Some(Value::String(s)) => serde_json::from_str::<Value>(s).unwrap_or(Value::Null),
        Some(v) => v.clone(),
        None => Value::Null,
    };
    let cmd = payload.get("cmd").and_then(|c| c.as_str()).unwrap_or("");
    let tid = payload.get("tid").and_then(|t| t.as_str()).unwrap_or("").to_string();

    let snack = |text: &str| {
        vk.call(
            "messages.sendMessageEventAnswer",
            vec![
                ("event_id", event_id.clone()),
                ("user_id", uid.to_string()),
                ("peer_id", peer_id.to_string()),
                ("event_data", json!({"type":"show_snackbar","text":text}).to_string()),
            ],
        )
    };

    match cmd {
        "reply" => {
            if users.cookies(uid).is_none() {
                let _ = snack("Сначала добавь cookies: /cookies").await;
                vk.send_message(peer_id, "Нет cookies. Пришли /cookies и JSON с форума — потом сможешь отвечать.", None).await;
                return;
            }
            storage.set_pending(uid, &tid, &tid);
            storage.clear_pending_text(uid);
            let replies = users.quick_list(uid);
            let kb = quick_keyboard(&replies, &tid);
            let _ = snack("Напиши ответ или выбери шаблон").await;
            let mut text_msg = String::from("✍️ Напиши свой ответ одним сообщением — он уйдёт в тему.\n\n");
            if !replies.is_empty() {
                text_msg.push_str("Или отправь номер шаблона:\n");
                for (i, r) in replies.iter().enumerate() {
                    text_msg.push_str(&format!("{}. {}\n", i + 1, r));
                }
            }
            text_msg.push_str("\n/cancel — отмена");
            vk.send_message(peer_id, &text_msg, Some(kb)).await;
        }
        "quick" => {
            let idx = payload.get("i").and_then(|i| i.as_u64()).unwrap_or(0) as usize;
            let replies = users.quick_list(uid);
            if let Some(text) = replies.get(idx) {
                storage.set_pending(uid, &tid, &tid);
                storage.set_pending_text(uid, text);
                let _ = snack("Отправляю быстрый ответ").await;
                publish_reply(uid, &tid, text, vk.clone(), cfg.clone(), users.clone(), storage.clone(), sessions.clone()).await;
            } else {
                let _ = snack("Шаблон не найден").await;
            }
        }
        "content" => {
            let _ = snack("Загружаю содержимое...").await;
            let Some(cookies) = users.cookies(uid) else {
                vk.send_message(peer_id, "Нет cookies. Пришли /cookies, чтобы видеть содержимое.", None).await;
                return;
            };
            let url = if tid.starts_with("http") {
                tid.clone()
            } else {
                format!("{}/threads/{}/", cfg.forum_base, tid)
            };
            match get_session(&sessions, &cfg, uid, &cookies, "").await {
                Some(forum) => match forum.forum.read_first_post(&url).await {
                    Ok((body, author)) => {
                        let snippet: String = body.split_whitespace().collect::<Vec<_>>().join(" ");
                        let snippet: String = snippet.chars().take(20000).collect();
                        let msg = format!("📄 Содержимое темы\n\n👤 Автор: {}\n\n{}\n\n🔗 {}", author, snippet, url);
                        for part in split_message(&msg, 3800) {
                            vk.send_message(peer_id, &part, None).await;
                        }
                    }
                    Err(e) => {
                        vk.send_message(peer_id, &format!("Не удалось прочитать тему: {e}"), None).await;
                    }
                },
                None => {
                    vk.send_message(peer_id, "Не удалось запустить браузер для чтения темы.", None).await;
                }
            }
        }
        "custom" => {
            storage.set_pending(uid, &tid, &tid);
            storage.clear_pending_text(uid);
            let _ = snack("Пришли текст ответа").await;
            vk.send_message(peer_id, " Пришли текст ответа одним сообщением.\n(Отмена: /cancel)", None).await;
        }
        "adm" => {
            if !users.is_founder(uid, cfg.vk_admin_id) {
                let _ = snack("Только для основателя").await;
                return;
            }
            let action = payload.get("a").and_then(|a| a.as_str()).unwrap_or("");
            match action {
                "users" => {
                    let mut t = String::from("👥 Пользователи:\n\n");
                    for u in users.all() {
                        let roles = if u.roles.is_empty() { "—".to_string() } else { u.roles.join(", ") };
                        let cookies = if u.cookies_enc.is_empty() { "нет" } else { "есть" };
                        t.push_str(&format!("• {} | {} | cookies: {} | {}\n", u.vk_id, roles, cookies,
                            if u.allowed { "допущен" } else { "нет доступа" }));
                    }
                    t.push_str("\n/allow <id> <роль>  /deny <id>");
                    let _ = snack("Список людей").await;
                    vk.send_message(peer_id, &t, None).await;
                }
                "roles" => {
                    let map = cfg.role_sections();
                    let mut keys: Vec<String> = map.keys().cloned().collect();
                    keys.sort();
                    let mut t = String::from("🎭 Роли:\n\n");
                    for r in keys {
                        let secs = map.get(&r).cloned().unwrap_or_default();
                        let names: Vec<String> = secs.iter().map(|u| cfg.section_title(u)).collect();
                        t.push_str(&format!("• {} → {}\n", r, names.join(", ")));
                    }
                    t.push_str("\n/role <id> <роль>");
                    let _ = snack("Список ролей").await;
                    vk.send_message(peer_id, &t, None).await;
                }
                "status" => {
                    let t = format!(
                        "📊 Статистика\n\nТем в базе: {}\nПользователей: {}\nРазделов: {}",
                        storage.count(), users.all().len(), cfg.sections().len()
                    );
                    let _ = snack("Статистика").await;
                    vk.send_message(peer_id, &t, None).await;
                }
                _ => { let _ = snack("Неизвестно").await; }
            }
        }
        _ => {}
    }
}

async fn publish_reply(
    uid: i64,
    tid: &str,
    text: &str,
    vk: Arc<Vk>,
    cfg: Arc<Config>,
    users: Arc<Users>,
    storage: Arc<Storage>,
    sessions: Sessions,
) {
    let url = if tid.starts_with("http") {
        tid.to_string()
    } else {
        format!("{}/threads/{}/", cfg.forum_base, tid)
    };
    let Some(cookies) = users.cookies(uid) else {
        vk.send_message(uid, " Сначала добавь cookies: /cookies", None).await;
        return;
    };
    let forum = {
        let map = sessions.lock().await;
        map.get(&uid).map(|s| s.forum.clone())
    };
    let forum = match forum {
        Some(f) => f,
        None => match Forum::start(&cfg, &cookies, Some(&format!("profiles/vk{}-{}", uid, rand::random::<u32>()))).await {
            Ok(f) => {
                let f = Arc::new(f);
                let s = Arc::new(Session { forum: f.clone(), forum_user: String::new() });
                sessions.lock().await.insert(uid, s);
                f
            }
            Err(e) => {
                vk.send_message(uid, &format!(" Браузер: {e}"), None).await;
                return;
            }
        },
    };
    match forum.post_reply(&url, text).await {
        Ok(()) => {
            storage.clear_pending(uid);
            storage.clear_pending_text(uid);
            vk.send_message(uid, &format!(" Ответ опубликован:\n{url}"), None).await;
        }
        Err(e) => {
            vk.send_message(uid, &format!(" Не удалось отправить: {e}"), None).await;
        }
    }
}

async fn on_message(
    msg: Value,
    vk: Arc<Vk>,
    cfg: Arc<Config>,
    users: Arc<Users>,
    storage: Arc<Storage>,
    sessions: Sessions,
) {
    let uid = msg.get("from_id").and_then(|v| v.as_i64()).unwrap_or(0);
    if uid == 0 {
        return;
    }
    let peer_id = msg.get("peer_id").and_then(|v| v.as_i64()).unwrap_or(uid);
    let text = msg.get("text").and_then(|t| t.as_str()).unwrap_or("").trim().to_string();
    let low = text.to_lowercase();

    let (_, created) = users.ensure(uid, "");
    let founder = users.is_founder(uid, cfg.vk_admin_id);

    // если пишут в беседу — запоминаем её, туда будем слать жалобы
    let in_chat = peer_id >= 2_000_000_000;

    // ---- самозапись по команде «начать» ----
    if !in_chat && !users.is_allowed(uid, cfg.vk_admin_id) {
        let start_like = matches!(low.as_str(),
            "начать" | "start" | "/start" | "привет" | "прив" | "/help" | "help" | "/menu" | "меню");
        if start_like {
            users.set_allowed(uid, true);
            vk.send_message(peer_id,
                &format!("Добро пожаловать! Ты зарегистрирован.\nТвой VK ID: {uid}\n\nВыбери разделы командой /setsections (я покажу список).\nПотом пришли /cookies — без них не будешь видеть темы."),
                None).await;
            let list = sections_help(&cfg);
            vk.send_message(peer_id, &list, None).await;
        } else {
            vk.send_message(peer_id,
                &format!("Привет! Чтобы зарегистрироваться, напиши: начать\n\nТвой VK ID: {uid}"),
                None).await;
        }
        return;
    }

    // ---- админ-панель (только основатель) ----
    if founder {
        if matches!(low.as_str(), "/panel" | "панель" | "/admin" | "админка" | "/start") {
            let kb = admin_panel_keyboard();
            vk.send_message(
                peer_id,
                "🛠 Админ-панель\n\n\
                 /users — список людей\n\
                 /allow <id> [роль,роль] — допустить\n\
                 /deny <id> — убрать доступ\n\
                 /role <id> <роль> — выдать роль\n\
                 /unrole <id> <роль> — снять роль\n\
                 /roles — роли и разделы\n\
                 /status — статистика",
                Some(kb),
            )
            .await;
            return;
        }

        if low == "/roles" || low == "роли" {
            let map = cfg.role_sections();
            let mut keys: Vec<String> = map.keys().cloned().collect();
            keys.sort();
            let mut t = String::from("🎭 Роли и разделы:\n\n");
            for r in keys {
                let secs = map.get(&r).cloned().unwrap_or_default();
                let names: Vec<String> = secs.iter().map(|u| cfg.section_title(u)).collect();
                t.push_str(&format!("• {} → {}\n", r, names.join(", ")));
            }
            t.push_str("\nВыдать роль: /role <id> <роль>");
            vk.send_message(peer_id, &t, None).await;
            return;
        }

        if low == "/users" || low == "люди" {
            let mut t = String::from("👥 Пользователи:\n\n");
            for u in users.all() {
                let roles = if u.roles.is_empty() { "—".to_string() } else { u.roles.join(", ") };
                let cookies = if u.cookies_enc.is_empty() { "нет" } else { "есть" };
                t.push_str(&format!(
                    "• {} | роли: {} | cookies: {} | {}\n",
                    u.vk_id,
                    roles,
                    cookies,
                    if u.allowed { "допущен" } else { "нет доступа" }
                ));
            }
            t.push_str("\n/allow <id> <роль>  /deny <id>");
            vk.send_message(peer_id, &t, None).await;
            return;
        }

        if let Some(rest) = text.strip_prefix("/allow ") {
            let mut it = rest.splitn(2, ' ');
            let id_s = it.next().unwrap_or("");
            let roles_s = it.next().unwrap_or("");
            if let Ok(id) = id_s.trim().parse::<i64>() {
                users.ensure(id, "");
                users.set_allowed(id, true);
                let mut added = Vec::new();
                for r in roles_s.split(',').map(|x| x.trim()).filter(|x| !x.is_empty()) {
                    let rl = r.to_lowercase();
                    if cfg.role_sections().contains_key(&rl) || rl == "основатель" {
                        users.add_role(id, &rl);
                        added.push(rl);
                    }
                }
                let msg = if added.is_empty() {
                    format!("✅ {id} допущен. Роль не задана — выдай: /role {id} госсник")
                } else {
                    format!("✅ {id} допущен. Роли: {}", added.join(", "))
                };
                vk.send_message(peer_id, &msg, None).await;
            } else {
                vk.send_message(peer_id, "Использование: /allow <id> [роль,роль]", None).await;
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("/deny ") {
            if let Ok(id) = rest.trim().parse::<i64>() {
                users.set_allowed(id, false);
                vk.send_message(peer_id, &format!("🚫 {id} больше не имеет доступа"), None).await;
            } else {
                vk.send_message(peer_id, "Использование: /deny <id>", None).await;
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("/role ") {
            let mut it = rest.splitn(2, ' ');
            let id_s = it.next().unwrap_or("");
            let role = it.next().unwrap_or("").trim().to_lowercase();
            if let Ok(id) = id_s.trim().parse::<i64>() {
                if role.is_empty() {
                    vk.send_message(peer_id, "Использование: /role <id> <роль>", None).await;
                } else if cfg.role_sections().contains_key(&role) || role == "основатель" {
                    users.ensure(id, "");
                    users.add_role(id, &role);
                    vk.send_message(peer_id, &format!("✅ {id}: выдана роль «{role}»"), None).await;
                } else {
                    vk.send_message(peer_id, &format!("❌ Неизвестная роль «{role}». Список: /roles"), None).await;
                }
            } else {
                vk.send_message(peer_id, "Использование: /role <id> <роль>", None).await;
            }
            return;
        }

        if let Some(rest) = text.strip_prefix("/unrole ") {
            let mut it = rest.splitn(2, ' ');
            let id_s = it.next().unwrap_or("");
            let role = it.next().unwrap_or("").trim().to_lowercase();
            if let Ok(id) = id_s.trim().parse::<i64>() {
                if users.remove_role(id, &role) {
                    vk.send_message(peer_id, &format!("✅ {id}: роль «{role}» снята"), None).await;
                } else {
                    vk.send_message(peer_id, &format!("⚠️ У {id} нет роли «{role}»"), None).await;
                }
            } else {
                vk.send_message(peer_id, "Использование: /unrole <id> <роль>", None).await;
            }
            return;
        }
    }

    if low == "/whoami" || low == "мои роли" {
        let roles = users.roles(uid);
        let secs = users.sections_for(uid, &cfg.role_sections());
        let names: Vec<String> = secs.iter().map(|u| cfg.section_title(u)).collect();
        vk.send_message(
            peer_id,
            &format!(
                "Твой ID: {uid}\nРоли: {}\nРазделы: {}",
                if roles.is_empty() { "—".to_string() } else { roles.join(", ") },
                if names.is_empty() { "—".to_string() } else { names.join(", ") }
            ),
            None,
        )
        .await;
        return;
    }

    // awaiting cookies? (пользователь прислал JSON)
    if text.starts_with('[') && text.ends_with(']') {
        match users.set_cookies(uid, &text) {
            Ok(()) => {
                // проверим cookies сразу
                vk.send_message(peer_id, " Cookies сохранены (зашифрованы). Проверяю вход...", None).await;
                match Forum::start(&cfg, &text, Some(&format!("profiles/vk{}-{}", uid, rand::random::<u32>()))).await {
                    Ok(f) => {
                        let logged = f.is_logged_in(&cfg.forum_base).await;
                        let name = if logged { f.current_user(&cfg.forum_base).await } else { None };
                        let f = Arc::new(f);
                        sessions.lock().await.insert(uid, Arc::new(Session { forum: f, forum_user: name.clone().unwrap_or_default() }));
                        if logged {
                            vk.send_message(peer_id, &format!(" Вход выполнен: {}", name.unwrap_or_default()), None).await;
                        } else {
                            vk.send_message(peer_id, " Cookies не подошли  старые? Войди заново и пришли свежие.", None).await;
                        }
                    }
                    Err(e) => {
                        vk.send_message(peer_id, &format!(" Браузер: {e}"), None).await;
                    }
                }
            }
            Err(e) => {
                vk.send_message(peer_id, &format!(" Ошибка cookies: {e}\nНужен JSON-массив."), None).await;
            }
        }
        return;
    }

    // пропущенные ответы
    match low.as_str() {
        "начать" | "/start" | "start" | "/help" | "help" => {
            let help = " Бот парсера Arizona RP\n\n\
                /cookies  добавить свои cookies (пришли JSON)\n\
                /status  статус\n\
                /mysections  мои разделы\n\
                /setsections 4041,4042  задать разделы\n\
                /quick  мои быстрые ответы\n\
                /qadd текст  добавить шаблон\n\
                /qdel номер  удалить шаблон\n\
                /cancel  отменить\n\n\
                Под уведомлением жми  Ответить  появятся шаблоны.";
            vk.send_message(peer_id, help, None).await;
        }
        "/cookies" => {
            vk.send_message(peer_id,
                " Пришли JSON-массив cookies (экспорт из Cookie-Editor).\n\
                 Они будут зашифрованы и использованы только от твоего имени.",
                None).await;
        }
        "/status" => {
            let has = users.has_cookies(uid);
            let list = users.quick_list(uid);
            let s = format!(
                "Пользователь: {}\nCookies: {}\nШаблонов: {}\nТем в базе: {}",
                uid,
                if has { "есть " } else { "нет " },
                list.len(),
                users.get(uid).map(|u| u.sections.len()).unwrap_or(0)
            );
            vk.send_message(peer_id, &s, None).await;
        }
        "/quick" => {
            let list = users.quick_list(uid);
            if list.is_empty() {
                vk.send_message(peer_id, "Шаблонов нет. Добавь: /qadd текст", None).await;
            } else {
                let mut s = String::from(" Быстрые ответы:\n");
                for (i, r) in list.iter().enumerate() {
                    s.push_str(&format!("{}. {}\n", i + 1, r));
                }
                vk.send_message(peer_id, &s, None).await;
            }
        }
        "/cancel" | "отмена" => {
            vk.send_message(peer_id, "Отменено.", None).await;
        }
        "/mysections" => {
            let secs = users.get(uid).map(|u| u.sections).unwrap_or_default();
            let s = if secs.is_empty() {
                format!("Разделы по умолчанию: {:?}", cfg.sections())
            } else {
                format!("Твои разделы: {secs:?}")
            };
            vk.send_message(peer_id, &s, None).await;
        }
        _ => {
            // команды без аргументов
            if low == "/qadd" || low == "qadd" {
                vk.send_message(peer_id, "Использование: /qadd текст шаблона\nПример: /qadd Отказано: нет доказательств", None).await;
                return;
            }
            if low == "/qdel" || low == "qdel" {
                vk.send_message(peer_id, "Использование: /qdel номер\nПосмотреть номера: /quick", None).await;
                return;
            }
            if low == "/setsections" || low == "разделы" {
                let list = sections_help(&cfg);
                vk.send_message(peer_id, &list, None).await;
                return;
            }
            if let Some(rest) = text.strip_prefix("/qadd ") {
                let text_add = rest.trim().to_string();
                if text_add.is_empty() {
                    vk.send_message(peer_id, "Использование: /qadd текст шаблона", None).await;
                } else {
                    let n = users.quick_add(uid, &text_add);
                    vk.send_message(peer_id, &format!(" Добавлено. Всего шаблонов: {n}"), None).await;
                }
            } else if let Some(rest) = text.strip_prefix("/qdel ") {
                if let Ok(idx) = rest.trim().parse::<usize>() {
                    match users.quick_remove(uid, idx) {
                        Some(rem) => { vk.send_message(peer_id, &format!(" Удалено: {rem}"), None).await; }
                        None => { vk.send_message(peer_id, " Нет шаблона с таким номером", None).await; }
                    }
                } else {
                    vk.send_message(peer_id, "Использование: /qdel номер", None).await;
                }
            } else if let Some(rest) = text.strip_prefix("/setsections ") {
                let all = cfg.sections();
                let mut secs: Vec<String> = Vec::new();
                for part in rest.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
                    if part.starts_with("http") {
                        secs.push(part.to_string());
                    } else if let Ok(n) = part.parse::<usize>() {
                        if n >= 1 && n <= all.len() {
                            secs.push(all[n - 1].clone());
                        }
                    } else {
                        secs.push(format!("{}/forums/{}/", cfg.forum_base, part));
                    }
                }
                users.set_sections(uid, secs.clone());
                vk.send_message(peer_id, &format!(" Разделы: {secs:?}"), None).await;
                        } else if let Some((tid, _url)) = storage.get_pending(uid) {
                storage.clear_pending(uid);
                // номер шаблона -> шаблон, иначе введённый текст
                let replies = users.quick_list(uid);
                let final_text = if let Ok(n) = text.parse::<usize>() {
                    if n >= 1 && n <= replies.len() { replies[n - 1].clone() } else { text.clone() }
                } else { text.clone() };
                vk.send_message(peer_id, "Публикую ответ...", None).await;
                publish_reply(uid, &tid, &final_text, vk.clone(), cfg.clone(), users.clone(), storage.clone(), sessions.clone()).await;
            } else if !text.is_empty() {
                vk.send_message(peer_id, "Не понял. Напиши /help", None).await;
            }
        }
    }
}

async fn console_loop(cfg: Arc<Config>, vk: Arc<Vk>, storage: Arc<Storage>, users: Arc<Users>) {
    println!("============================================================");
    println!(" Команды: msg <текст> | msg <id> <текст> | users | status | quit");
    println!("============================================================");
    let stdin = std::io::stdin();
    loop {
        let mut line = String::new();
        if stdin.read_line(&mut line).is_err() {
            loop { tokio::time::sleep(Duration::from_secs(3600)).await; }
        }
        let line = line.trim().to_string();
        if line.is_empty() { continue; }
        let mut parts = line.splitn(3, ' ');
        let cmd = parts.next().unwrap_or("").to_lowercase();
        let a1 = parts.next().unwrap_or("");
        let a2 = parts.next().unwrap_or("");
        match cmd.as_str() {
            "quit" | "exit" | "q" => std::process::exit(0),
            "help" | "?" => println!(" msg <текст> | msg <id> <текст> | users | status | importcookies [id] [файл] | quit"),
            "users" => {
                for u in users.all() {
                    println!("  {} | {} | cookies: {} | шаблонов: {}",
                        u.vk_id, if u.name.is_empty() { "-" } else { &u.name },
                        if u.cookies_enc.is_empty() { "нет" } else { "есть" },
                        u.quick_replies.len());
                }
            }
            "status" => println!(" Тем в базе: {}\n Разделов: {}\n Пользователей: {}",
                storage.count(), cfg.sections().len(), users.all().len()),
            "importcookies" => {
                // importcookies [id] [файл= cookies.json]
                let uid = a1.parse::<i64>().unwrap_or(cfg.vk_admin_id);
                let path = if a2.is_empty() { "cookies.json" } else { a2 };
                match std::fs::read_to_string(path) {
                    Ok(json) => match users.set_cookies(uid, &json) {
                        Ok(()) => println!("  cookies зашифрованы и сохранены для {uid}"),
                        Err(e) => println!("  {e}"),
                    },
                    Err(e) => println!("  не прочитать {path}: {e}"),
                }
            }
            "msg" | "send" => {
                if a1.is_empty() { println!(" msg <текст> | msg <id> <текст>"); continue; }
                if let Ok(uid) = a1.parse::<i64>() {
                    if !a2.is_empty() {
                        let ok = vk.send_message(uid, a2, None).await;
                        println!(" {} -> {}", if ok { "" } else { "" }, uid);
                        continue;
                    }
                }
                let text = format!("{a1} {a2}").trim().to_string();
                let mut sent = 0;
                for u in users.all() {
                    if vk.send_message(u.vk_id, &text, None).await { sent += 1; }
                }
                println!("  отправлено: {sent}");
            }
            _ => println!(" Неизвестная команда. help"),
        }
    }
}


#[cfg(test)]
mod tests {
    use super::crypto;

    #[test]
    fn encrypt_roundtrip() {
        let key = [7u8; 32];
        let plain = r#"[{"name":"xf_user","value":"SECRET"}]"#;
        let enc = crypto::encrypt(&key, plain).unwrap();
        assert!(!enc.contains("SECRET"), "ciphertext не должен содержать открытый текст");
        let dec = crypto::decrypt(&key, &enc).unwrap();
        assert_eq!(dec, plain);
        // другой ключ - ошибка
        let key2 = [8u8; 32];
        assert!(crypto::decrypt(&key2, &enc).is_err());
    }

    #[test]
    fn user_cookies_encrypted() {
        let key = [9u8; 32];
        let plain = r#"[{"name":"xf_user","value":"TOPSECRETVALUE"}]"#;
        let enc = crypto::encrypt(&key, plain).unwrap();
        assert!(!enc.contains("TOPSECRETVALUE"));
    }
}
