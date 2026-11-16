//! Light formatting of punctuation already supplied by ASR; no grammar inference.

use super::protected::{intersects, protected_spans};
use super::{TextLanguage, TextProcessor, cleanup_language};

/// Tidy punctuation spacing and sentence starts while preserving protected source spans.
pub struct PunctuationProcessor {
    language: TextLanguage,
    reported_language: Option<String>,
}

impl PunctuationProcessor {
    /// Source language preference and optional engine language hint.
    pub fn new(language: TextLanguage, reported_language: Option<&str>) -> Self {
        Self {
            language,
            reported_language: reported_language.map(str::to_owned),
        }
    }
}

impl TextProcessor for PunctuationProcessor {
    fn name(&self) -> &str {
        "punctuation-and-case"
    }

    fn process(&self, input: &str) -> String {
        let language = cleanup_language(input, self.language, self.reported_language.as_deref());
        let spans = protected_spans(input);
        let mut out = String::with_capacity(input.len());
        let mut sentence_start = true;
        for (index, c) in input.char_indices() {
            if intersects(&spans, &(index..index + c.len_utf8())) {
                out.push(c);
                if c.is_alphabetic() {
                    sentence_start = false;
                }
                continue;
            }
            let punctuation = matches!(
                c,
                '.' | ',' | '!' | '?' | ':' | ';' | '。' | '，' | '！' | '？' | '、' | '،' | '؟'
            );
            let french_space = language == TextLanguage::French && matches!(c, '!' | '?' | ':' | ';');
            if punctuation && !french_space {
                while out.ends_with([' ', '\t']) {
                    out.pop();
                }
            }
            if sentence_start && c.is_alphabetic() {
                let word = input[index..]
                    .split(|v: char| !v.is_alphabetic())
                    .next()
                    .unwrap_or("");
                // iPhone, eBay and other supplied mixed-case names keep their spelling.
                if !word.chars().any(char::is_uppercase) {
                    if language == TextLanguage::Turkish && c == 'i' {
                        out.push('İ');
                    } else {
                        out.extend(c.to_uppercase());
                    }
                } else {
                    out.push(c);
                }
                sentence_start = false;
            } else {
                out.push(c);
            }
            if matches!(c, '.' | '!' | '?' | '。' | '！' | '？' | '؟' | '\n') {
                sentence_start = true;
            }
        }
        out.trim().to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn published_examples_match_the_processor_and_are_idempotent() {
        #[derive(serde::Deserialize)]
        struct Case {
            language: TextLanguage,
            input: String,
            expected: String,
        }
        let cases: Vec<Case> = serde_json::from_str(include_str!(
            "../../../../tests/fixtures/text-processing/punctuation.json"
        ))
        .unwrap();
        for case in cases {
            let processor = PunctuationProcessor::new(case.language, None);
            let output = processor.process(&case.input);
            assert_eq!(output, case.expected, "{}: {}", case.language, case.input);
            assert_eq!(processor.process(&output), output);
        }
    }

    #[test]
    fn formats_asr_punctuation_without_inventing_grammar() {
        let p = PunctuationProcessor::new(TextLanguage::English, None);
        assert_eq!(
            p.process("hello , world . how are you ?"),
            "Hello, world. How are you?"
        );
        assert_eq!(p.process("where is the report"), "Where is the report");
        assert_eq!(p.process("iPhone works. eBay too."), "iPhone works. eBay too.");
    }

    #[test]
    fn respects_french_and_chinese_spacing() {
        assert_eq!(
            PunctuationProcessor::new(TextLanguage::French, None).process("bonjour ! comment ça va ?"),
            "Bonjour ! Comment ça va ?"
        );
        assert_eq!(
            PunctuationProcessor::new(TextLanguage::Chinese, None).process("你好 ，世界 。"),
            "你好，世界。"
        );
    }

    #[test]
    fn preserves_code_quotes_urls_and_decimal_values() {
        let text = "`config.json` and \"uh , hello\" cost 2.50 at https://example.com/api/v1.";
        let result = PunctuationProcessor::new(TextLanguage::English, None).process(text);
        for literal in [
            "`config.json`",
            "\"uh , hello\"",
            "2.50",
            "https://example.com/api/v1",
        ] {
            assert!(result.contains(literal), "changed {literal}: {result}");
        }
    }
}
