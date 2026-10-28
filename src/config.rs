//! Конфигурация парсера Arizona RP.
use std::fs;
use std::path::Path;

#[derive(Debug, Clone)]
pub struct Config {
    pub forum_base: String,
    pub vk_token: String,
    pub vk_group_id: i64,
    pub vk_admin_id: i64,
    pub check_interval_secs: u64,
    pub min_delay: f64,
    pub max_delay: f64,
    pub headless: bool,
    pub max_threads_per_section: usize,
    pub chrome_path: Option<String>,
    pub cookies_file: String,
    pub db_file: String,
    pub sections_file: String,
    pub recipients_file: String,
    pub log_file: String,
    pub admin_section: String,
    pub admin_nicks: Vec<String>,
    pub tg_token: String,
    pub tg_chat_id: String,
    /// Названия разделов: ссылка -> человекочитаемое имя
    pub section_titles: std::collections::HashMap<String, String>,
    /// Роли: роль -> номера разделов (например "несост" -> ["4041"])
    pub role_map: std::collections::HashMap<String, String>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            forum_base: "https://forum.arizona-rp.com".to_string(),
            vk_token: String::new(),
            vk_group_id: 0,
            vk_admin_id: 0,
            check_interval_secs: 300,
            min_delay: 2.0,
            max_delay: 5.0,
            headless: true,
            max_threads_per_section: 30,
            chrome_path: None,
            cookies_file: "cookies.json".to_string(),
            db_file: "state.db".to_string(),
            sections_file: "sections.txt".to_string(),
            recipients_file: "recipients.txt".to_string(),
            log_file: "parser.log".to_string(),
            admin_section: String::new(),
            admin_nicks: vec!["Richard_Torenno".into(), "R.Torenno".into(), "Richard Torenno".into()],
            tg_token: String::new(),
            tg_chat_id: String::new(),
            section_titles: std::collections::HashMap::new(),
            role_map: default_role_map(),
        }
    }
}

impl Config {
    /// Читает config.toml, если он есть (простой формат key="value").
    pub fn load() -> Self {
        let mut cfg = Config::default();

        // 1) config.toml (если есть)
        if Path::new("config.toml").exists() {
            if let Ok(text) = fs::read_to_string("config.toml") {
                for line in text.lines() {
                    let line = line.trim();
                    if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                        continue;
                    }
                    let Some((k, v)) = line.split_once('=') else { continue };
                    let k = k.trim();
                    let v = v.trim().trim_matches('"').trim_matches('\'');
                    cfg.apply(k, v);
                }
            }
        }

        // 2) переменные окружения переопределяют
        for (key, field) in [
            ("VK_TOKEN", "vk_token"),
            ("VK_GROUP_ID", "vk_group_id"),
            ("VK_ADMIN_ID", "vk_admin_id"),
        ] {
            if let Ok(val) = std::env::var(key) {
                cfg.apply(field, &val);
            }
        }
        cfg
    }

    fn apply(&mut self, key: &str, val: &str) {
        match key {
            "FORUM_BASE" => self.forum_base = val.to_string(),
            "VK_TOKEN" => self.vk_token = val.to_string(),
            "VK_GROUP_ID" => self.vk_group_id = val.parse().unwrap_or(0),
            "VK_ADMIN_ID" => self.vk_admin_id = val.parse().unwrap_or(0),
            "CHECK_INTERVAL" => self.check_interval_secs = val.parse().unwrap_or(300),
            "MIN_DELAY" => self.min_delay = val.parse().unwrap_or(2.0),
            "MAX_DELAY" => self.max_delay = val.parse().unwrap_or(5.0),
            "HEADLESS" => self.headless = val.parse().unwrap_or(true),
            "MAX_THREADS_PER_SECTION" => self.max_threads_per_section = val.parse().unwrap_or(30),
            "CHROME_PATH" => self.chrome_path = Some(val.to_string()),
            "COOKIES_FILE" => self.cookies_file = val.to_string(),
            "DB_FILE" => self.db_file = val.to_string(),
            "SECTIONS_FILE" => self.sections_file = val.to_string(),
            "RECIPIENTS_FILE" => self.recipients_file = val.to_string(),
            "LOG_FILE" => self.log_file = val.to_string(),
            "ADMIN_SECTION" => self.admin_section = val.to_string(),
            "TG_TOKEN" => self.tg_token = val.to_string(),
            "TG_CHAT_ID" => self.tg_chat_id = val.to_string(),
            "ADMIN_NICKS" => self.admin_nicks = val.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect(),
            "ROLE_MAP" => {
                self.role_map.clear();
                for pair in val.split(';') {
                    if let Some((role, secs)) = pair.split_once('=') {
                        let role = role.trim().to_lowercase();
                        let secs = secs.trim();
                        if !role.is_empty() && !secs.is_empty() {
                            self.role_map.insert(role, secs.to_string());
                        }
                    }
                }
            }
            "SECTION_TITLES" => {
                for pair in val.split(';') {
                    if let Some((u, name)) = pair.split_once('=') {
                        let u = u.trim();
                        let name = name.trim();
                        if !u.is_empty() && !name.is_empty() {
                            self.section_titles.insert(u.trim_end_matches('/').to_string(), name.to_string());
                            // также с завершающим слэшем
                            self.section_titles.insert(format!("{}/", u.trim_end_matches('/')), name.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    /// Разделы для каждой роли: роль -> список полных URL.
    pub fn role_sections(&self) -> std::collections::HashMap<String, Vec<String>> {
        let mut out = std::collections::HashMap::new();
        for (role, secs) in &self.role_map {
            let mut urls = Vec::new();
            for part in secs.split(',') {
                let p = part.trim();
                if p.is_empty() {
                    continue;
                }
                let url = if p.starts_with("http") {
                    p.to_string()
                } else {
                    format!("{}/forums/{}/", self.forum_base.trim_end_matches('/'), p.trim_matches('/'))
                };
                urls.push(url);
            }
            out.insert(role.clone(), urls);
        }
        out
    }

    /// Человекочитаемое имя раздела (или номер, если не задано).
    pub fn section_title(&self, url: &str) -> String {
        if let Some(t) = self.section_titles.get(url) {
            return t.clone();
        }
        if let Some(t) = self.section_titles.get(url.trim_end_matches('/')) {
            return t.clone();
        }
        // fallback: номер раздела
        url.trim_end_matches('/').rsplit('/').next().unwrap_or("").to_string()
    }

    /// Список разделов из sections.txt.
    pub fn sections(&self) -> Vec<String> {
        let mut out = Vec::new();
        if let Ok(text) = fs::read_to_string(&self.sections_file) {
            for line in text.lines() {
                let line = line.trim().trim_start_matches('\u{feff}');
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                out.push(line.to_string());
            }
        }
        out
    }

    /// Список получателей из recipients.txt.
    pub fn recipients(&self) -> Vec<i64> {
        let mut out = Vec::new();
        if let Ok(text) = fs::read_to_string(&self.recipients_file) {
            for line in text.lines() {
                let line = line.trim().trim_start_matches('\u{feff}');
                if line.is_empty() || line.starts_with('#') {
                    continue;
                }
                if let Some(first) = line.split_whitespace().next() {
                    if let Ok(id) = first.parse::<i64>() {
                        out.push(id);
                    }
                }
            }
        }
        if out.is_empty() && self.vk_admin_id != 0 {
            out.push(self.vk_admin_id);
        }
        out
    }
}


fn default_role_map() -> std::collections::HashMap<String, String> {
    let mut m = std::collections::HashMap::new();
    m.insert("несост".to_string(), "4041".to_string());
    m.insert("госсник".to_string(), "4042".to_string());
    m.insert("гетто".to_string(), "4043".to_string());
    m.insert("маф".to_string(), "4044".to_string());
    m.insert("игроки".to_string(), "4045".to_string());
    m.insert("слеты".to_string(), "4048".to_string());
    m.insert("смп".to_string(), "4050".to_string());
    m
}
