//! A tokenizer for syntax highlighting. It never fails: any text, however broken, becomes a list
//! of tokens that cover it exactly, in order, so an editor can colour a script while it is being
//! typed. It follows the language's lexical rules (comments, strings, templates, numbers,
//! keywords); it does not parse.

use std::collections::HashSet;
use std::sync::OnceLock;

use crate::docs::{API, command_script_names, name_of};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TokenKind {
    Keyword,
    /// `true`, `false`, `()`-like constants.
    Constant,
    Number,
    /// A string, a character or a backtick template.
    Text,
    Comment,
    /// A function or type of the script API (the names in the reference).
    Api,
    Ident,
    Punct,
    Space,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Token {
    pub kind: TokenKind,
    /// Byte range in the text.
    pub start: usize,
    pub end: usize,
}

const KEYWORDS: &[&str] = &[
    "let", "const", "if", "else", "switch", "do", "while", "until", "loop", "for", "in", "continue", "break", "return", "throw", "try", "catch", "fn",
    "private", "import", "export", "as", "global", "this",
];

fn api_names() -> &'static HashSet<String> {
    static NAMES: OnceLock<HashSet<String>> = OnceLock::new();
    NAMES.get_or_init(|| {
        let mut s: HashSet<String> = API.iter().map(|(_, sig, _)| name_of(sig).to_string()).collect();
        s.extend(command_script_names());
        s.insert("cmd".to_string());
        s
    })
}

fn is_ident_start(c: char) -> bool {
    c == '_' || c.is_alphabetic()
}

fn is_ident(c: char) -> bool {
    c == '_' || c.is_alphanumeric()
}

/// Tokens covering `text` exactly.
pub fn tokenize(text: &str) -> Vec<Token> {
    let b = text.as_bytes();
    let mut out: Vec<Token> = Vec::new();
    let mut i = 0usize;
    fn push(kind: TokenKind, start: usize, end: usize, out: &mut Vec<Token>) {
        if end > start {
            // Merge neighbours of the same quiet kind to keep the list short.
            if let Some(last) = out.last_mut()
                && last.kind == kind
                && last.end == start
                && matches!(kind, TokenKind::Space | TokenKind::Punct)
            {
                last.end = end;
                return;
            }
            out.push(Token { kind, start, end });
        }
    }
    while i < b.len() {
        let rest = &text[i..];
        let c = rest.chars().next().unwrap_or(' ');
        let start = i;
        if c.is_whitespace() {
            let n: usize = rest.chars().take_while(|c| c.is_whitespace()).map(char::len_utf8).sum();
            i += n;
            push(TokenKind::Space, start, i, &mut out);
        } else if rest.starts_with("//") {
            let n = rest.find('\n').unwrap_or(rest.len());
            i += n;
            push(TokenKind::Comment, start, i, &mut out);
        } else if rest.starts_with("/*") {
            // Block comments nest.
            let mut depth = 0usize;
            let mut j = 0usize;
            let rb = rest.as_bytes();
            while j < rb.len() {
                if rb[j..].starts_with(b"/*") {
                    depth += 1;
                    j += 2;
                } else if rb[j..].starts_with(b"*/") {
                    depth = depth.saturating_sub(1);
                    j += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    j += 1;
                }
            }
            i += j.min(rb.len());
            // `j` can stop inside a multi-byte character only after the loop's byte steps; move to a boundary.
            while i < b.len() && !text.is_char_boundary(i) {
                i += 1;
            }
            push(TokenKind::Comment, start, i, &mut out);
        } else if c == '"' || c == '`' || c == '\'' {
            // Quotes and characters end at the line (an unfinished one must not swallow the
            // file); a template runs until its closing backtick or the end of the text.
            let mut j = rest.len();
            let mut escaped = false;
            for (k, ch) in rest.char_indices().skip(1) {
                if escaped {
                    escaped = false;
                } else if ch == '\\' && c != '`' {
                    escaped = true;
                } else if ch == c {
                    j = k + ch.len_utf8();
                    break;
                } else if ch == '\n' && c != '`' {
                    j = k;
                    break;
                }
            }
            i += j;
            push(TokenKind::Text, start, i, &mut out);
        } else if c.is_ascii_digit() {
            let n = number_len(rest);
            i += n;
            push(TokenKind::Number, start, i, &mut out);
        } else if is_ident_start(c) {
            let n: usize = rest.chars().take_while(|c| is_ident(*c)).map(char::len_utf8).sum();
            let word = &rest[..n];
            i += n;
            let kind = if KEYWORDS.contains(&word) {
                TokenKind::Keyword
            } else if word == "true" || word == "false" {
                TokenKind::Constant
            } else if api_names().contains(word) {
                TokenKind::Api
            } else {
                TokenKind::Ident
            };
            push(kind, start, i, &mut out);
        } else {
            i += c.len_utf8();
            push(TokenKind::Punct, start, i, &mut out);
        }
    }
    out
}

fn number_len(s: &str) -> usize {
    let b = s.as_bytes();
    let mut i = 0;
    if b.len() > 1 && b[0] == b'0' && matches!(b[1], b'x' | b'X' | b'b' | b'B' | b'o' | b'O') {
        i = 2;
        while i < b.len() && (b[i].is_ascii_hexdigit() || b[i] == b'_') {
            i += 1;
        }
        return i;
    }
    while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
        i += 1;
    }
    // A fraction (`1.5`, not the range `1..5` or a method call `1.max(2)`).
    if i + 1 < b.len() && b[i] == b'.' && b[i + 1].is_ascii_digit() {
        i += 1;
        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'_') {
            i += 1;
        }
    }
    if i < b.len() && matches!(b[i], b'e' | b'E') {
        let mut j = i + 1;
        if j < b.len() && matches!(b[j], b'+' | b'-') {
            j += 1;
        }
        if j < b.len() && b[j].is_ascii_digit() {
            while j < b.len() && b[j].is_ascii_digit() {
                j += 1;
            }
            i = j;
        }
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(s: &str) -> Vec<(TokenKind, &str)> {
        tokenize(s).into_iter().filter(|t| t.kind != TokenKind::Space).map(|t| (t.kind, &s[t.start..t.end])).collect()
    }

    #[test]
    fn colours_the_parts_of_a_script() {
        use TokenKind::*;
        assert_eq!(
            kinds("let c = add_primitive(\"cube\", 1.5e3); // note"),
            vec![
                (Keyword, "let"),
                (Ident, "c"),
                (Punct, "="),
                (Api, "add_primitive"),
                (Punct, "("),
                (Text, "\"cube\""),
                (Punct, ","),
                (Number, "1.5e3"),
                (Punct, ");"),
                (Comment, "// note")
            ]
        );
        assert_eq!(kinds("cmd::undo()")[0], (Api, "cmd"));
        assert_eq!(kinds("x.vertex_count()")[2], (Api, "vertex_count"));
        assert_eq!(kinds("true false")[0].0, Constant);
        assert_eq!(kinds("0xFF 1_000 0b101"), vec![(Number, "0xFF"), (Number, "1_000"), (Number, "0b101")]);
        assert_eq!(kinds("0..5"), vec![(Number, "0"), (Punct, ".."), (Number, "5")]);
        assert_eq!(kinds("1.max(2)")[0], (Number, "1"));
        assert_eq!(kinds("/* a /* nested */ still */ x"), vec![(Comment, "/* a /* nested */ still */"), (Ident, "x")]);
        assert_eq!(kinds("`a ${b}\nc`"), vec![(Text, "`a ${b}\nc`")]);
        assert_eq!(kinds(r#""a\"b" c"#), vec![(Text, r#""a\"b""#), (Ident, "c")]);
    }

    #[test]
    fn unfinished_text_does_not_swallow_the_file() {
        use TokenKind::*;
        assert_eq!(kinds("\"open\nword"), vec![(Text, "\"open"), (Ident, "word")]);
        assert_eq!(kinds("/* never closed\nstill comment"), vec![(Comment, "/* never closed\nstill comment")]);
        assert_eq!(kinds("`never closed\nstill text"), vec![(Text, "`never closed\nstill text")]);
    }

    #[test]
    fn tokens_cover_any_text_exactly() {
        use proptest::prelude::*;
        let mut runner = proptest::test_runner::TestRunner::new(proptest::test_runner::Config { cases: 2000, ..Default::default() });
        runner
            .run(&"(?s).{0,120}", |s: String| {
                let t = tokenize(&s);
                let mut at = 0;
                for tok in &t {
                    prop_assert_eq!(tok.start, at);
                    prop_assert!(tok.end > tok.start);
                    prop_assert!(s.is_char_boundary(tok.start) && s.is_char_boundary(tok.end));
                    at = tok.end;
                }
                prop_assert_eq!(at, s.len());
                Ok(())
            })
            .unwrap();
    }
}
