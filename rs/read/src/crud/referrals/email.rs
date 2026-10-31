//! Compatibility port of pydantic's `EmailStr` validation.
//!
//! Source of truth: `pydantic.networks.validate_email` (pydantic 2.13.4) over
//! `email_validator` 2.3.0 with `check_deliverability=False`, and the `idna`
//! 3.18 package it calls. The reason strings are part of the public 422 body,
//! so every branch below keeps the original order and wording. The Python
//! oracle's vectors (`tests/rust_contract/phase2/referral-email-vectors.json`)
//! pin the behaviour; the Unicode and IDNA data are generated from the same
//! pinned interpreter by `scripts/rust_contract/referral_email_tables.py`.
//!
//! Name comparison with the source: `validate` is `validate_email`; the
//! `split_*`, `validate_*` and `check_*` functions keep the names of their
//! Python counterparts. IDNA labels are slices of code points because Python
//! strings, unlike Rust strings, can hold surrogates after Punycode decoding.

use std::cmp::Ordering;
use std::collections::BTreeSet;

use unicode_normalization::UnicodeNormalization;

use super::email_tables as tables;

/// pydantic `MAX_EMAIL_LENGTH`, counted in characters.
const MAX_INPUT_LENGTH: usize = 2048;
/// RFC 5321 address limit, counted in UTF-8 bytes.
const EMAIL_MAX_LENGTH: usize = 254;
const DOMAIN_MAX_LENGTH: usize = 253;
const DNS_LABEL_LENGTH_LIMIT: usize = 63;
/// The `idna` package refuses longer inputs before doing any work.
const IDNA_MAX_INPUT_LENGTH: usize = 1024;
const MAILBOX_NAMES: [&str; 15] = [
    "info",
    "marketing",
    "sales",
    "support",
    "abuse",
    "noc",
    "security",
    "postmaster",
    "hostmaster",
    "usenet",
    "news",
    "webmaster",
    "www",
    "uucp",
    "ftp",
];
const SPECIAL_USE_DOMAIN_NAMES: [&str; 6] =
    ["arpa", "invalid", "local", "localhost", "onion", "test"];
const ATEXT_PUNCTUATION: &str = "_!#$%&'*+-/=?^`{|}~";
const FULLWIDTH_AT: char = '\u{ff20}';
const SMALL_COMMERCIAL_AT: char = '\u{fe6b}';

type Reason = String;

// ---------------------------------------------------------------------------
// Unicode data access
// ---------------------------------------------------------------------------

fn compare_range(start: u32, end: u32, code: u32) -> Ordering {
    if code < start {
        Ordering::Greater
    } else if code > end {
        Ordering::Less
    } else {
        Ordering::Equal
    }
}

fn in_ranges(ranges: &[(u32, u32)], code: u32) -> bool {
    ranges
        .binary_search_by(|&(start, end)| compare_range(start, end, code))
        .is_ok()
}

fn is_surrogate(code: u32) -> bool {
    (0xd800..0xe000).contains(&code)
}

/// `unicodedata.category`, as the two-letter code.
fn category(code: u32) -> [u8; 2] {
    if is_surrogate(code) {
        return *b"Cs";
    }
    tables::GENERAL_CATEGORY
        .binary_search_by(|&(start, end, _)| compare_range(start, end, code))
        .map_or(*b"Cn", |index| tables::GENERAL_CATEGORY[index].2)
}

/// `unicodedata.name(c, None)` for the categories whose names are ever shown.
fn character_name(code: u32) -> Option<&'static str> {
    tables::CHARACTER_NAMES
        .binary_search_by_key(&code, |&(candidate, _)| candidate)
        .ok()
        .map(|index| tables::CHARACTER_NAMES[index].1)
}

/// `unicodedata.bidirectional`; the empty string means no class is assigned.
fn bidirectional(code: u32) -> &'static str {
    if is_surrogate(code) {
        return "";
    }
    tables::BIDIRECTIONAL
        .binary_search_by(|&(start, end, _)| compare_range(start, end, code))
        .map_or("", |index| {
            tables::BIDIRECTIONAL_CLASSES[usize::from(tables::BIDIRECTIONAL[index].2)]
        })
}

/// `str.isspace()`, which is also the meaning of `\s` and of `str.strip()`.
fn is_space(character: char) -> bool {
    in_ranges(tables::WHITESPACE, u32::from(character))
}

/// `\w` for `str` patterns: alphanumeric or underscore.
fn is_word(character: char) -> bool {
    character == '_' || in_ranges(tables::ALPHANUMERIC, u32::from(character))
}

fn strip(text: &str) -> &str {
    text.trim_matches(is_space)
}

fn code_points(text: &str) -> Vec<u32> {
    text.chars().map(u32::from).collect()
}

fn to_string(points: &[u32]) -> String {
    points
        .iter()
        .map(|&code| char::from_u32(code).expect("surrogates never reach a final string"))
        .collect()
}

fn nfc(text: &str) -> String {
    text.nfc().collect()
}

/// NFC over code points that may include surrogates. A surrogate is a starter
/// that never composes, so normalizing the runs between surrogates is exact.
fn nfc_points(points: &[u32]) -> Vec<u32> {
    let mut output = Vec::with_capacity(points.len());
    let mut run = String::new();
    for &code in points {
        match char::from_u32(code) {
            Some(character) => run.push(character),
            None => {
                output.extend(nfc(&run).chars().map(u32::from));
                run.clear();
                output.push(code);
            }
        }
    }
    output.extend(nfc(&run).chars().map(u32::from));
    output
}

/// Python `repr()` of a string: quote choice, escapes and printable test.
fn python_repr(points: &[u32]) -> String {
    let quote = if points.contains(&u32::from('\'')) && !points.contains(&u32::from('"')) {
        '"'
    } else {
        '\''
    };
    let mut output = String::from(quote);
    for &code in points {
        match code {
            0x5c => output.push_str("\\\\"),
            0x09 => output.push_str("\\t"),
            0x0a => output.push_str("\\n"),
            0x0d => output.push_str("\\r"),
            _ if code == u32::from(quote) => {
                output.push('\\');
                output.push(quote);
            }
            0x20..=0x7e => output.push(char::from_u32(code).expect("ascii")),
            _ if is_printable(code) => output.push(char::from_u32(code).expect("printable")),
            0..=0xff => output.push_str(&format!("\\x{code:02x}")),
            0x100..=0xffff => output.push_str(&format!("\\u{code:04x}")),
            _ => output.push_str(&format!("\\U{code:08x}")),
        }
    }
    output.push(quote);
    output
}

/// `str.isprintable()` for one character other than the ASCII space.
fn is_printable(code: u32) -> bool {
    matches!(category(code)[0], b'L' | b'M' | b'N' | b'P' | b'S')
}

/// `safe_character_display`: quoted printable characters, otherwise the
/// Unicode name or a hexadecimal fallback.
fn safe_character_display(character: char) -> String {
    if character == '\\' {
        return "\"\\\"".to_owned();
    }
    let code = u32::from(character);
    if matches!(category(code)[0], b'L' | b'N' | b'P' | b'S') {
        return python_repr(&[code]);
    }
    if let Some(name) = character_name(code) {
        return name.to_owned();
    }
    if code < 0xffff {
        format!("U+{code:04X}")
    } else {
        format!("U+{code:08X}")
    }
}

fn displays(characters: impl IntoIterator<Item = char>) -> String {
    let displayed: BTreeSet<String> = characters.into_iter().map(safe_character_display).collect();
    displayed.into_iter().collect::<Vec<_>>().join(", ")
}

// ---------------------------------------------------------------------------
// pydantic wrapper
// ---------------------------------------------------------------------------

/// `validate_email(value)[1]`: the normalized address, or the exact reason
/// pydantic puts into its `value_error`.
pub fn validate(value: &str) -> Result<String, Reason> {
    if value.chars().count() > MAX_INPUT_LENGTH {
        return Err(format!(
            "Length must not exceed {MAX_INPUT_LENGTH} characters"
        ));
    }
    validate_address(strip(pretty_address(value).unwrap_or(value)))
}

/// The address group of pydantic's `pretty_email_regex` when it matches the
/// whole value:
///
/// `\s*(?:((?:N+\s+)*N+)|"((?:[^"]|\")+)")?\s*<(.+)>\s*` with `N` the name
/// characters `[\w!#$%&'*+\-/=?^_`{|}~]`.
///
/// A display name is words of name characters, or a quoted run of any
/// characters (the escaped alternative `\"` makes a quote ordinary text). The
/// quoted run is greedy, so the last closing quote that lets the remainder
/// match is the one used.
fn pretty_address(value: &str) -> Option<&str> {
    let rest = value.trim_start_matches(is_space);
    if let Some(quoted) = rest.strip_prefix('"') {
        return quoted
            .rmatch_indices('"')
            .filter(|&(index, _)| index > 0)
            .find_map(|(index, _)| angle_address(&quoted[index + 1..]));
    }
    angle_address(skip_name_words(rest))
}

/// `\s*<(.+)>\s*` ending the text: the address runs from the first `<` to the
/// last `>`, must not span a line feed (`.` excludes `\n`), and only
/// whitespace may follow the `>`.
fn angle_address(text: &str) -> Option<&str> {
    let text = text.trim_start_matches(is_space).strip_prefix('<')?;
    let address = text.trim_end_matches(is_space).strip_suffix('>')?;
    (!address.is_empty() && !address.contains('\n')).then_some(address)
}

fn is_name_character(character: char) -> bool {
    is_word(character) || "!#$%&'*+-/=?^`{|}~".contains(character)
}

/// Skip `(?:N+\s+)*N+`: words of name characters separated by whitespace.
fn skip_name_words(text: &str) -> &str {
    let mut position = 0;
    loop {
        let word_end = text[position..]
            .find(|character| !is_name_character(character))
            .map_or(text.len(), |offset| position + offset);
        if word_end == position {
            return &text[position..];
        }
        position = word_end;
        let after_space = text[position..].trim_start_matches(is_space);
        let space_length = text.len() - position - after_space.len();
        if space_length > 0 && after_space.starts_with(is_name_character) {
            position += space_length;
        } else {
            return &text[position..];
        }
    }
}

// ---------------------------------------------------------------------------
// email_validator: splitting
// ---------------------------------------------------------------------------

struct Split {
    has_display_name: bool,
    local: String,
    domain: String,
    quoted: bool,
}

/// `unicodedata.normalize("NFC", text[i:])[0] != c` is false: the character
/// still starts the normalized text, so it keeps its own meaning.
fn keeps_meaning(text: &[char]) -> bool {
    let first = text[0];
    if first.is_ascii() && text.get(1).is_none_or(char::is_ascii) {
        return true;
    }
    text.iter().collect::<String>().nfc().next() == Some(first)
}

/// Index of the first unquoted special character, which is not followed by a
/// combining character. `Err` carries the reason when there is none.
fn split_at_unquoted_special(text: &[char], specials: &[char]) -> Result<usize, Reason> {
    let mut inside_quote = false;
    let mut escaped = false;
    for (index, &character) in text.iter().enumerate() {
        if !keeps_meaning(&text[index..]) {
            continue;
        }
        if inside_quote {
            if character == '\\' && !escaped {
                escaped = true;
            } else if character == '"' && !escaped {
                inside_quote = false;
                escaped = false;
            } else {
                escaped = false;
            }
        } else if character == '"' {
            inside_quote = true;
        } else if specials.contains(&character) {
            return Ok(index);
        }
    }
    if text.contains(&FULLWIDTH_AT) {
        return Err(
            "The email address has the \"full-width\" at-sign (@) character instead of a regular at-sign."
                .to_owned(),
        );
    }
    if text.contains(&SMALL_COMMERCIAL_AT) {
        return Err(
            "The email address has the \"small commercial at\" character instead of a regular at-sign."
                .to_owned(),
        );
    }
    Err("An email address must have an @-sign.".to_owned())
}

/// Remove surrounding quotes and unescape. Returns the value and whether the
/// text was quoted.
fn unquote_quoted_string(text: &[char]) -> Result<(String, bool), Reason> {
    let mut quoted = false;
    let mut escaped = false;
    let mut value = String::new();
    for (index, &character) in text.iter().enumerate() {
        if quoted {
            if escaped {
                value.push(character);
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                if index != text.len() - 1 {
                    let extra: Vec<String> = text[index + 1..]
                        .iter()
                        .map(|&c| safe_character_display(c))
                        .collect();
                    return Err(format!(
                        "Extra character(s) found after close quote: {}",
                        extra.join(", ")
                    ));
                }
                break;
            } else {
                value.push(character);
            }
        } else if index == 0 && character == '"' {
            quoted = true;
        } else {
            value.push(character);
        }
    }
    Ok((value, quoted))
}

fn is_atext(character: char) -> bool {
    character.is_ascii_alphanumeric() || ATEXT_PUNCTUATION.contains(character)
}

fn split_email(email: &str) -> Result<Split, Reason> {
    let text: Vec<char> = email.chars().collect();
    let left_length = split_at_unquoted_special(&text, &['@', '<'])?;
    let (left, right) = text.split_at(left_length);
    let (has_display_name, local, domain) = if right.first() == Some(&'<') {
        let left_end = left
            .iter()
            .rposition(|&character| !is_space(character))
            .map_or(0, |index| index + 1);
        let (display_name, display_name_quoted) = unquote_quoted_string(&left[..left_end])?;
        if !display_name_quoted {
            let invalid = displays(display_name.chars().filter(|&character| {
                character == '.' || (!is_atext(character) && character != ' ')
            }));
            if !invalid.is_empty() {
                return Err(format!(
                    "The display name contains invalid characters when not quoted: {invalid}."
                ));
            }
        }
        check_unsafe_chars(&display_name, true)?;
        if !right.contains(&'>') {
            return Err("An open angle bracket at the start of the email address has to be followed by a close angle bracket at the end.".to_owned());
        }
        let right_end = right
            .iter()
            .rposition(|&character| character != ' ')
            .map_or(0, |index| index + 1);
        let right = &right[..right_end];
        if right.last() != Some(&'>') {
            return Err("There can't be anything after the email address.".to_owned());
        }
        let address = &right[1..];
        let address_end = address
            .iter()
            .rposition(|&character| character != '>')
            .map_or(0, |index| index + 1);
        let address = &address[..address_end];
        let local_length = split_at_unquoted_special(address, &['@'])?;
        (
            true,
            address[..local_length].to_vec(),
            address[local_length..].to_vec(),
        )
    } else {
        (false, left.to_vec(), right.to_vec())
    };
    let domain = domain.strip_prefix(&['@']).unwrap_or(&domain[..]);
    let (local, quoted) = unquote_quoted_string(&local)?;
    Ok(Split {
        has_display_name,
        local,
        domain: domain.iter().collect(),
        quoted,
    })
}

// ---------------------------------------------------------------------------
// email_validator: local part
// ---------------------------------------------------------------------------

struct LocalPart {
    local: String,
    ascii: Option<String>,
}

fn length_reason(length: usize, limit: usize) -> String {
    let difference = length - limit;
    let suffix = if difference > 1 { "s" } else { "" };
    format!("({difference} character{suffix} too many)")
}

/// `check_unsafe_chars`: letters, numbers, punctuation and symbols are safe;
/// a leading mark, separators, and every control, format, private or
/// unassigned code point are not.
fn check_unsafe_chars(text: &str, allow_space: bool) -> Result<(), Reason> {
    let mut unsafe_characters = BTreeSet::new();
    for (index, character) in text.chars().enumerate() {
        let class = category(u32::from(character));
        let allowed = match class[0] {
            b'L' | b'N' | b'P' | b'S' => true,
            b'M' => index != 0,
            b'Z' => &class == b"Zs" && allow_space,
            _ => false,
        };
        if !allowed {
            unsafe_characters.insert(character);
        }
    }
    if unsafe_characters.is_empty() {
        return Ok(());
    }
    let shown: Vec<String> = unsafe_characters
        .into_iter()
        .map(safe_character_display)
        .collect();
    Err(format!(
        "The email address contains unsafe characters: {}.",
        shown.join(", ")
    ))
}

/// `check_dot_atom`: dots (and, for host names, hyphens) in forbidden places.
fn check_dot_atom(
    label: &str,
    start: &dyn Fn(&str) -> String,
    end: &dyn Fn(&str) -> String,
    is_hostname: bool,
) -> Result<(), Reason> {
    if label.ends_with('.') {
        return Err(end("period"));
    }
    if label.starts_with('.') {
        return Err(start("period"));
    }
    if label.contains("..") {
        return Err("An email address cannot have two periods in a row.".to_owned());
    }
    if is_hostname {
        if label.ends_with('-') {
            return Err(end("hyphen"));
        }
        if label.starts_with('-') {
            return Err(start("hyphen"));
        }
        if label.contains(".-") || label.contains("-.") {
            return Err(
                "An email address cannot have a period and a hyphen next to each other.".to_owned(),
            );
        }
    }
    Ok(())
}

/// `[X]+(?:\.[X]+)*\Z`: non-empty dot-separated runs of allowed characters.
fn is_dot_atom(text: &str, allowed: impl Fn(char) -> bool) -> bool {
    text.split('.')
        .all(|part| !part.is_empty() && part.chars().all(&allowed))
}

fn validate_local_part(local: &str, quoted: bool) -> Result<LocalPart, Reason> {
    if local.is_empty() {
        return Err("There must be something before the @-sign.".to_owned());
    }
    if is_dot_atom(local, is_atext) {
        return Ok(LocalPart {
            local: local.to_owned(),
            ascii: Some(local.to_owned()),
        });
    }
    let mut valid_form = None;
    let mut requires_smtputf8 = false;
    if is_dot_atom(local, |character| {
        is_atext(character) || u32::from(character) >= 0x80
    }) {
        valid_form = Some("dot-atom");
        requires_smtputf8 = true;
    } else if quoted {
        let invalid = displays(
            local
                .chars()
                .filter(|&c| !matches!(u32::from(c), 0x20..=0x7e | 0x80..)),
        );
        if !invalid.is_empty() {
            return Err(format!(
                "The email address contains invalid characters in quotes before the @-sign: {invalid}."
            ));
        }
        requires_smtputf8 = local
            .chars()
            .any(|character| !matches!(u32::from(character), 32..=126));
        valid_form = Some("quoted");
    }
    if let Some(form) = valid_form {
        check_unsafe_chars(local, form == "quoted")?;
        let mut local = local.to_owned();
        if form == "quoted" {
            let escaped: String = local
                .chars()
                .flat_map(|character| {
                    let prefix = matches!(character, '"' | '\\').then_some('\\');
                    prefix.into_iter().chain(std::iter::once(character))
                })
                .collect();
            local = format!("\"{escaped}\"");
        }
        return Ok(LocalPart {
            ascii: (!requires_smtputf8).then(|| local.clone()),
            local,
        });
    }
    let invalid = displays(local.chars().filter(|&character| {
        !(character == '.' || is_atext(character) || u32::from(character) >= 0x80)
    }));
    if !invalid.is_empty() {
        return Err(format!(
            "The email address contains invalid characters before the @-sign: {invalid}."
        ));
    }
    check_dot_atom(
        local,
        &|descriptor| format!("An email address cannot start with a {descriptor}."),
        &|descriptor| {
            format!("An email address cannot have a {descriptor} immediately before the @-sign.")
        },
        false,
    )?;
    Err("The email address contains invalid characters before the @-sign.".to_owned())
}

// ---------------------------------------------------------------------------
// email_validator: domain literals (Python `ipaddress` diagnostics)
// ---------------------------------------------------------------------------

fn repr(text: &str) -> String {
    python_repr(&code_points(text))
}

/// `IPv4Address(text)`; the error is the `AddressValueError` text.
fn parse_ipv4(text: &str) -> Result<(), Reason> {
    if text.contains('/') {
        return Err(format!("Unexpected '/' in {}", repr(text)));
    }
    if text.is_empty() {
        return Err("Address cannot be empty".to_owned());
    }
    let octets: Vec<&str> = text.split('.').collect();
    if octets.len() != 4 {
        return Err(format!("Expected 4 octets in {}", repr(text)));
    }
    for octet in octets {
        parse_octet(octet).map_err(|reason| format!("{reason} in {}", repr(text)))?;
    }
    Ok(())
}

fn parse_octet(octet: &str) -> Result<(), Reason> {
    if octet.is_empty() {
        return Err("Empty octet not permitted".to_owned());
    }
    if !octet.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!("Only decimal digits permitted in {}", repr(octet)));
    }
    if octet.len() > 3 {
        return Err(format!("At most 3 characters permitted in {}", repr(octet)));
    }
    if octet != "0" && octet.starts_with('0') {
        return Err(format!(
            "Leading zeros are not permitted in {}",
            repr(octet)
        ));
    }
    let value: u32 = octet.parse().expect("three ascii digits");
    if value > 255 {
        return Err(format!("Octet {value} (> 255) not permitted"));
    }
    Ok(())
}

fn parse_hextet(hextet: &str) -> Result<(), Reason> {
    if !hextet.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("Only hex digits permitted in {}", repr(hextet)));
    }
    if hextet.len() > 4 {
        return Err(format!(
            "At most 4 characters permitted in {}",
            repr(hextet)
        ));
    }
    Ok(())
}

/// `IPv6Address(text)`, including the scope-id split; error is the
/// `AddressValueError` text.
fn parse_ipv6(text: &str) -> Result<(), Reason> {
    if text.contains('/') {
        return Err(format!("Unexpected '/' in {}", repr(text)));
    }
    let address = match text.split_once('%') {
        None => text,
        Some((address, scope)) => {
            if scope.is_empty() || scope.contains('%') {
                return Err(format!("Invalid IPv6 address: \"{}\"", repr(text)));
            }
            address
        }
    };
    ipv6_integer(address)
}

fn ipv6_integer(text: &str) -> Result<(), Reason> {
    const HEXTET_COUNT: usize = 8;
    if text.is_empty() {
        return Err("Address cannot be empty".to_owned());
    }
    let mut parts: Vec<String> = text.split(':').map(str::to_owned).collect();
    if parts.len() < 3 {
        return Err(format!("At least 3 parts expected in {}", repr(text)));
    }
    if parts.last().is_some_and(|part| part.contains('.')) {
        let suffix = parts.pop().expect("checked above");
        parse_ipv4(&suffix).map_err(|reason| format!("{reason} in {}", repr(text)))?;
        parts.push("0".to_owned());
        parts.push("0".to_owned());
    }
    if parts.len() > HEXTET_COUNT + 1 {
        return Err(format!(
            "At most {} colons permitted in {}",
            HEXTET_COUNT,
            repr(text)
        ));
    }
    let mut skip_index = None;
    // Only the interior parts can be an empty run between two colons.
    for (offset, part) in parts[1..parts.len() - 1].iter().enumerate() {
        if part.is_empty() {
            if skip_index.is_some() {
                return Err(format!("At most one '::' permitted in {}", repr(text)));
            }
            skip_index = Some(offset + 1);
        }
    }
    let leading = format!(
        "Leading ':' only permitted as part of '::' in {}",
        repr(text)
    );
    let trailing = format!(
        "Trailing ':' only permitted as part of '::' in {}",
        repr(text)
    );
    let (parts_hi, parts_lo);
    if let Some(skip) = skip_index {
        let mut hi = skip;
        let mut lo = parts.len() - skip - 1;
        if parts[0].is_empty() {
            hi -= 1;
            if hi > 0 {
                return Err(leading);
            }
        }
        if parts[parts.len() - 1].is_empty() {
            lo -= 1;
            if lo > 0 {
                return Err(trailing);
            }
        }
        if hi + lo >= HEXTET_COUNT {
            return Err(format!(
                "Expected at most {} other parts with '::' in {}",
                HEXTET_COUNT - 1,
                repr(text)
            ));
        }
        (parts_hi, parts_lo) = (hi, lo);
    } else {
        if parts.len() != HEXTET_COUNT {
            return Err(format!(
                "Exactly {HEXTET_COUNT} parts expected without '::' in {}",
                repr(text)
            ));
        }
        if parts[0].is_empty() {
            return Err(leading);
        }
        if parts[parts.len() - 1].is_empty() {
            return Err(trailing);
        }
        (parts_hi, parts_lo) = (parts.len(), 0);
    }
    let selected = parts[..parts_hi]
        .iter()
        .chain(parts[parts.len() - parts_lo..].iter());
    for part in selected {
        parse_hextet(part).map_err(|reason| format!("{reason} in {}", repr(text)))?;
    }
    Ok(())
}

/// `validate_email_domain_literal`: always an error here, because this
/// validator never allows bracketed addresses.
fn validate_domain_literal(literal: &str) -> Result<(), Reason> {
    let digits_and_dots = literal.strip_suffix('\n').unwrap_or(literal);
    if !digits_and_dots.is_empty()
        && digits_and_dots
            .bytes()
            .all(|byte| byte.is_ascii_digit() || byte == b'.')
    {
        return parse_ipv4(literal).map_err(|reason| {
            format!(
                "The address in brackets after the @-sign is not valid: It is not an IPv4 address ({reason}) or is missing an address literal tag."
            )
        });
    }
    if let Some(address) = literal.strip_prefix("IPv6:") {
        return parse_ipv6(address).map_err(|reason| {
            format!("The IPv6 address in brackets after the @-sign is not valid ({reason}).")
        });
    }
    if !literal.contains(':') {
        return Err(
            "The part after the @-sign in brackets is not an IPv4 address and has no address literal tag."
                .to_owned(),
        );
    }
    let invalid = displays(literal.chars().filter(|&character| {
        // email-validator's class is [\u0021-\u00FA\u005E-\u007E]; the second range lies inside the first.
        !matches!(u32::from(character), 0x21..=0xfa)
    }));
    if !invalid.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters in brackets: {invalid}."
        ));
    }
    Err("The part after the @-sign contains an invalid address literal tag in brackets.".to_owned())
}

// ---------------------------------------------------------------------------
// idna 3.18
// ---------------------------------------------------------------------------

fn unot(code: u32) -> String {
    format!("U+{code:04X}")
}

fn punycode_adapt(delta: u128, first: bool, count: usize) -> u128 {
    let mut delta = if first { delta / 700 } else { delta / 2 };
    delta += delta / count as u128;
    let mut divisions = 0;
    while delta > 455 {
        delta /= 35;
        divisions += 36;
    }
    divisions + (36 * delta / (delta + 38))
}

fn punycode_threshold(index: u128, bias: u128) -> u128 {
    (36 * (index + 1)).saturating_sub(bias).clamp(1, 26)
}

/// RFC 3492 encoding, as `str.encode("punycode")`.
fn punycode_encode(label: &[u32]) -> String {
    let mut output: String = label
        .iter()
        .filter(|&&code| code < 0x80)
        .map(|&code| char::from_u32(code).expect("ascii"))
        .collect();
    let basic = output.chars().count();
    if basic > 0 {
        output.push('-');
    }
    let (mut n, mut delta, mut bias, mut handled) = (0x80_u128, 0_u128, 72_u128, basic);
    while handled < label.len() {
        let next = label
            .iter()
            .map(|&code| u128::from(code))
            .filter(|&code| code >= n)
            .min()
            .expect("an unhandled code point remains");
        delta += (next - n) * (handled as u128 + 1);
        n = next;
        for &code in label {
            let code = u128::from(code);
            if code < n {
                delta += 1;
            } else if code == n {
                let mut remaining = delta;
                let mut index = 0;
                loop {
                    let threshold = punycode_threshold(index, bias);
                    if remaining < threshold {
                        break;
                    }
                    output.push(punycode_digit(
                        threshold + (remaining - threshold) % (36 - threshold),
                    ));
                    remaining = (remaining - threshold) / (36 - threshold);
                    index += 1;
                }
                output.push(punycode_digit(remaining));
                bias = punycode_adapt(delta, handled == basic, handled + 1);
                delta = 0;
                handled += 1;
            }
        }
        delta += 1;
        n += 1;
    }
    output
}

fn punycode_digit(value: u128) -> char {
    let value = u8::try_from(value).expect("a punycode digit is below 36");
    char::from(if value < 26 {
        b'a' + value
    } else {
        b'0' + value - 26
    })
}

/// CPython's Punycode decoder in strict mode; `Err(())` is any `UnicodeError`.
fn punycode_decode(text: &[u8]) -> Result<Vec<u32>, ()> {
    let (mut base, extended): (Vec<u32>, Vec<u8>) =
        match text.iter().rposition(|&byte| byte == b'-') {
            None => (Vec::new(), text.to_ascii_uppercase()),
            Some(position) => (
                text[..position]
                    .iter()
                    .map(|&byte| u32::from(byte))
                    .collect(),
                text[position + 1..].to_ascii_uppercase(),
            ),
        };
    let (mut character, mut position, mut bias, mut extended_position) =
        (0x80_u128, -1_i128, 72_u128, 0_usize);
    while extended_position < extended.len() {
        let (new_position, delta) = decode_generalized_number(&extended, extended_position, bias)?;
        position =
            position.saturating_add(i128::try_from(delta).unwrap_or(i128::MAX).saturating_add(1));
        let length = base.len() as i128 + 1;
        character = character.saturating_add((position / length) as u128);
        if character > 0x10ffff {
            return Err(());
        }
        position %= length;
        let insert_at = usize::try_from(position).expect("remainder is small");
        base.insert(insert_at, u32::try_from(character).expect("checked above"));
        bias = punycode_adapt(delta, extended_position == 0, base.len());
        extended_position = new_position;
    }
    Ok(base)
}

fn decode_generalized_number(
    extended: &[u8],
    mut position: usize,
    bias: u128,
) -> Result<(usize, u128), ()> {
    let (mut result, mut weight, mut index) = (0_u128, 1_u128, 0_u128);
    loop {
        let byte = *extended.get(position).ok_or(())?;
        position += 1;
        let digit = match byte {
            b'A'..=b'Z' => u128::from(byte - b'A'),
            b'0'..=b'9' => u128::from(byte) - 22,
            _ => return Err(()),
        };
        let threshold = punycode_threshold(index, bias);
        result = result.saturating_add(digit.saturating_mul(weight));
        if digit < threshold {
            return Ok((position, result));
        }
        weight = weight.saturating_mul(36 - threshold);
        index += 1;
    }
}

fn joining_type(code: u32) -> Option<char> {
    tables::JOINING_TYPES
        .iter()
        .find(|(_, ranges)| in_ranges(ranges, code))
        .map(|&(kind, _)| kind)
}

/// `_combining_class(code) == 9`, where an unknown character is an error.
fn is_virama(code: u32) -> Result<bool, ()> {
    if is_surrogate(code) || in_ranges(tables::UNNAMED, code) {
        return Err(());
    }
    Ok(tables::VIRAMA.binary_search(&code).is_ok())
}

fn valid_contextj(label: &[u32], position: usize) -> Result<bool, ()> {
    match label[position] {
        0x200c => {
            if position > 0 && is_virama(label[position - 1])? {
                return Ok(true);
            }
            let mut joined = false;
            for &code in label[..position].iter().rev() {
                match joining_type(code) {
                    Some('T') => continue,
                    Some('L' | 'D') => joined = true,
                    _ => {}
                }
                break;
            }
            if !joined {
                return Ok(false);
            }
            for &code in &label[position + 1..] {
                match joining_type(code) {
                    Some('T') => continue,
                    Some('R' | 'D') => return Ok(true),
                    _ => {}
                }
                break;
            }
            Ok(false)
        }
        0x200d => Ok(position > 0 && is_virama(label[position - 1])?),
        _ => Ok(false),
    }
}

fn valid_contexto(label: &[u32], position: usize) -> bool {
    let code = label[position];
    let in_script = |code: u32, ranges: &[(u32, u32)]| in_ranges(ranges, code);
    match code {
        0xb7 => {
            0 < position
                && position + 1 < label.len()
                && label[position - 1] == 0x6c
                && label[position + 1] == 0x6c
        }
        0x375 => {
            position + 1 < label.len()
                && label.len() > 1
                && in_script(label[position + 1], tables::SCRIPT_GREEK)
        }
        0x5f3 | 0x5f4 => position > 0 && in_script(label[position - 1], tables::SCRIPT_HEBREW),
        0x30fb => label.iter().any(|&other| {
            other != 0x30fb
                && (in_script(other, tables::SCRIPT_HIRAGANA)
                    || in_script(other, tables::SCRIPT_KATAKANA)
                    || in_script(other, tables::SCRIPT_HAN))
        }),
        0x660..=0x669 => !label.iter().any(|other| (0x6f0..=0x6f9).contains(other)),
        0x6f0..=0x6f9 => !label.iter().any(|other| (0x660..=0x669).contains(other)),
        _ => false,
    }
}

/// RFC 5893 Bidi Rule for a label that holds a right-to-left character.
fn check_bidi(label: &[u32]) -> Result<(), String> {
    if label.len() > IDNA_MAX_INPUT_LENGTH {
        return Err("Label too long".to_owned());
    }
    let mut bidi_label = false;
    for (index, &code) in label.iter().enumerate() {
        let direction = bidirectional(code);
        if direction.is_empty() {
            return Err(format!(
                "Unknown directionality in label {} at position {}",
                python_repr(label),
                index + 1
            ));
        }
        bidi_label |= matches!(direction, "R" | "AL" | "AN");
    }
    if !bidi_label {
        return Ok(());
    }
    let rtl = match bidirectional(label[0]) {
        "R" | "AL" => true,
        "L" => false,
        _ => {
            return Err(format!(
                "First codepoint in label {} must be directionality L, R or AL",
                python_repr(label)
            ));
        }
    };
    let mut valid_ending = false;
    let mut number_type: Option<&str> = None;
    for (index, &code) in label.iter().enumerate() {
        let direction = bidirectional(code);
        if rtl {
            if !matches!(
                direction,
                "R" | "AL" | "AN" | "EN" | "ES" | "CS" | "ET" | "ON" | "BN" | "NSM"
            ) {
                return Err(format!(
                    "Invalid direction for codepoint at position {} in a right-to-left label",
                    index + 1
                ));
            }
            if matches!(direction, "R" | "AL" | "EN" | "AN") {
                valid_ending = true;
            } else if direction != "NSM" {
                valid_ending = false;
            }
            if matches!(direction, "AN" | "EN") {
                match number_type {
                    None => number_type = Some(direction),
                    Some(existing) if existing != direction => {
                        return Err("Can not mix numeral types in a right-to-left label".to_owned());
                    }
                    Some(_) => {}
                }
            }
        } else {
            if !matches!(
                direction,
                "L" | "EN" | "ES" | "CS" | "ET" | "ON" | "BN" | "NSM"
            ) {
                return Err(format!(
                    "Invalid direction for codepoint at position {} in a left-to-right label",
                    index + 1
                ));
            }
            if matches!(direction, "L" | "EN") {
                valid_ending = true;
            } else if direction != "NSM" {
                valid_ending = false;
            }
        }
    }
    if valid_ending {
        Ok(())
    } else {
        Err("Label ends with illegal codepoint directionality".to_owned())
    }
}

/// `idna.check_label`: the full IDNA 2008 validity checks for one label.
fn check_label(label: &[u32]) -> Result<(), String> {
    if label.len() > IDNA_MAX_INPUT_LENGTH {
        return Err("Label too long".to_owned());
    }
    if label.is_empty() {
        return Err("Empty Label".to_owned());
    }
    if label.len() > 254 {
        return Err("Label too long".to_owned());
    }
    if nfc_points(label) != label {
        return Err("Label must be in Normalization Form C".to_owned());
    }
    if label.get(2..4) == Some(&[0x2d, 0x2d][..]) {
        return Err("Label has disallowed hyphens in 3rd and 4th position".to_owned());
    }
    if label[0] == 0x2d || label[label.len() - 1] == 0x2d {
        return Err("Label must not start or end with a hyphen".to_owned());
    }
    if category(label[0])[0] == b'M' {
        return Err("Label begins with an illegal combining character".to_owned());
    }
    for (position, &code) in label.iter().enumerate() {
        if in_ranges(tables::IDNA_PVALID, code) {
            continue;
        }
        if in_ranges(tables::IDNA_CONTEXTJ, code) {
            // idna 3.18 raises `InvalidCodepointContext` inside a `try` that
            // catches `ValueError`, which it subclasses. Every refused joiner
            // therefore reports the "unknown codepoint" text.
            if !matches!(valid_contextj(label, position), Ok(true)) {
                return Err(format!(
                    "Unknown codepoint adjacent to joiner {} at position {} in {}",
                    unot(code),
                    position + 1,
                    python_repr(label)
                ));
            }
        } else if in_ranges(tables::IDNA_CONTEXTO, code) {
            if !valid_contexto(label, position) {
                return Err(format!(
                    "Codepoint {} not allowed at position {} in {}",
                    unot(code),
                    position + 1,
                    python_repr(label)
                ));
            }
        } else {
            return Err(format!(
                "Codepoint {} at position {} of {} not allowed",
                unot(code),
                position + 1,
                python_repr(label)
            ));
        }
    }
    check_bidi(label)
}

/// `idna.ulabel` for an ASCII label: lower-case, decode A-labels, validate.
fn ulabel_ascii(label: &str) -> Result<Vec<u32>, String> {
    if label.len() > IDNA_MAX_INPUT_LENGTH {
        return Err("Label too long".to_owned());
    }
    let lowered = label.to_ascii_lowercase();
    if let Some(encoded) = lowered.strip_prefix("xn--") {
        if encoded.is_empty() {
            return Err("Malformed A-label, no Punycode eligible content found".to_owned());
        }
        if encoded.ends_with('-') {
            return Err("A-label must not end with a hyphen".to_owned());
        }
        let decoded =
            punycode_decode(encoded.as_bytes()).map_err(|()| "Invalid A-label".to_owned())?;
        check_label(&decoded)?;
        return Ok(decoded);
    }
    let points = code_points(&lowered);
    check_label(&points)?;
    Ok(points)
}

/// `idna.alabel`: validate and convert one label to its ASCII form.
fn alabel(label: &[u32]) -> Result<String, String> {
    if label.len() > IDNA_MAX_INPUT_LENGTH {
        return Err("Label too long".to_owned());
    }
    if label.iter().all(|&code| code < 0x80) {
        let text = to_string(label);
        ulabel_ascii(&text)?;
        if text.len() > DNS_LABEL_LENGTH_LIMIT {
            return Err("Label too long".to_owned());
        }
        return Ok(text);
    }
    check_label(label)?;
    let encoded = format!("xn--{}", punycode_encode(label));
    if encoded.len() > DNS_LABEL_LENGTH_LIMIT {
        return Err("Label too long".to_owned());
    }
    Ok(encoded)
}

/// `idna.uts46_remap(domain, std3_rules=False, transitional=False)`.
fn uts46_remap(domain: &[u32]) -> Result<Vec<u32>, String> {
    if domain.len() > IDNA_MAX_INPUT_LENGTH {
        return Err("Domain too long".to_owned());
    }
    let mut output = String::new();
    for (position, &code) in domain.iter().enumerate() {
        let index = if code < 256 {
            code as usize
        } else {
            tables::UTS46_STARTS.partition_point(|&start| start <= code) - 1
        };
        let status = tables::UTS46_STATUSES[index];
        let replacement = tables::UTS46_REPLACEMENTS[index];
        let character = char::from_u32(code).expect("domain text has no surrogates");
        if status == b'V' || status == b'D' || (status == b'3' && replacement.is_none()) {
            output.push(character);
        } else if let Some(text) = replacement.filter(|_| matches!(status, b'M' | b'3')) {
            output.push_str(text);
        } else if status != b'I' {
            return Err(format!(
                "Codepoint {} not allowed at position {} in {}",
                unot(code),
                position + 1,
                python_repr(domain)
            ));
        }
    }
    Ok(code_points(&nfc(&output)))
}

fn is_label_separator(code: u32) -> bool {
    matches!(code, 0x2e | 0x3002 | 0xff0e | 0xff61)
}

fn split_labels(domain: &[u32]) -> Vec<&[u32]> {
    domain
        .split(|&code| is_label_separator(code))
        .collect::<Vec<_>>()
}

/// `idna.decode(domain)` for an ASCII domain: the Unicode form, joined by dots.
fn idna_decode(domain: &str) -> Result<Vec<u32>, String> {
    if domain.len() > IDNA_MAX_INPUT_LENGTH || domain.len() > 254 {
        return Err("Domain too long".to_owned());
    }
    let mut labels: Vec<&str> = domain.split('.').collect();
    if labels == [""] {
        return Err("Empty domain".to_owned());
    }
    let trailing_dot = labels.last() == Some(&"");
    if trailing_dot {
        labels.pop();
    }
    let mut output = Vec::new();
    for (index, label) in labels.iter().enumerate() {
        let decoded = ulabel_ascii(label)?;
        if decoded.is_empty() {
            return Err("Empty label".to_owned());
        }
        if index > 0 {
            output.push(0x2e);
        }
        output.extend(decoded);
    }
    if trailing_dot {
        output.push(0x2e);
    }
    Ok(output)
}

/// `idna.encode(domain)`: only whether it succeeds matters here.
fn idna_encode(domain: &[u32]) -> Result<(), String> {
    if domain.len() > IDNA_MAX_INPUT_LENGTH || domain.len() > 254 {
        return Err("Domain too long".to_owned());
    }
    let mut labels = split_labels(domain);
    if labels == [&[] as &[u32]] {
        return Err("Empty domain".to_owned());
    }
    let trailing_dot = labels.last().is_some_and(|label| label.is_empty());
    if trailing_dot {
        labels.pop();
    }
    let mut total = 0;
    for label in &labels {
        let encoded = alabel(label)?;
        if encoded.is_empty() {
            return Err("Empty label".to_owned());
        }
        total += encoded.len() + 1;
    }
    total -= 1;
    if total + usize::from(trailing_dot) > if trailing_dot { 254 } else { 253 } {
        return Err("Domain too long".to_owned());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// email_validator: domain name
// ---------------------------------------------------------------------------

fn is_hostname_character(character: char) -> bool {
    character.is_ascii_alphanumeric()
        || character == '-'
        || character == '.'
        || u32::from(character) >= 0x80
}

/// The characters `uts46_valid_char` accepts; the rest get a clear message
/// before UTS 46 would reject them with a generic one.
fn uts46_valid_char(character: char) -> bool {
    let code = u32::from(character);
    if (0x80..=0x9f).contains(&code) {
        return false;
    }
    if ((0x2010..=0x2060).contains(&code)
        && !(0x2024..=0x2026).contains(&code)
        && !(0x2028..=0x202e).contains(&code))
        || matches!(code, 0xad | 0x2064 | 0xff0e)
        || (0x200b..=0x200d).contains(&code)
        || (0x1bca0..=0x1bca3).contains(&code)
    {
        return true;
    }
    let class = category(code);
    if matches!(
        &class,
        b"Cf" | b"Cn" | b"Co" | b"Cs" | b"Zs" | b"Zl" | b"Zp"
    ) {
        return false;
    }
    tables::DECOMPOSES_WITH_DOT.binary_search(&code).is_err()
}

fn is_hostname_label(label: &str) -> bool {
    let bytes = label.as_bytes();
    !bytes.is_empty()
        && bytes
            .iter()
            .all(|&byte| byte.is_ascii_alphanumeric() || byte == b'-')
        && bytes[0].is_ascii_alphanumeric()
        && bytes[bytes.len() - 1].is_ascii_alphanumeric()
}

fn is_hostname(domain: &str) -> bool {
    domain.split('.').all(is_hostname_label)
}

fn domain_dot_atom_messages(label: &str) -> Result<(), Reason> {
    check_dot_atom(
        label,
        &|descriptor| {
            format!("An email address cannot have a {descriptor} immediately after the @-sign.")
        },
        &|descriptor| format!("An email address cannot end with a {descriptor}."),
        true,
    )
}

struct DomainName {
    ascii: String,
    unicode: String,
}

fn validate_domain_name(domain: &str) -> Result<DomainName, Reason> {
    let invalid = displays(
        domain
            .chars()
            .filter(|&character| !is_hostname_character(character)),
    );
    if !invalid.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters: {invalid}."
        ));
    }
    check_unsafe_chars(domain, false)?;
    let invalid = displays(
        domain
            .chars()
            .filter(|&character| !uts46_valid_char(character)),
    );
    if !invalid.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters: {invalid}."
        ));
    }
    let original = domain;
    let remapped = uts46_remap(&code_points(domain)).map_err(|reason| {
        format!("The part after the @-sign contains invalid characters ({reason}).")
    })?;
    let domain = to_string(&remapped);
    let invalid = displays(
        domain
            .chars()
            .filter(|&character| !is_hostname_character(character)),
    );
    if !invalid.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters after Unicode normalization: {invalid}."
        ));
    }
    domain_dot_atom_messages(&domain)?;
    for label in domain.split('.') {
        let characters: Vec<char> = label.chars().collect();
        if characters.len() >= 4
            && characters[2] == '-'
            && characters[3] == '-'
            && !(characters[0].eq_ignore_ascii_case(&'x')
                && characters[1].eq_ignore_ascii_case(&'n'))
        {
            return Err("An email address cannot have two letters followed by two dashes immediately after the @-sign or after a period, except Punycode.".to_owned());
        }
    }
    let ascii = if is_hostname(&domain) {
        domain.clone()
    } else {
        let mut labels = Vec::new();
        for label in domain.split('.') {
            labels.push(
                alabel(&code_points(label)).map_err(|reason| {
                    format!("The part after the @-sign is invalid ({reason}).")
                })?,
            );
        }
        let ascii = labels.join(".");
        if !is_hostname(&ascii) {
            return Err("The email address contains invalid characters after the @-sign after IDNA encoding.".to_owned());
        }
        ascii
    };
    if ascii.len() > DOMAIN_MAX_LENGTH {
        if ascii == original {
            return Err(format!(
                "The email address is too long after the @-sign {}.",
                length_reason(ascii.chars().count(), DOMAIN_MAX_LENGTH)
            ));
        }
        let difference = ascii.len() - DOMAIN_MAX_LENGTH;
        let suffix = if difference == 1 { "" } else { "s" };
        return Err(format!(
            "The email address is too long after the @-sign ({difference} byte{suffix} too many after IDNA encoding)."
        ));
    }
    for label in ascii.split('.') {
        if label.len() > DNS_LABEL_LENGTH_LIMIT {
            return Err(format!(
                "After the @-sign, periods cannot be separated by so many characters {}.",
                length_reason(label.chars().count(), DNS_LABEL_LENGTH_LIMIT)
            ));
        }
    }
    if !ascii.contains('.') {
        return Err("The part after the @-sign is not valid. It should have a period.".to_owned());
    }
    if !ascii.ends_with(|character: char| character.is_ascii_alphabetic()) {
        return Err(
            "The part after the @-sign is not valid. It is not within a valid top-level domain."
                .to_owned(),
        );
    }
    for reserved in SPECIAL_USE_DOMAIN_NAMES {
        if ascii == reserved || ascii.ends_with(&format!(".{reserved}")) {
            return Err(
                "The part after the @-sign is a special-use or reserved name that cannot be used with email.".to_owned(),
            );
        }
    }
    let decoded = idna_decode(&ascii)
        .map_err(|reason| format!("The part after the @-sign is not valid IDNA ({reason})."))?;
    let unicode = to_string(&decoded);
    let invalid = displays(
        unicode
            .chars()
            .filter(|&character| !is_hostname_character(character)),
    );
    if !invalid.is_empty() {
        return Err(format!(
            "The part after the @-sign contains invalid characters: {invalid}."
        ));
    }
    check_unsafe_chars(&unicode, false)?;
    idna_encode(&decoded).map_err(|reason| {
        format!(
            "The part after the @-sign became invalid after normalizing to international characters ({reason})."
        )
    })?;
    Ok(DomainName { ascii, unicode })
}

// ---------------------------------------------------------------------------
// email_validator: whole address
// ---------------------------------------------------------------------------

/// `validate_email_length`: the original, normalized and ASCII-domain forms
/// must each fit in 254 UTF-8 bytes.
fn validate_length(original: &str, normalized: &str, ascii_form: &str) -> Result<(), Reason> {
    let forms = [
        (original, None),
        (normalized, Some("after normalization")),
        (
            ascii_form,
            Some("when the part after the @-sign is converted to IDNA ASCII"),
        ),
    ];
    for (address, qualifier) in forms {
        let characters = address.chars().count();
        let bytes = address.len();
        let Some(difference) = bytes.checked_sub(EMAIL_MAX_LENGTH).filter(|&d| d > 0) else {
            continue;
        };
        let suffix = if difference > 1 { "s" } else { "" };
        let reason = match qualifier {
            None if characters == bytes => length_reason(characters, EMAIL_MAX_LENGTH),
            None => {
                let widest = address.chars().map(char::len_utf8).max().unwrap_or(1);
                let minimum = (difference / widest).max(1);
                if minimum == difference {
                    format!("({difference} character{suffix} too many)")
                } else {
                    format!("({minimum}-{difference} character{suffix} too many)")
                }
            }
            Some(text) => format!("{text} ({difference} byte{suffix} too many)"),
        };
        return Err(format!("The email address is too long {reason}."));
    }
    Ok(())
}

fn validate_address(email: &str) -> Result<String, Reason> {
    let Split {
        has_display_name,
        local,
        domain,
        quoted,
    } = split_email(email)?;
    let original = if quoted {
        format!("\"{local}\"@{domain}")
    } else {
        format!("{local}@{domain}")
    };
    let LocalPart {
        local: mut local_part,
        ascii: mut ascii_local,
    } = validate_local_part(&local, quoted)?;
    let normalized_local = nfc(&local_part);
    if normalized_local != local_part {
        validate_local_part(&normalized_local, quoted)
            .map_err(|reason| format!("After Unicode normalization: {reason}"))?;
        local_part = normalized_local;
    }
    if quoted {
        return Err("Quoting the part before the @-sign is not allowed here.".to_owned());
    }
    if let Some(ascii) = ascii_local.as_mut()
        && MAILBOX_NAMES.contains(&ascii.to_ascii_lowercase().as_str())
    {
        ascii.make_ascii_lowercase();
        local_part.make_ascii_lowercase();
    }
    if domain.is_empty() {
        return Err("There must be something after the @-sign.".to_owned());
    }
    if domain.starts_with('[') && domain.ends_with(']') {
        validate_domain_literal(&domain[1..domain.len() - 1])?;
        return Err("A bracketed IP address after the @-sign is not allowed here.".to_owned());
    }
    let DomainName { ascii, unicode } = validate_domain_name(&domain)?;
    let normalized = format!("{local_part}@{unicode}");
    let ascii_form = format!(
        "{}@{ascii}",
        ascii_local.as_deref().unwrap_or(local_part.as_str())
    );
    validate_length(&original, &normalized, &ascii_form)?;
    if has_display_name {
        return Err(
            "A display name and angle brackets around the email address are not permitted here."
                .to_owned(),
        );
    }
    Ok(normalized)
}
