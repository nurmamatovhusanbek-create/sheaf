//! Link check and appendix generator for `ARCHITECTURE.md`.
//!
//! Rules checked:
//! - every `<a id="x"></a>` anchor is unique;
//! - every `](#x)` link points at an anchor;
//! - an ID link ``[`SEC-NONET`](#sec-nonet)`` names the anchor it points at;
//! - the generated appendices (reverse index, open VERIFY items) are current.
//!
//! No third-party dependencies on purpose (SEC-SUPPLY).

use std::collections::{BTreeMap, BTreeSet};

pub const BEGIN_A: &str = "<!-- BEGIN GENERATED appendix-a -->";
pub const END_A: &str = "<!-- END GENERATED appendix-a -->";
pub const BEGIN_B: &str = "<!-- BEGIN GENERATED appendix-b -->";
pub const END_B: &str = "<!-- END GENERATED appendix-b -->";

const PREAMBLE: &str = "(preamble)";

#[derive(Debug, Default)]
pub struct Report {
    pub errors: Vec<String>,
    /// The document with both generated appendices refreshed.
    pub regenerated: String,
    pub anchors: usize,
    pub links: usize,
    pub verify_items: usize,
}

/// All values of `<a id="..."></a>` on a line, in order.
fn anchors_in(line: &str) -> Vec<&str> {
    const OPEN: &str = "<a id=\"";
    let mut out = Vec::new();
    let mut rest = line;
    while let Some(i) = rest.find(OPEN) {
        rest = &rest[i + OPEN.len()..];
        match rest.find("\"></a>") {
            Some(j) => {
                out.push(&rest[..j]);
                rest = &rest[j..];
            }
            None => break,
        }
    }
    out
}

/// All links on a line as `(text, target)` for `[text](#target)`.
fn links_in(line: &str) -> Vec<(&str, &str)> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find("](#") {
        let mid = from + i;
        let start = line[..mid].rfind('[').map_or(mid, |s| s + 1);
        let tgt_start = mid + 3;
        let Some(len) = line[tgt_start..].find(')') else {
            break;
        };
        out.push((&line[start..mid], &line[tgt_start..tgt_start + len]));
        from = tgt_start + len;
    }
    out
}

fn is_id_text(text: &str) -> bool {
    text.len() > 2 && text.starts_with('`') && text.ends_with('`')
}

fn display(id: &str) -> String {
    id.to_ascii_uppercase()
}

fn link(id: &str) -> String {
    format!("[`{}`](#{})", display(id), id)
}

/// Replace ``[`ID`](#id)`` with `ID` so quoted text does not create new links.
fn strip_links(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(i) = rest.find("[`") {
        let after = &rest[i + 2..];
        if let Some(j) = after.find("`](#") {
            if let Some(k) = after[j..].find(')') {
                out.push_str(&rest[..i]);
                out.push_str(&after[..j]);
                rest = &after[j + k + 1..];
                continue;
            }
        }
        out.push_str(&rest[..i + 2]);
        rest = after;
    }
    out.push_str(rest);
    out
}

/// Innermost parenthesised text around each `VERIFY` on a line. A `VERIFY`
/// outside parentheses is prose about the marker, not an open item.
fn verify_items_in(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut from = 0;
    while let Some(i) = line[from..].find("VERIFY") {
        let pos = from + i;
        from = pos + "VERIFY".len();
        let mut open = None;
        let mut stack = Vec::new();
        for (idx, ch) in line[..pos].char_indices() {
            match ch {
                '(' => stack.push(idx),
                ')' => {
                    stack.pop();
                }
                _ => {}
            }
        }
        if let Some(&o) = stack.last() {
            open = Some(o);
        }
        let Some(o) = open else { continue };
        let mut depth = 0usize;
        let mut close = line.len();
        for (idx, ch) in line[pos..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' if depth == 0 => {
                    close = pos + idx;
                    break;
                }
                ')' => depth -= 1,
                _ => {}
            }
        }
        let item = strip_links(&line[o + 1..close]).replace('|', "\\|");
        if !out.contains(&item) {
            out.push(item);
        }
    }
    out
}

fn replace_block(doc: &str, begin: &str, end: &str, body: &str, errors: &mut Vec<String>) -> String {
    let (Some(b), Some(e)) = (doc.find(begin), doc.find(end)) else {
        errors.push(format!("missing generated-block markers: {begin}"));
        return doc.to_string();
    };
    if e < b {
        errors.push(format!("generated-block markers out of order: {begin}"));
        return doc.to_string();
    }
    format!("{}{}\n{}{}", &doc[..b], begin, body, &doc[e..])
}

pub fn check(doc: &str) -> Report {
    let mut rep = Report::default();

    // Pass 1: anchors (whole document).
    let mut anchors: BTreeSet<&str> = BTreeSet::new();
    for (n, line) in doc.lines().enumerate() {
        for a in anchors_in(line) {
            if !anchors.insert(a) {
                rep.errors.push(format!("line {}: duplicate anchor `{a}`", n + 1));
            }
            if a != a.to_ascii_lowercase() || a.contains(' ') {
                rep.errors
                    .push(format!("line {}: anchor `{a}` must be lower-case, no spaces", n + 1));
            }
        }
    }
    rep.anchors = anchors.len();

    // Pass 2: links, contexts, VERIFY items. The appendices are output, not input.
    let mut refs: BTreeMap<&str, BTreeSet<String>> = BTreeMap::new();
    let mut verify: Vec<(String, String)> = Vec::new();
    let mut heading_ctx: Vec<&str> = Vec::new();
    let mut in_fence = false;
    let mut in_appendix = false;
    for (n, line) in doc.lines().enumerate() {
        let here = anchors_in(line);
        if here.contains(&"appendix-a") {
            in_appendix = true;
        }
        if line.trim_start().starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if !in_fence && line.starts_with('#') && !here.is_empty() {
            heading_ctx = here.clone();
        }
        let ctx: Vec<&str> = if here.is_empty() { heading_ctx.clone() } else { here };

        let mut targets: Vec<&str> = Vec::new();
        if in_fence {
            // Diagrams and sketches name IDs as bare text.
            for tok in line.split(|c: char| !(c.is_ascii_uppercase() || c.is_ascii_digit() || c == '-')) {
                let lower = tok.to_ascii_lowercase();
                if let Some(a) = anchors.get(lower.as_str()) {
                    if tok.contains('-') {
                        targets.push(a);
                    }
                }
            }
        } else {
            for (text, target) in links_in(line) {
                rep.links += 1;
                let Some(a) = anchors.get(target) else {
                    rep.errors
                        .push(format!("line {}: link to missing anchor `#{target}`", n + 1));
                    continue;
                };
                if is_id_text(text) {
                    let named = text.trim_matches('`').to_ascii_lowercase();
                    if named != *a {
                        rep.errors
                            .push(format!("line {}: link text {text} points at `#{target}`", n + 1));
                    }
                    targets.push(a);
                }
            }
        }
        if in_appendix {
            continue;
        }
        for t in targets {
            let set = refs.entry(t).or_default();
            if ctx.is_empty() {
                set.insert(PREAMBLE.to_string());
            }
            for c in ctx.iter().filter(|c| **c != t) {
                set.insert((*c).to_string());
            }
        }
        if !in_fence {
            for item in verify_items_in(line) {
                let wher = ctx.iter().map(|c| link(c)).collect::<Vec<_>>().join(", ");
                verify.push((wher, item));
            }
        }
    }
    if in_fence {
        rep.errors.push("unterminated code fence".to_string());
    }
    rep.verify_items = verify.len();

    // Appendix A.
    let mut a = String::from("| ID | Kind | Referenced by |\n|---|---|---|\n");
    for id in anchors.iter().filter(|id| !id.starts_with("appendix-")) {
        let kind = display(id.split('-').next().unwrap_or(id));
        let by = match refs.get(id) {
            Some(set) if !set.is_empty() => set
                .iter()
                .map(|r| if r == PREAMBLE { r.clone() } else { link(r) })
                .collect::<Vec<_>>()
                .join(", "),
            _ => "(nothing; entry point or leaf)".to_string(),
        };
        a.push_str(&format!("| {} | {kind} | {by} |\n", link(id)));
    }

    // Appendix B.
    let mut b = String::from("| Where | Item |\n|---|---|\n");
    for (wher, item) in &verify {
        b.push_str(&format!("| {wher} | {item} |\n"));
    }

    let with_a = replace_block(doc, BEGIN_A, END_A, &a, &mut rep.errors);
    rep.regenerated = replace_block(&with_a, BEGIN_B, END_B, &b, &mut rep.errors);
    if rep.errors.is_empty() && rep.regenerated != doc {
        rep.errors
            .push("generated appendices are stale; run `cargo run -p link-check -- --write`".to_string());
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(body: &str) -> String {
        format!("{body}\n## <a id=\"appendix-a\"></a>A\n{BEGIN_A}\n{END_A}\n{BEGIN_B}\n{END_B}\n")
    }

    fn fresh(body: &str) -> Report {
        let first = check(&doc(body));
        check(&first.regenerated)
    }

    #[test]
    fn clean_document_passes_after_regeneration() {
        let r =
            fresh("## <a id=\"doc-x\"></a>X\nsee [`MOD-Y`](#mod-y)\n| <a id=\"mod-y\"></a> | y ([`DOC-X`](#doc-x)) |");
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert!(r
            .regenerated
            .contains("| [`MOD-Y`](#mod-y) | MOD | [`DOC-X`](#doc-x) |"));
        assert!(r
            .regenerated
            .contains("| [`DOC-X`](#doc-x) | DOC | [`MOD-Y`](#mod-y) |"));
    }

    #[test]
    fn missing_anchor_is_an_error() {
        let r = check(&doc("[`MOD-Z`](#mod-z)"));
        assert!(r.errors.iter().any(|e| e.contains("missing anchor `#mod-z`")));
    }

    #[test]
    fn duplicate_anchor_is_an_error() {
        let r = check(&doc("<a id=\"a-b\"></a>\n<a id=\"a-b\"></a>"));
        assert!(r.errors.iter().any(|e| e.contains("duplicate anchor")));
    }

    #[test]
    fn mismatched_link_text_is_an_error() {
        let r = check(&doc("<a id=\"a-b\"></a> <a id=\"a-c\"></a> [`A-B`](#a-c)"));
        assert!(r.errors.iter().any(|e| e.contains("points at")));
    }

    #[test]
    fn stale_appendix_is_an_error() {
        let r = check(&doc("## <a id=\"doc-x\"></a>X"));
        assert!(r.errors.iter().any(|e| e.contains("stale")));
    }

    #[test]
    fn verify_only_counts_inside_parentheses() {
        let r =
            fresh("## <a id=\"doc-x\"></a>X\n**VERIFY** marks a claim.\nfast (VERIFY by bench, [`DOC-X`](#doc-x)).");
        assert_eq!(r.verify_items, 1);
        assert!(r.regenerated.contains("| [`DOC-X`](#doc-x) | VERIFY by bench, DOC-X |"));
    }

    #[test]
    fn code_fences_count_bare_ids() {
        let r = fresh("## <a id=\"doc-x\"></a>X\n```\nbox MOD-Y here\n```\n### <a id=\"mod-y\"></a>Y");
        assert!(r
            .regenerated
            .contains("| [`MOD-Y`](#mod-y) | MOD | [`DOC-X`](#doc-x) |"));
    }
}
