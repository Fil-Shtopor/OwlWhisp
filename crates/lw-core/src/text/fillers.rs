//! Conservative, multilingual hesitation removal. No rewriting or model inference.

use super::protected::{intersects, protected_spans};
use super::{TextLanguage, TextProcessor, cleanup_language};
use regex::Regex;
use std::sync::OnceLock;

/// Remove only language-specific hesitation forms, preserving quoted and technical source spans.
pub struct FillerProcessor {
    language: TextLanguage,
    reported_language: Option<String>,
}

impl FillerProcessor {
    /// Explicit source preference and optional language reported by the speech engine.
    pub fn new(language: TextLanguage, reported_language: Option<&str>) -> Self {
        Self {
            language,
            reported_language: reported_language.map(str::to_owned),
        }
    }
}

impl TextProcessor for FillerProcessor {
    fn name(&self) -> &str {
        "filler-words"
    }

    fn process(&self, input: &str) -> String {
        let language = cleanup_language(input, self.language, self.reported_language.as_deref());
        let forms = language.fillers();
        if forms.is_empty() {
            return input.to_owned();
        }
        let protected = protected_spans(input);
        static WORDS: OnceLock<Regex> = OnceLock::new();
        let words = WORDS.get_or_init(|| {
            Regex::new(r"[\p{L}\p{M}]+(?:[-–][\p{L}\p{M}]+)*").expect("constant word expression")
        });
        let tokens: Vec<_> = words.find_iter(input).collect();
        let mut removals = Vec::new();
        for (index, token) in tokens.iter().enumerate() {
            let low = if language == TextLanguage::Turkish {
                token.as_str().replace('I', "ı").replace('İ', "i").to_lowercase()
            } else {
                token.as_str().to_lowercase()
            };
            let hesitation = forms.iter().any(|form| {
                low == *form || {
                    let last = form.chars().next_back().unwrap_or_default();
                    low.strip_prefix(form)
                        .is_some_and(|rest| !rest.is_empty() && rest.chars().all(|c| c == last))
                }
            });
            if !hesitation || intersects(&protected, &token.range()) {
                continue;
            }
            if index > 0 && literal_context(tokens[index - 1].as_str()) {
                continue;
            }
            // A letter beside an identifier is never a standalone hesitation.
            if input[..token.start()]
                .chars()
                .next_back()
                .is_some_and(|c| c.is_numeric() || c == '_')
                || input[token.end()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_numeric() || c == '_')
            {
                continue;
            }
            removals.push(token.range());
        }
        if matches!(language, TextLanguage::Chinese | TextLanguage::Japanese) {
            for form in forms {
                for (start, _) in input.match_indices(form) {
                    let mut end = start + form.len();
                    if language == TextLanguage::Chinese {
                        while input[end..].starts_with('呃') {
                            end += '呃'.len_utf8();
                        }
                    }
                    let boundary = start == 0
                        || input[..start]
                            .chars()
                            .next_back()
                            .is_some_and(|c| !c.is_alphanumeric());
                    // 呃逆 is a lexical word (hiccup), not a hesitation.
                    if boundary && !input[end..].starts_with('逆') && !intersects(&protected, &(start..end))
                    {
                        removals.push(start..end);
                    }
                }
            }
        }
        if removals.is_empty() {
            return input.to_owned();
        }
        removals.sort_by_key(|r| (r.start, std::cmp::Reverse(r.end)));
        let mut out = String::with_capacity(input.len());
        let mut cursor = 0;
        for range in removals {
            if range.start < cursor {
                continue;
            }
            out.push_str(&input[cursor..range.start]);
            let mut end = range.end;
            let tail = &input[end..];
            let gap = tail.len() - tail.trim_start_matches([' ', '\t']).len();
            let after_gap = &input[end + gap..];
            let sentence_start = out.trim_end().is_empty()
                || out.trim_end().ends_with(['.', '!', '?', '。', '！', '？', '\n']);
            if let Some(mark) = after_gap.chars().next()
                && (matches!(mark, ',' | '，' | '、' | '،')
                    || (sentence_start && matches!(mark, '.' | '。' | '!' | '?' | '！' | '？' | '…')))
            {
                end += gap + mark.len_utf8();
                if sentence_start {
                    while let Some(mark) = input[end..].chars().next() {
                        if !matches!(mark, '.' | '。' | '!' | '?' | '！' | '？' | '…') {
                            break;
                        }
                        end += mark.len_utf8();
                    }
                }
            }
            if out.is_empty() || out.ends_with([' ', '\t']) {
                end += input[end..].len() - input[end..].trim_start_matches([' ', '\t']).len();
            }
            cursor = end;
        }
        out.push_str(&input[cursor..]);
        out.trim().to_owned()
    }
}

fn literal_context(word: &str) -> bool {
    matches!(
        word.to_lowercase().as_str(),
        "буква"
            | "букву"
            | "слово"
            | "слова"
            | "звук"
            | "термин"
            | "переменная"
            | "літера"
            | "letter"
            | "word"
            | "sound"
            | "variable"
            | "token"
            | "named"
            | "lettre"
            | "mot"
            | "son"
            | "palabra"
            | "letra"
            | "parola"
            | "lettera"
            | "wort"
            | "buchstabe"
            | "palavra"
            | "woord"
            | "litera"
            | "słowo"
            | "slovo"
            | "písmeno"
            | "kelime"
            | "harf"
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clean(text: &str, language: TextLanguage) -> String {
        FillerProcessor::new(language, None).process(text)
    }

    #[test]
    fn common_languages_remove_hesitations_but_not_discourse_words() {
        for (lang, input, expected) in [
            (
                TextLanguage::Russian,
                "Эм, привет, ну давай завтра.",
                "привет, ну давай завтра.",
            ),
            (TextLanguage::Ukrainian, "Е-е, це слово.", "це слово."),
            (TextLanguage::Ukrainian, "Ем, це слово.", "це слово."),
            (
                TextLanguage::English,
                "Um, I like this, uh, really.",
                "I like this, really.",
            ),
            (
                TextLanguage::Spanish,
                "Eh, hola, necesito el informe.",
                "hola, necesito el informe.",
            ),
            (
                TextLanguage::Italian,
                "Ehm, ciao, vorrei questo.",
                "ciao, vorrei questo.",
            ),
            (
                TextLanguage::French,
                "Euh, bonjour, je veux ceci.",
                "bonjour, je veux ceci.",
            ),
            (
                TextLanguage::German,
                "Ähm, ich komme um acht.",
                "ich komme um acht.",
            ),
            (
                TextLanguage::Portuguese,
                "Ahn, quero ficar em casa.",
                "quero ficar em casa.",
            ),
            (
                TextLanguage::Dutch,
                "Ehm, ik stuur het bestand.",
                "ik stuur het bestand.",
            ),
            (
                TextLanguage::Polish,
                "Yyy, proszę wysłać plik.",
                "proszę wysłać plik.",
            ),
            (
                TextLanguage::Czech,
                "Ehm, prosím pošli soubor.",
                "prosím pošli soubor.",
            ),
            (TextLanguage::Turkish, "Iıı, merhaba.", "merhaba."),
            (TextLanguage::Chinese, "呃，我想明天开会。", "我想明天开会。"),
            (TextLanguage::Chinese, "呃我想明天开会。", "我想明天开会。"),
            (
                TextLanguage::Japanese,
                "えーと、明日お願いします。",
                "明日お願いします。",
            ),
            (TextLanguage::Korean, "음, 내일 만나요.", "내일 만나요."),
            (TextLanguage::Arabic, "أمم، مرحبا", "مرحبا"),
            (TextLanguage::Hindi, "उम्म, कल मिलते हैं।", "कल मिलते हैं।"),
        ] {
            assert_eq!(clean(input, lang), expected, "{}: {input}", lang.label());
        }
    }

    #[test]
    fn preserves_quotes_code_names_numbers_and_literal_sounds() {
        let input =
            "Um, keep `um` and \"uh\", config.json, user_id=42, /api/um, https://um.example and the word um.";
        assert_eq!(
            clean(input, TextLanguage::English),
            "keep `um` and \"uh\", config.json, user_id=42, /api/um, https://um.example and the word um."
        );
        for input in [
            "Это ошибка типа данных, а не слово-паразит типа.",
            "Как бы ни изменился срок, не удаляй файл.",
            "Буква эм и слово «эм».",
            "嗯，我同意。",
            "呃逆是一种症状。",
            "Umberto is a name.",
        ] {
            assert_eq!(clean(input, TextLanguage::Auto), input);
        }
    }

    #[test]
    fn language_hints_prevent_cross_language_deletion() {
        assert_eq!(
            clean("Ich komme um acht.", TextLanguage::Auto),
            "Ich komme um acht."
        );
        assert_eq!(
            clean("Uh, please send the file.", TextLanguage::Auto),
            "please send the file."
        );
        assert_eq!(
            clean("Euh, je veux le rapport.", TextLanguage::Auto),
            "je veux le rapport."
        );
        assert_eq!(clean("Um, hola.", TextLanguage::Spanish), "Um, hola.");
        assert_eq!(
            FillerProcessor::new(TextLanguage::Auto, Some("en-US")).process("um hello"),
            "hello"
        );
    }

    #[test]
    fn blank_and_filler_only_input_does_not_leave_punctuation() {
        assert_eq!(clean("Um, uh.", TextLanguage::English), "");
        assert_eq!(clean("", TextLanguage::Russian), "");
    }

    #[test]
    fn multilingual_outcome_fixture_and_idempotence() {
        #[derive(serde::Deserialize)]
        struct Case {
            language: TextLanguage,
            input: String,
            expected: String,
        }
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/text-processing/multilingual-cleanup.json"
        ))
        .unwrap();
        for case in cases {
            let result = clean(&case.input, case.language);
            assert_eq!(result, case.expected, "{}: {}", case.language, case.input);
            assert_eq!(
                clean(&result, case.language),
                result,
                "second pass: {}",
                case.input
            );
        }
    }
}
