//! Identifier and free-text tokenization for the FTS5 symbol index.
//!
//! SQLite's `unicode61` tokenizer treats `my_foo_bar` and `MyFooBar` as single
//! terms, so a query for `foo` never reaches them. The store therefore feeds a
//! dedicated split-token column built by [`split_ident`], and query strings are
//! expanded through the same function before they hit `MATCH`.

use unicode_segmentation::UnicodeSegmentation;

/// Character class used to find identifier word boundaries.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Lower,
    Upper,
    Digit,
    Other,
}

fn classify(c: char) -> Class {
    if c.is_lowercase() {
        Class::Lower
    } else if c.is_uppercase() {
        Class::Upper
    } else if c.is_numeric() {
        Class::Digit
    } else {
        Class::Other
    }
}

fn flush(current: &mut String, out: &mut Vec<String>) {
    if !current.is_empty() {
        out.push(current.to_lowercase());
        current.clear();
    }
}

/// Split one identifier into lowercase word tokens.
///
/// Handles `camelCase`/`PascalCase` transitions, `ACRONYMWord` runs,
/// `snake_case`/`kebab-case` separators, and digit/letter boundaries:
///
/// ```
/// # use pa_graph::tokenize::split_ident;
/// assert_eq!(split_ident("MyFooBar"), ["my", "foo", "bar"]);
/// assert_eq!(split_ident("my_foo_bar"), ["my", "foo", "bar"]);
/// assert_eq!(split_ident("HTTPRequest"), ["http", "request"]);
/// assert_eq!(split_ident("parse2Int"), ["parse", "2", "int"]);
/// ```
#[must_use]
pub fn split_ident(ident: &str) -> Vec<String> {
    let chars: Vec<char> = ident.chars().collect();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut prev: Option<Class> = None;

    for (i, &c) in chars.iter().enumerate() {
        let cls = classify(c);
        if cls == Class::Other {
            flush(&mut current, &mut out);
            prev = None;
            continue;
        }
        if let Some(p) = prev {
            let next = chars.get(i + 1).copied().map(classify);
            let boundary = match (p, cls) {
                // `HTTPRequest`: a run of capitals ends where the next
                // character drops back to lowercase.
                (Class::Upper, Class::Upper) => matches!(next, Some(Class::Lower)),
                (Class::Lower | Class::Digit, Class::Upper)
                | (Class::Lower, Class::Digit)
                | (Class::Digit, Class::Lower) => true,
                _ => false,
            };
            if boundary {
                flush(&mut current, &mut out);
            }
        }
        current.push(c);
        prev = Some(cls);
    }
    flush(&mut current, &mut out);
    out
}

/// Tokenize free text (docstrings, comments, prose) into lowercase words,
/// splitting any embedded identifiers along the way.
#[must_use]
pub fn tokenize(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for word in text.unicode_words() {
        out.extend(split_ident(word));
    }
    out
}

/// Render an identifier as a space-joined split-token string for the FTS
/// `tokens` column. The original identifier is included so exact-name lookups
/// keep working.
#[must_use]
pub fn split_for_fts(ident: &str) -> String {
    let mut parts = split_ident(ident);
    if !parts.iter().any(|p| p.eq_ignore_ascii_case(ident)) {
        parts.push(ident.to_lowercase());
    }
    parts.join(" ")
}

/// Build an FTS5 `MATCH` expression that ORs every split token.
///
/// Returns `None` when the query yields no tokens (an all-punctuation query).
#[must_use]
pub fn fts_match_expr(query: &str) -> Option<String> {
    let tokens = tokenize(query);
    if tokens.is_empty() {
        return None;
    }
    let mut seen = Vec::new();
    for token in tokens {
        if !seen.contains(&token) {
            seen.push(token);
        }
    }
    let terms: Vec<String> = seen.iter().map(|t| quote_fts_term(t)).collect();
    Some(terms.join(" OR "))
}

/// Quote a token as an FTS5 string literal (`"..."`, doubling inner quotes).
#[must_use]
pub fn quote_fts_term(term: &str) -> String {
    format!("\"{}\"", term.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_camel_and_pascal() {
        assert_eq!(split_ident("MyFooBar"), ["my", "foo", "bar"]);
        assert_eq!(split_ident("camelCase"), ["camel", "case"]);
        assert_eq!(split_ident("GraphStore"), ["graph", "store"]);
    }

    #[test]
    fn splits_snake_kebab_and_screaming() {
        assert_eq!(split_ident("my_foo_bar"), ["my", "foo", "bar"]);
        assert_eq!(split_ident("kebab-case-name"), ["kebab", "case", "name"]);
        assert_eq!(split_ident("MAX_RETRY_COUNT"), ["max", "retry", "count"]);
    }

    #[test]
    fn splits_acronym_runs_and_digits() {
        assert_eq!(split_ident("HTTPRequest"), ["http", "request"]);
        assert_eq!(split_ident("parse2Int"), ["parse", "2", "int"]);
        assert_eq!(split_ident("utf8Decode"), ["utf", "8", "decode"]);
    }

    #[test]
    fn handles_leading_underscores_and_empty() {
        assert_eq!(split_ident("__init__"), ["init"]);
        assert_eq!(split_ident("").len(), 0);
        assert_eq!(split_ident("___").len(), 0);
    }

    #[test]
    fn tokenizes_free_text() {
        assert_eq!(
            tokenize("Parse the config file"),
            ["parse", "the", "config", "file"]
        );
        assert_eq!(
            tokenize("calls parse_config now"),
            ["calls", "parse", "config", "now"]
        );
    }

    #[test]
    fn split_for_fts_keeps_original() {
        let s = split_for_fts("MyFooBar");
        assert!(s.contains("my") && s.contains("foo") && s.contains("bar"));
        assert!(s.contains("myfoobar"));
    }

    #[test]
    fn match_expr_ors_unique_tokens() {
        assert_eq!(
            fts_match_expr("MyFooBar foo"),
            Some("\"my\" OR \"foo\" OR \"bar\"".to_string())
        );
        assert_eq!(fts_match_expr("!!!"), None);
    }

    #[test]
    fn quotes_embedded_quotes() {
        assert_eq!(quote_fts_term("a\"b"), "\"a\"\"b\"");
    }
}
