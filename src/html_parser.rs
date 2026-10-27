//! Разбор HTML страниц XenForo (раздел, тема).
use crate::models::Thread;
use scraper::{Html, Selector};

/// Извлекает темы со страницы раздела.
pub fn parse_threads(html: &str, section_url: &str) -> Vec<Thread> {
    let doc = Html::parse_document(html);
    let item_sel = Selector::parse("div.js-threadList .structItem, div.structItem--thread").unwrap();
    let title_sel = Selector::parse(".structItem-title a[href*='/threads/']").unwrap();
    let author_sel = Selector::parse(".structItem-parts .username").unwrap();
    let time_sel = Selector::parse(".structItem-startDate time, time.u-dt").unwrap();
    let meta_dl_sel = Selector::parse(".structItem-cell--meta dl.pairs").unwrap();
    let latest_sel = Selector::parse(".structItem-cell--latest").unwrap();
    let status_sel = Selector::parse(".structItem-statuses i.structItem-status").unwrap();

    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for item in doc.select(&item_sel) {
        let Some(a) = item.select(&title_sel).next() else { continue };
        let href = a.value().attr("href").unwrap_or_default();
        let url = make_absolute(href, section_url);
        let url = url
            .trim_end_matches("/unread")
            .trim_end_matches('/')
            .to_string();
        if url.is_empty() || !seen.insert(url.clone()) {
            continue;
        }
        let title = a.text().collect::<Vec<_>>().join(" ").trim().to_string();

        let author = item
            .select(&author_sel)
            .next()
            .map(|e| e.text().collect::<Vec<_>>().join("").trim().to_string())
            .or_else(|| item.value().attr("data-author").map(|s| s.to_string()))
            .unwrap_or_default();

        let created = item
            .select(&time_sel)
            .next()
            .map(|e| {
                e.value()
                    .attr("data-timestamp")
                    .or_else(|| e.value().attr("datetime"))
                    .unwrap_or_default()
                    .to_string()
            })
            .unwrap_or_default();

        let mut replies = String::new();
        let mut views = String::new();
        for dl in item.select(&meta_dl_sel) {
            let dt = Selector::parse("dt").unwrap();
            let dd = Selector::parse("dd").unwrap();
            let key = dl
                .select(&dt)
                .next()
                .map(|e| e.text().collect::<String>().to_lowercase())
                .unwrap_or_default();
            let val = dl
                .select(&dd)
                .next()
                .map(|e| e.text().collect::<String>().trim().to_string())
                .unwrap_or_default();
            if key.contains("ответ") {
                replies = val;
            } else if key.contains("просмотр") {
                views = val;
            }
        }

        let last_post = item
            .select(&latest_sel)
            .next()
            .map(|e| {
                e.select(&time_sel)
                    .next()
                    .and_then(|t| t.value().attr("data-timestamp").map(|s| s.to_string()))
                    .unwrap_or_else(|| e.text().collect::<String>().trim().to_string())
            })
            .unwrap_or_default();

        let flags = item
            .select(&status_sel)
            .filter_map(|e| e.value().attr("title").map(|s| s.to_string()))
            .collect::<Vec<_>>()
            .join(", ");

        let id = Thread::from_url(&url).unwrap_or_default();

        out.push(Thread {
            id,
            title,
            url,
            author,
            created,
            last_post,
            replies,
            views,
            flags,
            section: section_url.to_string(),
        });
    }
    out
}

/// Пустая ли страница раздела (нет тем).
pub fn page_is_empty(html: &str) -> bool {
    let low = html.to_lowercase();
    low.contains("js-emptyThreadList") && !low.contains("structitem--thread")
        || low.contains("пока нет ни одной темы")
}

/// Ещё идёт проверка браузера?
pub fn is_challenge(html: &str) -> bool {
    let low = html.to_lowercase();
    (low.contains("проверка браузера") || low.contains("checking your browser"))
        && !low.contains("structitem")
}

fn make_absolute(href: &str, base: &str) -> String {
    if href.starts_with("http") {
        return href.to_string();
    }
    if let Ok(base_url) = url::Url::parse(base) {
        if let Ok(joined) = base_url.join(href) {
            return joined.to_string();
        }
    }
    href.to_string()
}
