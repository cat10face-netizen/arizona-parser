//! Браузер (chromiumoxide): вход через cookies, L7, парсинг, ответы.
//! Поддерживает изолированные профили (по пользователю).
use crate::config::Config;
use crate::html_parser;
use crate::models::Thread;
use chromiumoxide::browser::{Browser, BrowserConfig};
use chromiumoxide::cdp::browser_protocol::network::{CookieParam, TimeSinceEpoch};
use chromiumoxide::page::Page;
use futures::StreamExt;
use serde_json::Value;
use std::time::Duration;
use tokio::task::JoinHandle;

pub struct Forum {
    pub browser: Browser,
    handler_task: JoinHandle<()>,
}

impl Forum {
    /// Запускает браузер с cookies пользователя.
    /// `profile_dir`  отдельный профиль Chrome для изоляции (None = общий).
    pub async fn start(cfg: &Config, cookies_json: &str, profile_dir: Option<&str>) -> anyhow::Result<Self> {
        let mut builder = BrowserConfig::builder()
            .arg("--disable-blink-features=AutomationControlled")
            .arg("--no-first-run")
            .arg("--no-default-browser-check")
            .arg("--disable-dev-shm-usage")
            .arg("--no-sandbox")
            .window_size(1400, 950);
        // Авто-поиск Chrome: сначала путь из конфига, потом типовые места.
        if let Some(chrome) = detect_chrome(cfg.chrome_path.as_deref()) {
            log::info!("Chrome: {}", chrome.display());
            builder = builder.chrome_executable(chrome);
        } else {
            log::warn!("Chrome не найден по типовым путям — пробую системный по умолчанию");
        }
        if let Some(dir) = profile_dir {
            std::fs::create_dir_all(dir)?;
            builder = builder.user_data_dir(dir);
        }
        if !cfg.headless {
            builder = builder.with_head();
        }
        let conf = builder.build().map_err(|e| anyhow::anyhow!("browser config: {e}"))?;

        let (browser, mut handler) = Browser::launch(conf).await?;
        let handler_task = tokio::spawn(async move {
            while handler.next().await.is_some() {}
        });

        let mut forum = Self { browser, handler_task };
        forum.set_cookies(cookies_json).await?;
        Ok(forum)
    }

    async fn set_cookies(&mut self, cookies_json: &str) -> anyhow::Result<()> {
        let cookies: Vec<Value> = serde_json::from_str(cookies_json)?;
        let mut params = Vec::new();
        for c in &cookies {
            let name = c["name"].as_str().unwrap_or_default().to_string();
            let value = c["value"].as_str().unwrap_or_default().to_string();
            if name.is_empty() {
                continue;
            }
            let domain = c["domain"]
                .as_str()
                .unwrap_or("forum.arizona-rp.com")
                .to_string();
            let mut b = CookieParam::builder()
                .name(name)
                .value(value)
                .domain(domain)
                .path("/");
            if let Some(exp) = c["expires"].as_f64() {
                if exp > 0.0 {
                    b = b.expires(TimeSinceEpoch::new(exp));
                }
            }
            if let Some(s) = c["secure"].as_bool() {
                b = b.secure(s);
            }
            if let Some(h) = c["httpOnly"].as_bool() {
                b = b.http_only(h);
            }
            if let Ok(p) = b.build() {
                params.push(p);
            }
        }
        if !params.is_empty() {
            self.browser.set_cookies(params).await?;
        }
        Ok(())
    }

    async fn open(&self, url: &str) -> anyhow::Result<Page> {
        let page = self.browser.new_page(url).await?;
        self.wait_challenge(&page).await?;
        Ok(page)
    }

    async fn wait_challenge(&self, page: &Page) -> anyhow::Result<()> {
        for _ in 0..40 {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let html = page.content().await.unwrap_or_default();
            if !html.is_empty() && !html_parser::is_challenge(&html) {
                return Ok(());
            }
        }
        Ok(())
    }

    pub async fn is_logged_in(&self, base: &str) -> bool {
        for attempt in 0..3 {
            if let Ok(page) = self.open(&format!("{base}/")).await {
                let html = page.content().await.unwrap_or_default();
                let low = html.to_lowercase();
                let _ = page.close().await;
                if low.contains("/logout/")
                    || (low.contains("/account/") && low.contains("p-navgroup-link--user"))
                {
                    return true;
                }
                if low.contains("войдите или зарегистрируйтесь") {
                    return false;
                }
                if attempt < 2 {
                    tokio::time::sleep(Duration::from_secs(2)).await;
                }
            } else {
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        false
    }

    pub async fn current_user(&self, base: &str) -> Option<String> {
        let page = self.open(&format!("{base}/")).await.ok()?;
        let html = page.content().await.ok()?;
        let _ = page.close().await;
        let doc = scraper::Html::parse_document(&html);
        let sel = scraper::Selector::parse(".p-navgroup-link--user .p-navgroup-linkText").ok()?;
        doc.select(&sel)
            .next()
            .map(|e| e.text().collect::<String>().trim().to_string())
    }

    /// Экспортирует текущие cookies (после прохождения L7), чтобы обновить сохранённые.
    pub async fn export_cookies(&self) -> anyhow::Result<String> {
        let cookies = self.browser.get_cookies().await?;
        let arr: Vec<Value> = cookies
            .iter()
            .filter_map(|c| serde_json::to_value(c).ok())
            .collect();
        Ok(serde_json::to_string(&arr)?)
    }

    pub async fn parse_section(&self, cfg: &Config, url: &str) -> anyhow::Result<Vec<Thread>> {
        let page = self.open(url).await?;
        let html = page.content().await.unwrap_or_default();
        let _ = page.close().await;
        if html_parser::page_is_empty(&html) {
            return Ok(Vec::new());
        }
        let mut threads = html_parser::parse_threads(&html, url);
        threads.truncate(cfg.max_threads_per_section);
        Ok(threads)
    }

    /// Публикует ответ в теме (через реальный браузер).
    pub async fn post_reply(&self, thread_url: &str, message: &str) -> anyhow::Result<()> {
        let page = self.open(thread_url).await?;
        tokio::time::sleep(Duration::from_millis(1200)).await;

        let js = r#"
        (async () => {
            const msg = __MSG__;
            const btn = [...document.querySelectorAll('button')].find(b => (b.innerText||'').trim() === 'Ответить');
            if (btn) { btn.click(); await new Promise(r => setTimeout(r, 1800)); }
            const ed = document.querySelector("form.js-quickReply .fr-element[contenteditable='true'], .fr-element[contenteditable='true']");
            if (!ed) return {ok:false, error:'поле ответа не найдено (тема закрыта?)'};
            ed.focus();
            document.execCommand('insertText', false, msg);
            await new Promise(r => setTimeout(r, 400));
            const form = document.querySelector("form.js-quickReply") || ed.closest('form');
            if (!form) return {ok:false, error:'форма ответа не найдена'};
            const sub = form.querySelector("button[type='submit']") || form.querySelector("button");
            if (!sub) return {ok:false, error:'кнопка отправки не найдена'};
            sub.click();
            return {ok:true};
        })()
        "#;
        let msg_json = serde_json::to_string(message)?;
        let js = js.replace("__MSG__", &msg_json);
        let res = page.evaluate(js).await?;
        let val = res.value().cloned().unwrap_or(Value::Null);
        tokio::time::sleep(Duration::from_secs(3)).await;
        let ok = val.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let _ = page.close().await;
        if ok {
            Ok(())
        } else {
            let err = val
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("неизвестная ошибка")
                .to_string();
            anyhow::bail!(err)
        }
    }


    /// Читает первый пост темы. Возвращает (текст, автор).
    pub async fn read_first_post(&self, thread_url: &str) -> anyhow::Result<(String, String)> {
        let page = self.open(thread_url).await?;
        let html = page.content().await.unwrap_or_default();
        let _ = page.close().await;
        let doc = scraper::Html::parse_document(&html);

        let art_sel = scraper::Selector::parse("article.message.is-first, article.message.message--post").unwrap();
        let body_sel = scraper::Selector::parse(".message-body .bbWrapper, .bbWrapper").unwrap();

        let mut text = String::new();
        let mut author = String::new();
        if let Some(art) = doc.select(&art_sel).next() {
            author = art.value().attr("data-author").unwrap_or_default().to_string();
            if let Some(body) = art.select(&body_sel).next() {
                for node in body.descendants() {
                    if let Some(t) = node.value().as_text() {
                        text.push_str(t);
                        text.push('\n');
                    }
                }
                // чтобы не потерять ссылки
                let a_sel = scraper::Selector::parse("a[href]").unwrap();
                for a in body.select(&a_sel) {
                    if let Some(href) = a.value().attr("href") {
                        if href.starts_with("http") {
                            text.push_str(href);
                            text.push('\n');
                        }
                    }
                }
            }
        }
        Ok((text, author))
    }

    pub async fn close(mut self) {
        let _ = self.browser.close().await;
        let _ = self.browser.wait().await;
        self.handler_task.abort();
    }
}


/// Ищет Chrome/Chromium: конфиг -> типовые пути Windows/macOS/Linux.
fn detect_chrome(configured: Option<&str>) -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if let Some(p) = configured {
        let p = p.trim();
        if !p.is_empty() {
            let pb = PathBuf::from(p);
            if pb.exists() {
                return Some(pb);
            }
        }
    }
    let candidates: &[&str] = &[
        // Linux
        "/usr/bin/google-chrome",
        "/usr/bin/google-chrome-stable",
        "/usr/bin/chromium",
        "/usr/bin/chromium-browser",
        "/snap/bin/chromium",
        // macOS
        "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
        "/Applications/Chromium.app/Contents/MacOS/Chromium",
        // Windows
        r"C:\Program Files\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files (x86)\Google\Chrome\Application\chrome.exe",
        r"C:\Program Files\Microsoft\Edge\Application\msedge.exe",
    ];
    for c in candidates {
        let pb = PathBuf::from(c);
        if pb.exists() {
            return Some(pb);
        }
    }
    // Windows: переменные окружения
    for var in ["PROGRAMFILES", "PROGRAMFILES(X86)", "LOCALAPPDATA"] {
        if let Ok(base) = std::env::var(var) {
            let pb = PathBuf::from(base)
                .join("Google\\Chrome\\Application\\chrome.exe");
            if pb.exists() {
                return Some(pb);
            }
        }
    }
    None
}
