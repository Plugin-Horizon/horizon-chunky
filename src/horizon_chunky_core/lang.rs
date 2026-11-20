//! Chunky's messages in the configured `language` (`config.toml`), from the translation files Chunky ships
//! (`lang/<code>.json`, GPL-3.0, pop4959/Chunky). The engine writes its messages in English. `localize` turns a
//! line into the chosen language: `lang/en.json` gives each message's English text with its `%s` slots, the line
//! is matched against those texts (the most literal text wins), and the same slots are put into the translation.
//! The words the engine fills in (shape and pattern names, `enabled` and `disabled`) are translated as well, also
//! inside a comma-separated list. A line that matches nothing, or whose language lacks the message, is printed as
//! it is. `tr` renders one message directly, for the few lines built from already translated parts.

use std::collections::HashMap;
use std::iter::Peekable;
use std::str::Chars;
use std::sync::{Arc, OnceLock, RwLock};

/// The translation files by language code, as Chunky bundles them. `en` is the text the engine's messages are written in.
const FILES: [(&str, &str); 28] = [
    ("bg", include_str!("../../lang/bg.json")),
    ("bs", include_str!("../../lang/bs.json")),
    ("cs", include_str!("../../lang/cs.json")),
    ("de", include_str!("../../lang/de.json")),
    ("en", include_str!("../../lang/en.json")),
    ("es", include_str!("../../lang/es.json")),
    ("es_AR", include_str!("../../lang/es_AR.json")),
    ("fi", include_str!("../../lang/fi.json")),
    ("fr", include_str!("../../lang/fr.json")),
    ("he", include_str!("../../lang/he.json")),
    ("hi", include_str!("../../lang/hi.json")),
    ("hr", include_str!("../../lang/hr.json")),
    ("it", include_str!("../../lang/it.json")),
    ("ja", include_str!("../../lang/ja.json")),
    ("ko", include_str!("../../lang/ko.json")),
    ("nl", include_str!("../../lang/nl.json")),
    ("no", include_str!("../../lang/no.json")),
    ("pl", include_str!("../../lang/pl.json")),
    ("pt", include_str!("../../lang/pt.json")),
    ("pt_BR", include_str!("../../lang/pt_BR.json")),
    ("ru", include_str!("../../lang/ru.json")),
    ("sr_CS", include_str!("../../lang/sr_CS.json")),
    ("tr", include_str!("../../lang/tr.json")),
    ("uk", include_str!("../../lang/uk.json")),
    ("vi", include_str!("../../lang/vi.json")),
    ("zh_CN", include_str!("../../lang/zh_CN.json")),
    ("zh_HK", include_str!("../../lang/zh_HK.json")),
    ("zh_TW", include_str!("../../lang/zh_TW.json")),
];

/// The prefix of Chunky's prefixed messages (`sendMessagePrefixed`), not part of the translated text.
const PREFIX: &str = "[Chunky] ";

/// The translation in use; None is English (the engine's own text).
static CURRENT: RwLock<Option<Arc<HashMap<String, String>>>> = RwLock::new(None);

/// Sets the language (`language` in config.toml). An unknown code gives English, as Chunky's `checkLanguage` does.
/// Returns whether the code is one Chunky ships.
pub fn set_language(code: &str) -> bool {
    let (map, known) = match FILES.iter().find(|f| f.0 == code) {
        Some(f) if f.0 != "en" => (Some(Arc::new(parse_flat(f.1))), true),
        Some(_) => (None, true),
        None => (None, false),
    };
    *CURRENT.write().unwrap_or_else(|e| e.into_inner()) = map;
    known
}

/// A message in the language in use, with its `%s` slots filled in (`Translator.translateKey`). English when the
/// message has no translation.
pub fn tr(key: &str, args: &[&str]) -> String {
    let current = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    let text = current
        .as_ref()
        .and_then(|m| m.get(key))
        .or_else(|| english().get(key))
        .cloned()
        .unwrap_or_else(|| key.to_owned());
    render(&compile(&text), args)
}

/// The line in the language in use (see the module doc). Engine lines in English come back unchanged.
pub fn localize(line: &str) -> String {
    let current = CURRENT.read().unwrap_or_else(|e| e.into_inner()).clone();
    localize_with(current.as_deref(), line)
}

fn localize_with(lang: Option<&HashMap<String, String>>, line: &str) -> String {
    let Some(lang) = lang else { return line.to_owned() };
    let (prefix, body) = match line.strip_prefix(PREFIX) {
        Some(rest) => (PREFIX, rest),
        None => ("", line),
    };
    let Some((template, args)) = find(body) else { return line.to_owned() };
    let Some(target) = lang.get(&template.key) else { return line.to_owned() };
    let args = translate_args(args, lang);
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    format!("{prefix}{}", render(&compile(target), &refs))
}

/// Keys whose English text is a word the engine puts into other messages (shape, pattern, on/off).
fn is_word_key(key: &str) -> bool {
    matches!(key, "enabled" | "disabled") || key.starts_with("shape_") || key.starts_with("pattern_") || key.starts_with("wrap_")
}

/// The English file, parsed once.
fn english() -> &'static HashMap<String, String> {
    static EN: OnceLock<HashMap<String, String>> = OnceLock::new();
    EN.get_or_init(|| {
        let text = FILES.iter().find(|f| f.0 == "en").map(|f| f.1).unwrap_or("{}");
        parse_flat(text)
    })
}

/// An English message as a line matcher: its key, its pieces, and how many characters of fixed text it has.
struct Template {
    key: String,
    pieces: Vec<Piece>,
    literal: usize,
}

/// The English messages that can be matched in a line, the most literal text first.
fn templates() -> &'static [Template] {
    static TEMPLATES: OnceLock<Vec<Template>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut out: Vec<Template> = english()
            .iter()
            .filter(|(key, _)| !is_word_key(key) && !key.starts_with("time_"))
            .map(|(key, text)| {
                let pieces = compile(text);
                let literal = pieces
                    .iter()
                    .map(|p| match p {
                        Piece::Lit(s) => s.len(),
                        Piece::Arg(_) => 0,
                    })
                    .sum();
                Template { key: key.clone(), pieces, literal }
            })
            // Two slots in a row cannot be told apart, and a message without fixed text would match any line.
            .filter(|t| t.literal > 0 && !t.pieces.windows(2).any(|w| matches!(w, [Piece::Arg(_), Piece::Arg(_)])))
            .collect();
        out.sort_by(|a, b| b.literal.cmp(&a.literal).then_with(|| a.key.cmp(&b.key)));
        out
    })
}

/// The first English message that the line is, with its slot texts.
fn find(line: &str) -> Option<(&'static Template, Vec<String>)> {
    templates().iter().find_map(|t| capture(&t.pieces, line).map(|args| (t, args)))
}

/// A translated word for an English shape, pattern or on/off word; the word itself when there is none.
fn translate_word(word: &str, lang: &HashMap<String, String>) -> String {
    english()
        .iter()
        .find(|(key, text)| is_word_key(key) && text.as_str() == word)
        .and_then(|(key, _)| lang.get(key))
        .cloned()
        .unwrap_or_else(|| word.to_owned())
}

/// Translates the words in each slot, each item of a list (`circle, square`) on its own.
fn translate_args(args: Vec<String>, lang: &HashMap<String, String>) -> Vec<String> {
    args.into_iter()
        .map(|arg| arg.split(", ").map(|part| translate_word(part, lang)).collect::<Vec<_>>().join(", "))
        .collect()
}

/// One part of a message: fixed text, or a slot (`%s`: the next argument, `%n$s`: argument n, counted from 0 here).
#[derive(Clone, Debug, PartialEq)]
enum Piece {
    Lit(String),
    Arg(Option<usize>),
}

/// Splits a message into its pieces (`%%` is a percent sign; a `%` that starts no slot stays as it is).
fn compile(text: &str) -> Vec<Piece> {
    let mut out: Vec<Piece> = Vec::new();
    let mut lit = String::new();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '%' {
            lit.push(c);
            continue;
        }
        match chars.peek().copied() {
            Some('%') => {
                chars.next();
                lit.push('%');
            }
            Some('s') => {
                chars.next();
                flush(&mut out, &mut lit);
                out.push(Piece::Arg(None));
            }
            Some(d) if d.is_ascii_digit() => {
                let mut probe = chars.clone();
                let mut digits = String::new();
                while let Some(&d) = probe.peek() {
                    if !d.is_ascii_digit() {
                        break;
                    }
                    digits.push(d);
                    probe.next();
                }
                let dollar_s = probe.next() == Some('$') && probe.next() == Some('s');
                match digits.parse::<usize>() {
                    Ok(n) if dollar_s && n >= 1 => {
                        chars = probe;
                        flush(&mut out, &mut lit);
                        out.push(Piece::Arg(Some(n - 1)));
                    }
                    _ => lit.push('%'),
                }
            }
            _ => lit.push('%'),
        }
    }
    flush(&mut out, &mut lit);
    out
}

fn flush(out: &mut Vec<Piece>, lit: &mut String) {
    if !lit.is_empty() {
        out.push(Piece::Lit(std::mem::take(lit)));
    }
}

/// The message with its slots filled in; a missing argument gives an empty slot.
fn render(pieces: &[Piece], args: &[&str]) -> String {
    let mut out = String::new();
    let mut next = 0;
    for piece in pieces {
        match piece {
            Piece::Lit(text) => out.push_str(text),
            Piece::Arg(slot) => {
                let i = match slot {
                    Some(i) => *i,
                    None => {
                        next += 1;
                        next - 1
                    }
                };
                if let Some(arg) = args.get(i) {
                    out.push_str(arg);
                }
            }
        }
    }
    out
}

/// The slot texts if `line` is `pieces` filled in, in the order the slots come. The fixed text must appear in order;
/// a slot takes the text up to the next fixed part.
fn capture(pieces: &[Piece], line: &str) -> Option<Vec<String>> {
    let mut args = Vec::new();
    let mut pos = 0;
    let mut slot_open = false;
    for (k, piece) in pieces.iter().enumerate() {
        let last = k + 1 == pieces.len();
        match piece {
            Piece::Arg(_) => slot_open = true,
            Piece::Lit(text) => {
                let rest = &line[pos..];
                let at = if !slot_open {
                    rest.starts_with(text.as_str()).then_some(0)?
                } else if last {
                    rest.ends_with(text.as_str()).then(|| rest.len() - text.len())?
                } else {
                    rest.find(text.as_str())?
                };
                if slot_open {
                    args.push(rest[..at].to_owned());
                }
                pos += at + text.len();
                slot_open = false;
            }
        }
    }
    if slot_open {
        args.push(line[pos..].to_owned());
        pos = line.len();
    }
    (pos == line.len()).then_some(args)
}

/// A flat JSON object of strings (the translation files): key to text. Malformed input gives what was read so far.
fn parse_flat(text: &str) -> HashMap<String, String> {
    let mut out = HashMap::new();
    let mut chars = text.chars().peekable();
    for c in chars.by_ref() {
        if c == '{' {
            break;
        }
    }
    loop {
        skip_ws(&mut chars);
        if chars.peek() != Some(&'"') {
            break;
        }
        let Some(key) = parse_string(&mut chars) else { break };
        skip_ws(&mut chars);
        if chars.next() != Some(':') {
            break;
        }
        skip_ws(&mut chars);
        let Some(value) = parse_string(&mut chars) else { break };
        out.insert(key, value);
        skip_ws(&mut chars);
        if chars.next() != Some(',') {
            break;
        }
    }
    out
}

fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
}

/// One JSON string, quotes included, with its escapes read.
fn parse_string(chars: &mut Peekable<Chars<'_>>) -> Option<String> {
    if chars.next() != Some('"') {
        return None;
    }
    let mut s = String::new();
    loop {
        match chars.next()? {
            '"' => return Some(s),
            '\\' => match chars.next()? {
                '"' => s.push('"'),
                '\\' => s.push('\\'),
                '/' => s.push('/'),
                'b' => s.push('\u{8}'),
                'f' => s.push('\u{c}'),
                'n' => s.push('\n'),
                'r' => s.push('\r'),
                't' => s.push('\t'),
                'u' => {
                    let hi = hex4(chars)?;
                    let code = if (0xD800..0xDC00).contains(&hi) {
                        // A surrogate pair: the low half follows as another escape.
                        if chars.next()? != '\\' || chars.next()? != 'u' {
                            return None;
                        }
                        let lo = hex4(chars)?;
                        0x10000 + ((hi - 0xD800) << 10) + lo.checked_sub(0xDC00)?
                    } else {
                        hi
                    };
                    s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                }
                _ => return None,
            },
            c => s.push(c),
        }
    }
}

/// The four hex digits of a `\u` escape.
fn hex4(chars: &mut Peekable<Chars<'_>>) -> Option<u32> {
    let hex: String = (0..4).map(|_| chars.next()).collect::<Option<String>>()?;
    u32::from_str_radix(&hex, 16).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn german() -> HashMap<String, String> {
        parse_flat(FILES.iter().find(|f| f.0 == "de").map(|f| f.1).unwrap_or("{}"))
    }

    #[test]
    fn flat_json_reads_escapes_and_surrogate_pairs() {
        let m = parse_flat("{\n  \"a\": \"x\\\"y\",\n  \"b\": \"\\u00e4\\ud83d\\ude00\"\n}");
        assert_eq!(m.get("a").map(String::as_str), Some("x\"y"));
        assert_eq!(m.get("b").map(String::as_str), Some("\u{e4}\u{1F600}"));
    }

    #[test]
    fn slots_and_percent_signs_compile_and_render() {
        let pieces = compile("Processed: %s chunks (%s%%)");
        assert_eq!(
            pieces,
            vec![
                Piece::Lit("Processed: ".into()),
                Piece::Arg(None),
                Piece::Lit(" chunks (".into()),
                Piece::Arg(None),
                Piece::Lit("%)".into()),
            ]
        );
        assert_eq!(render(&pieces, &["5", "3.50"]), "Processed: 5 chunks (3.50%)");
        assert_eq!(render(&compile("%2$s before %1$s"), &["a", "b"]), "b before a");
    }

    #[test]
    fn a_line_is_matched_to_its_english_message_and_slots() {
        let args = capture(&compile("Task finished for %s. Processed: %s chunks (%s%%)"), "Task finished for world. Processed: 5 chunks (3.50%)");
        assert_eq!(args, Some(vec!["world".to_owned(), "5".to_owned(), "3.50".to_owned()]));
        assert_eq!(capture(&compile("Pattern changed to %s."), "Shape changed to square."), None);
        assert_eq!(capture(&compile("&aChunky Commands&r%s"), "&aChunky Commands&r"), Some(vec![String::new()]));
    }

    #[test]
    fn german_lines_come_out_in_german_with_the_words_translated() {
        let de = german();
        // `format_shape` "Form geändert zu %s." with the word `square` translated by `shape_square` ("Quadrat").
        assert_eq!(
            localize_with(Some(&de), "[Chunky] Shape changed to square."),
            "[Chunky] Form geändert zu Quadrat."
        );
        // `format_silent` "Stummer Modus %s." with `enabled` ("aktiviert").
        assert_eq!(
            localize_with(Some(&de), "[Chunky] Silent mode enabled."),
            "[Chunky] Stummer Modus aktiviert."
        );
        // A list of words is translated item by item.
        // `format_shape_not_found` is still English in de.json, so only the list's words change.
        assert_eq!(
            localize_with(Some(&de), "[Chunky] Shape blob not found. Available shapes: circle, square"),
            "[Chunky] Shape blob not found. Available shapes: Kreis, Quadrat"
        );
    }

    #[test]
    fn english_and_unknown_lines_are_left_alone() {
        assert_eq!(localize_with(None, "[Chunky] Nothing to confirm!"), "[Chunky] Nothing to confirm!");
        let de = german();
        assert_eq!(localize_with(Some(&de), "some other line"), "some other line");
        assert!(!set_language("xx"));
        assert!(set_language("de"));
        assert!(set_language("en"));
    }
}
