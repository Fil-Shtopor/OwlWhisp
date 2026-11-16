//! Exact source spans which text cleanup must not edit.

use regex::Regex;
use std::{ops::Range, sync::OnceLock};

/// Quoted material, code, paths, identifiers, links and numeric forms, as source byte ranges.
pub fn protected_spans(input: &str) -> Vec<Range<usize>> {
    let mut spans = technical_spans(input);
    let chars: Vec<(usize, char)> = input.char_indices().collect();
    add_quotes(input, &chars, &mut spans);
    spans.sort_by_key(|r| r.start);
    spans
}

/// Technical source spans, without quoted prose (which a translator must be allowed to translate).
pub fn technical_spans(input: &str) -> Vec<Range<usize>> {
    static TECHNICAL: OnceLock<Regex> = OnceLock::new();
    let technical = TECHNICAL.get_or_init(|| Regex::new(
        r"(?x)(?:https?://|www\.)[^\s<>]+|[\p{L}\p{N}._%+\-]+@[\p{L}\p{N}.\-]+|(?:[A-Za-z]:\\|\\\\|\.{0,2}/)[^\s<>]+|(?:\b(?:let|const|var|int|str|float)\s+)?[\p{L}_][\p{L}\p{N}_]*[\t\x20]*=[\t\x20]*[^\s,;]+|[\p{L}\p{N}_]+(?:[._][\p{L}\p{N}_][\p{L}\p{N}_/\\.\-]*|[/@=:\\][\p{L}\p{N}_/\\.\-]+)+|\p{N}+(?:[.,:/%+\-]\p{N}*)*"
    ).expect("constant technical-span expression"));
    technical
        .find_iter(input)
        .map(|m| {
            let mut range = m.range();
            while let Some(c) = input[range.clone()].chars().next_back() {
                if !matches!(
                    c,
                    '.' | ','
                        | ';'
                        | '!'
                        | '?'
                        | ')'
                        | ']'
                        | '"'
                        | '\''
                        | '»'
                        | '”'
                        | '」'
                        | '』'
                        | '。'
                        | '，'
                ) {
                    break;
                }
                range.end -= c.len_utf8();
            }
            range
        })
        .filter(|r| !r.is_empty())
        .collect()
}

fn add_quotes(input: &str, chars: &[(usize, char)], spans: &mut Vec<Range<usize>>) {
    let mut i = 0;
    while i < chars.len() {
        let (start, ch) = chars[i];
        let close = match ch {
            '`' => '`',
            '"' => '"',
            '«' => '»',
            '“' => '”',
            '「' => '」',
            '『' => '』',
            '\'' if i == 0 || !chars[i - 1].1.is_alphanumeric() => '\'',
            _ => {
                i += 1;
                continue;
            }
        };
        let fenced = ch == '`' && input[start..].starts_with("```");
        if fenced {
            let end = input[start + 3..]
                .find("```")
                .map_or(input.len(), |n| start + 3 + n + 3);
            spans.push(start..end);
            while i < chars.len() && chars[i].0 < end {
                i += 1;
            }
            continue;
        }
        let mut end = input.len();
        let mut found = false;
        let mut j = i + 1;
        while j < chars.len() {
            if chars[j].1 == close
                && chars[j - 1].1 != '\\'
                && (close != '\'' || j + 1 == chars.len() || !chars[j + 1].1.is_alphanumeric())
            {
                end = chars[j].0 + close.len_utf8();
                found = true;
                break;
            }
            j += 1;
        }
        // An unmatched apostrophe may be an elision (French l', Italian un'), not a quote.
        if ch != '\'' || found {
            spans.push(start..end);
            while i < chars.len() && chars[i].0 < end {
                i += 1;
            }
        } else {
            i += 1;
        }
    }
}

pub(crate) fn intersects(spans: &[Range<usize>], range: &Range<usize>) -> bool {
    spans.iter().any(|s| s.start < range.end && range.start < s.end)
}
