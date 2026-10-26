// SPDX-License-Identifier: GPL-3.0-or-later
//! The web search page (`/search`, `/web`).
//!
//! Clavis routes `web` searches through a small engine table: a keyword
//! prefix (`g rust`) narrows to one engine, a bare query offers every engine,
//! and a literal URL opens as-is. This module keeps that shape as pure logic:
//! the page renders [`searches`] and the shell opens the chosen `url`.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Engine {
    /// The row id, stable across sessions (`google`, `duckduckgo`, …).
    pub id: &'static str,
    pub title: &'static str,
    /// The prefix typed before a query, and what the row teaches.
    pub keyword: &'static str,
    /// Anything else that selects this engine (`google`, `yt`, `维基`).
    pub aliases: &'static [&'static str],
    pub home: &'static str,
    /// The search URL; `{}` is replaced by the encoded query.
    pub search: &'static str,
}

impl Engine {
    fn matches(&self, token: &str) -> bool {
        let token = token.to_lowercase();
        self.keyword == token || self.aliases.iter().any(|alias| *alias == token)
    }

    fn target(&'static self, query: &str) -> WebSearch {
        let query = query.trim();
        WebSearch {
            engine: self,
            url: if query.is_empty() {
                self.home.to_owned()
            } else {
                self.search.replace("{}", &encode(query))
            },
            query: query.to_owned(),
            literal: false,
        }
    }
}

/// One row of the page: a search on one engine, or the literal URL the draft
/// spells out.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WebSearch {
    pub engine: &'static Engine,
    pub query: String,
    pub url: String,
    /// The draft is a URL, not a search: the row opens it directly.
    pub literal: bool,
}

impl WebSearch {
    /// The stable row id the view keys its debug selector on.
    pub fn id(&self) -> &'static str {
        if self.literal {
            "url"
        } else {
            self.engine.id
        }
    }

    /// The primary row label: the engine's name, or what the query will do.
    pub fn title(&self) -> String {
        if self.literal {
            "打开网址".to_owned()
        } else if self.query.is_empty() {
            self.engine.title.to_owned()
        } else {
            format!("用 {} 搜索", self.engine.title)
        }
    }

    /// The quiet line under it: the keyword the page teaches, then the query.
    pub fn subtitle(&self) -> String {
        if self.literal {
            self.url.clone()
        } else if self.query.is_empty() {
            format!("关键词 {} · 回车打开 {}", self.engine.keyword, host(&self.url))
        } else {
            self.query.clone()
        }
    }
}

pub const ENGINES: &[Engine] = &[
    Engine {
        id: "google",
        title: "Google",
        keyword: "g",
        aliases: &["google"],
        home: "https://www.google.com/",
        search: "https://www.google.com/search?q={}",
    },
    Engine {
        id: "duckduckgo",
        title: "DuckDuckGo",
        keyword: "ddg",
        aliases: &["duckduckgo"],
        home: "https://duckduckgo.com/",
        search: "https://duckduckgo.com/?q={}",
    },
    Engine {
        id: "bing",
        title: "Bing",
        keyword: "b",
        aliases: &["bing"],
        home: "https://www.bing.com/",
        search: "https://www.bing.com/search?q={}",
    },
    Engine {
        id: "github",
        title: "GitHub",
        keyword: "gh",
        aliases: &["github"],
        home: "https://github.com/",
        search: "https://github.com/search?q={}",
    },
    Engine {
        id: "wikipedia",
        title: "维基百科",
        keyword: "w",
        aliases: &["wiki", "wikipedia", "维基"],
        home: "https://zh.wikipedia.org/",
        search: "https://zh.wikipedia.org/w/index.php?search={}",
    },
    Engine {
        id: "youtube",
        title: "YouTube",
        keyword: "yt",
        aliases: &["youtube"],
        home: "https://www.youtube.com/",
        search: "https://www.youtube.com/results?search_query={}",
    },
    Engine {
        id: "bilibili",
        title: "哔哩哔哩",
        keyword: "bili",
        aliases: &["bilibili", "哔哩哔哩", "b站"],
        home: "https://www.bilibili.com/",
        search: "https://search.bilibili.com/all?keyword={}",
    },
];

/// A URL the draft spells out in full. Only explicit schemes and `www.` hosts
/// count, so a stray word with a dot stays a search.
pub fn literal_url(query: &str) -> Option<String> {
    let query = query.trim();
    let lower = query.to_lowercase();
    let explicit = lower.starts_with("https://")
        || lower.starts_with("http://")
        || lower.starts_with("www.");
    if !explicit || query.contains(char::is_whitespace) {
        return None;
    }
    if lower.contains(char::is_control) {
        return None;
    }
    Some(if lower.starts_with("www.") {
        format!("https://{query}")
    } else {
        query.to_owned()
    })
}

/// The rows the page shows for a draft: a literal URL opens directly, an
/// engine prefix (`g rust`) narrows to that engine, and a bare query offers
/// every engine.
pub fn searches(query: &str) -> Vec<WebSearch> {
    let query = query.trim();
    if let Some(url) = literal_url(query) {
        return vec![WebSearch {
            engine: &ENGINES[0],
            query: query.to_owned(),
            url,
            literal: true,
        }];
    }
    let mut parts = query.splitn(2, char::is_whitespace);
    let first = parts.next().unwrap_or_default();
    let rest = parts.next().unwrap_or_default().trim_start();
    if let Some(engine) = ENGINES.iter().find(|engine| engine.matches(first)) {
        return vec![engine.target(rest)];
    }
    ENGINES.iter().map(|engine| engine.target(query)).collect()
}

/// The host shown in a row subtitle (`https://www.google.com/` → `google.com`).
fn host(url: &str) -> &str {
    url.split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .split('/')
        .next()
        .unwrap_or(url)
        .trim_start_matches("www.")
}

/// Percent-encode a query for a URL. Unreserved bytes stay, spaces become
/// `+` (accepted by every engine here), everything else is UTF-8 hex.
pub fn encode(query: &str) -> String {
    let mut out = String::with_capacity(query.len());
    for byte in query.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            b' ' => out.push('+'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_query_offers_every_engine() {
        let rows = searches("rust wayland");
        assert_eq!(rows.len(), ENGINES.len());
        assert_eq!(rows[0].engine.id, "google");
        assert_eq!(
            rows[0].url,
            "https://www.google.com/search?q=rust+wayland"
        );
        assert_eq!(rows[0].title(), "用 Google 搜索");
        assert_eq!(rows[0].subtitle(), "rust wayland");
    }

    #[test]
    fn an_engine_prefix_or_alias_narrows_to_that_engine() {
        let rows = searches("gh tokio");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].engine.id, "github");
        assert_eq!(rows[0].url, "https://github.com/search?q=tokio");

        let rows = searches("维基 肖邦");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].engine.id, "wikipedia");
        assert_eq!(
            rows[0].url,
            "https://zh.wikipedia.org/w/index.php?search=%E8%82%96%E9%82%A6"
        );
    }

    #[test]
    fn an_empty_draft_opens_the_engine_homes() {
        let rows = searches("");
        assert_eq!(rows.len(), ENGINES.len());
        assert_eq!(rows[0].url, "https://www.google.com/");
        assert_eq!(rows[0].title(), "Google");
        assert_eq!(rows[0].subtitle(), "关键词 g · 回车打开 google.com");
    }

    #[test]
    fn a_literal_url_opens_as_is() {
        let rows = searches("https://example.com/a?b=1");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].url, "https://example.com/a?b=1");
        assert_eq!(rows[0].id(), "url");
        assert_eq!(rows[0].title(), "打开网址");

        let rows = searches("www.example.com");
        assert_eq!(rows[0].url, "https://www.example.com");

        // A word with a dot is still a search.
        assert!(literal_url("src/lib.rs").is_none());
        assert!(literal_url("https://example.com/with space").is_none());
    }

    #[test]
    fn the_engine_table_keeps_its_prefixes_unique() {
        let mut tokens: Vec<_> = ENGINES
            .iter()
            .flat_map(|engine| {
                std::iter::once(engine.keyword).chain(engine.aliases.iter().copied())
            })
            .collect();
        tokens.sort_unstable();
        let total = tokens.len();
        tokens.dedup();
        assert_eq!(tokens.len(), total, "a prefix selects two engines");
        assert_eq!(searches("g"), vec![ENGINES[0].target("")]);
    }
}
