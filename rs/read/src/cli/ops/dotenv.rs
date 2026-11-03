//! The deployed `.env` reader: a port of the python-dotenv 1.2.2 grammar that
//! the Python projection used (`dotenv_values(..., interpolate=False)`).
//!
//! Same bindings as Python for every line Python accepts: optional `export`,
//! quoted or unquoted keys, single-quoted values (only `\\` and `\'` escapes),
//! double-quoted values (`\\ \' \" \a \b \f \n \r \t \v`), unquoted values with
//! a ` # comment` removed, `KEY` without `=` (a key with no value), and the
//! last binding of a key wins. There is no `$VAR` expansion and no shell
//! evaluation; the `dotenvy` crate expands variables, so it is not used.
//!
//! One deliberate difference: Python logs and skips a line it cannot parse; this
//! reader refuses the whole file, because a skipped line could hide a key.
//!
//! The patterns are Python's. Python's `\s` also matches U+001C..U+001F, so
//! `SPACE` adds them; the `regex` crate's leftmost-first matching gives the
//! same match as Python's backtracking engine for these patterns.
use std::collections::BTreeMap;
use std::sync::LazyLock;

use regex::Regex;

/// Python `\s`.
const SPACE: &str = r"[\s\x1c-\x1f]";
/// Python `[^\S\r\n]`: whitespace other than a line break.
const INLINE_SPACE: &str = r"[[\s\x1c-\x1f]&&[^\r\n]]";

/// Compile one pattern anchored at the reader position.
fn anchored(pattern: &str) -> Regex {
    let pattern = pattern
        .replace("{SPACE}", SPACE)
        .replace("{INLINE_SPACE}", INLINE_SPACE);
    Regex::new(&format!("^(?:{pattern})")).expect("a valid dotenv pattern")
}

static MULTILINE_WHITESPACE: LazyLock<Regex> = LazyLock::new(|| anchored("{SPACE}*"));
static EXPORT: LazyLock<Regex> = LazyLock::new(|| anchored("(?:export{INLINE_SPACE}+)?"));
static SINGLE_QUOTED_KEY: LazyLock<Regex> = LazyLock::new(|| anchored("'([^']+)'"));
static UNQUOTED_KEY: LazyLock<Regex> = LazyLock::new(|| anchored(r"([^=\#\s\x1c-\x1f]+)"));
static WHITESPACE: LazyLock<Regex> = LazyLock::new(|| anchored("{INLINE_SPACE}*"));
static EQUAL_SIGN: LazyLock<Regex> = LazyLock::new(|| anchored("(={INLINE_SPACE}*)"));
static SINGLE_QUOTED_VALUE: LazyLock<Regex> = LazyLock::new(|| anchored(r"'((?:\\'|[^'])*)'"));
static DOUBLE_QUOTED_VALUE: LazyLock<Regex> = LazyLock::new(|| anchored(r#""((?:\\"|[^"])*)""#));
static UNQUOTED_VALUE: LazyLock<Regex> = LazyLock::new(|| anchored(r"([^\r\n]*)"));
static COMMENT: LazyLock<Regex> = LazyLock::new(|| anchored(r"(?:{INLINE_SPACE}*#[^\r\n]*)?"));
static END_OF_LINE: LazyLock<Regex> =
    LazyLock::new(|| anchored(r"{INLINE_SPACE}*(?:\r\n|\n|\r|$)"));
/// An inline comment of an unquoted value (Python `re.sub(r"\s+#.*", "", ...)`).
static INLINE_COMMENT: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(&format!("{SPACE}+#.*")).expect("a valid pattern"));
static DOUBLE_QUOTE_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"\\[\\'"abfnrtv]"#).expect("a valid pattern"));
static SINGLE_QUOTE_ESCAPE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\[\\']").expect("a valid pattern"));

/// The bindings of one file. `None` is a key written without `=`.
pub type Bindings = BTreeMap<String, Option<String>>;

/// A line python-dotenv would skip as invalid.
#[derive(Debug, PartialEq, Eq)]
pub struct InvalidLine;

/// Parse the whole file; the last binding of a key wins, as in Python.
pub fn parse(text: &str) -> Result<Bindings, InvalidLine> {
    let mut reader = Reader { text, position: 0 };
    let mut bindings = Bindings::new();
    while reader.has_next() {
        if let Some((key, value)) = parse_binding(&mut reader)? {
            bindings.insert(key, value);
        }
    }
    Ok(bindings)
}

struct Reader<'a> {
    text: &'a str,
    position: usize,
}

impl<'a> Reader<'a> {
    fn has_next(&self) -> bool {
        self.position < self.text.len()
    }

    fn peek(&self) -> Option<char> {
        self.text[self.position..].chars().next()
    }

    /// Match `pattern` at the position, advance past it and return group 1
    /// (empty when the pattern has no group).
    fn read(&mut self, pattern: &Regex) -> Result<&'a str, InvalidLine> {
        let rest = &self.text[self.position..];
        let captures = pattern.captures(rest).ok_or(InvalidLine)?;
        self.position += captures.get(0).map_or(0, |whole| whole.end());
        Ok(captures.get(1).map_or("", |group| group.as_str()))
    }
}

fn parse_binding(reader: &mut Reader<'_>) -> Result<Option<(String, Option<String>)>, InvalidLine> {
    reader.read(&MULTILINE_WHITESPACE)?;
    if !reader.has_next() {
        return Ok(None);
    }
    reader.read(&EXPORT)?;
    let key = parse_key(reader)?;
    reader.read(&WHITESPACE)?;
    let value = if reader.peek() == Some('=') {
        reader.read(&EQUAL_SIGN)?;
        Some(parse_value(reader)?)
    } else {
        None
    };
    reader.read(&COMMENT)?;
    reader.read(&END_OF_LINE)?;
    // A comment line has no key and binds nothing.
    Ok(key.map(|key| (key, value)))
}

fn parse_key(reader: &mut Reader<'_>) -> Result<Option<String>, InvalidLine> {
    match reader.peek() {
        Some('#') => Ok(None),
        Some('\'') => Ok(Some(reader.read(&SINGLE_QUOTED_KEY)?.to_owned())),
        _ => Ok(Some(reader.read(&UNQUOTED_KEY)?.to_owned())),
    }
}

fn parse_value(reader: &mut Reader<'_>) -> Result<String, InvalidLine> {
    match reader.peek() {
        Some('\'') => {
            let raw = reader.read(&SINGLE_QUOTED_VALUE)?;
            Ok(decode_escapes(&SINGLE_QUOTE_ESCAPE, raw))
        }
        Some('"') => {
            let raw = reader.read(&DOUBLE_QUOTED_VALUE)?;
            Ok(decode_escapes(&DOUBLE_QUOTE_ESCAPE, raw))
        }
        None | Some('\n') | Some('\r') => Ok(String::new()),
        Some(_) => {
            let raw = reader.read(&UNQUOTED_VALUE)?;
            let without_comment = INLINE_COMMENT.replace_all(raw, "");
            Ok(without_comment.trim_end_matches(is_python_space).to_owned())
        }
    }
}

/// Python's `codecs.decode(match, "unicode-escape")` for the escapes the two
/// escape patterns can match.
fn decode_escapes(pattern: &Regex, raw: &str) -> String {
    pattern
        .replace_all(raw, |captures: &regex::Captures<'_>| {
            match &captures[0][1..] {
                "a" => "\x07",
                "b" => "\x08",
                "f" => "\x0c",
                "n" => "\n",
                "r" => "\r",
                "t" => "\t",
                "v" => "\x0b",
                // `\\`, `\'` and `\"` decode to the escaped character.
                other => other,
            }
            .to_owned()
        })
        .into_owned()
}

/// Python's `str.isspace` for one character.
pub fn is_python_space(character: char) -> bool {
    character.is_whitespace() || ('\x1c'..='\x1f').contains(&character)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(text: &str, key: &str) -> Option<String> {
        parse(text).unwrap()[key].clone()
    }

    #[test]
    fn reads_the_python_dotenv_forms() {
        assert_eq!(value("A=plain\n", "A").as_deref(), Some("plain"));
        assert_eq!(
            value("export A = spaced  \n", "A").as_deref(),
            Some("spaced")
        );
        assert_eq!(value("A=x # note\n", "A").as_deref(), Some("x"));
        assert_eq!(
            value("A=x#not-a-comment\n", "A").as_deref(),
            Some("x#not-a-comment")
        );
        assert_eq!(
            value("A='a\\'b\\\\c\\n'\n", "A").as_deref(),
            Some("a'b\\c\\n")
        );
        assert_eq!(
            value("A=\"a\\\"b\\tc # d\"\n", "A").as_deref(),
            Some("a\"b\tc # d")
        );
        assert_eq!(value("A=$HOME\n", "A").as_deref(), Some("$HOME"));
        assert_eq!(value("A=\"${HOME}\"\n", "A").as_deref(), Some("${HOME}"));
        assert_eq!(value("'A B'=1\n", "A B").as_deref(), Some("1"));
        assert_eq!(value("A=\n", "A").as_deref(), Some(""));
        assert_eq!(value("A\n", "A"), None);
        assert_eq!(value("A=1\nA=2\n", "A").as_deref(), Some("2"));
        assert_eq!(
            value("A='multi\nline'\n", "A").as_deref(),
            Some("multi\nline")
        );
        // Python backtracks: a final `\'` closes the value.
        assert_eq!(value("A='abc\\'\n", "A").as_deref(), Some("abc\\"));
        assert_eq!(parse("# only a comment\n\n").unwrap(), Bindings::new());
    }

    #[test]
    fn refuses_a_line_python_would_skip() {
        assert_eq!(parse("=value\n"), Err(InvalidLine));
        assert_eq!(parse("A='unterminated\n"), Err(InvalidLine));
        assert_eq!(parse("A=\"x\" trailing\n"), Err(InvalidLine));
    }
}
