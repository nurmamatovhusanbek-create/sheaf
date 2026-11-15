//! Small in-memory full-text index over recognized words (CMD-SEARCH).

use sheaf_core::{OcrResult, PageId};
use std::collections::BTreeMap;

#[derive(Debug, Clone, PartialEq)]
pub struct Hit {
    pub page: PageId,
    /// Index into the page's word list of the first matching word.
    pub word: usize,
    /// The line the match is on, for display.
    pub snippet: String,
}

#[derive(Debug, Default)]
pub struct SearchIndex {
    pages: BTreeMap<PageId, Vec<(String, u32)>>,
}

/// Case-insensitive, blind to the apostrophe variants Uzbek Latin text uses
/// for oʻ and gʻ (scans and keyboards render them inconsistently), and with
/// surrounding punctuation stripped.
fn fold(s: &str) -> String {
    let folded: String = s
        .chars()
        .flat_map(char::to_lowercase)
        .map(|c| if "ʻʼ’‘`´".contains(c) { '\'' } else { c })
        .collect();
    folded.trim_matches(|c: char| !c.is_alphanumeric()).to_string()
}

impl SearchIndex {
    pub fn new() -> Self {
        Self::default()
    }

    /// Add or replace a page's words.
    pub fn set_page(&mut self, page: PageId, result: &OcrResult) {
        self.pages
            .insert(page, result.words.iter().map(|w| (w.text.clone(), w.line)).collect());
    }

    pub fn remove_page(&mut self, page: PageId) {
        self.pages.remove(&page);
    }

    /// Pages whose text contains every word of `query` in order, adjacent.
    /// The last query word may be a prefix. At most `limit` hits.
    pub fn search(&self, query: &str, limit: usize) -> Vec<Hit> {
        let needle: Vec<String> = query.split_whitespace().map(fold).collect();
        let mut hits = Vec::new();
        if needle.is_empty() {
            return hits;
        }
        for (&page, words) in &self.pages {
            let folded: Vec<String> = words.iter().map(|(w, _)| fold(w)).collect();
            for start in 0..folded.len().saturating_sub(needle.len() - 1) {
                let window = &folded[start..start + needle.len()];
                let last = needle.len() - 1;
                let matches = window[..last] == needle[..last] && window[last].starts_with(needle[last].as_str());
                if !matches {
                    continue;
                }
                let line = words[start].1;
                let snippet: Vec<&str> = words
                    .iter()
                    .filter(|(_, l)| *l == line)
                    .map(|(w, _)| w.as_str())
                    .collect();
                hits.push(Hit {
                    page,
                    word: start,
                    snippet: snippet.join(" "),
                });
                if hits.len() >= limit {
                    return hits;
                }
            }
        }
        hits
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sheaf_core::Word;

    fn result(lines: &[&str]) -> OcrResult {
        let mut words = Vec::new();
        for (line, text) in lines.iter().enumerate() {
            for w in text.split_whitespace() {
                words.push(Word {
                    text: w.to_string(),
                    bbox: [0; 4],
                    conf: 90.0,
                    line: line as u32,
                });
            }
        }
        OcrResult {
            words,
            ..Default::default()
        }
    }

    #[test]
    fn finds_words_and_phrases_across_pages() {
        let mut index = SearchIndex::new();
        index.set_page(PageId(1), &result(&["Shartnoma № 45", "Toshkent shahri,"]));
        index.set_page(PageId(2), &result(&["Договор поставки", "город Ташкент"]));
        index.set_page(PageId(3), &result(&["Oʻzbekiston Respublikasi"]));

        let hits = index.search("toshkent", 10);
        assert_eq!(hits.len(), 1);
        assert_eq!(
            (hits[0].page, hits[0].word, hits[0].snippet.as_str()),
            (PageId(1), 3, "Toshkent shahri,")
        );
        assert_eq!(index.search("ДОГОВОР постав", 10)[0].page, PageId(2));
        assert_eq!(
            index.search("o'zbekiston", 10)[0].page,
            PageId(3),
            "apostrophe variants match"
        );
        assert!(index.search("shahri Toshkent", 10).is_empty(), "word order matters");
        assert!(index.search("", 10).is_empty());
        assert!(index.search("yoʻq", 10).is_empty());

        index.remove_page(PageId(1));
        assert!(index.search("toshkent", 10).is_empty());
        index.set_page(PageId(2), &result(&["другой текст"]));
        assert!(
            index.search("договор", 10).is_empty(),
            "replacing a page drops its old words"
        );
    }

    #[test]
    fn limit_is_respected() {
        let mut index = SearchIndex::new();
        for i in 0..20 {
            index.set_page(PageId(i), &result(&["same word here"]));
        }
        assert_eq!(index.search("word", 5).len(), 5);
    }
}
