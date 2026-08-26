//! Deterministic text cleanup stages.
//!
//! - [`IdentityProcessor`]: passes text through unchanged (the always-safe default).
//! - [`CleanupProcessor`]: whitespace normalization, spoken-punctuation mapping, optional
//!   sentence capitalization and trailing-period trimming. All rules are local and reversible in
//!   spirit (they never invent content).

use unicode_normalization::UnicodeNormalization;

use super::TextProcessor;

/// A no-op processor: the safe default when cleanup is disabled.
pub struct IdentityProcessor;

impl TextProcessor for IdentityProcessor {
    fn name(&self) -> &str {
        "identity"
    }
    fn process(&self, input: &str) -> String {
        input.to_string()
    }
}

/// Options controlling [`CleanupProcessor`].
#[derive(Clone, Copy, Debug)]
pub struct NormalizeOptions {
    /// Collapse runs of whitespace to a single space and trim.
    pub collapse_whitespace: bool,
    /// Map spoken punctuation ("comma", "new line", "period") to symbols.
    pub spoken_punctuation: bool,
    /// Capitalize the first letter of each sentence.
    pub capitalize_sentences: bool,
    /// Remove a single trailing period (for chat/messaging profiles).
    pub strip_trailing_period: bool,
    /// Apply NFC Unicode normalization.
    pub unicode_nfc: bool,
}

impl Default for NormalizeOptions {
    fn default() -> Self {
        Self {
            collapse_whitespace: true,
            spoken_punctuation: false,
            capitalize_sentences: false,
            strip_trailing_period: false,
            unicode_nfc: true,
        }
    }
}

/// Deterministic cleanup processor.
pub struct CleanupProcessor {
    opts: NormalizeOptions,
}

impl CleanupProcessor {
    /// New processor with the given options.
    pub fn new(opts: NormalizeOptions) -> Self {
        Self { opts }
    }
}

impl TextProcessor for CleanupProcessor {
    fn name(&self) -> &str {
        "cleanup"
    }

    fn process(&self, input: &str) -> String {
        let mut text = input.to_string();

        if self.opts.unicode_nfc {
            text = text.nfc().collect();
        }
        if self.opts.spoken_punctuation {
            text = apply_spoken_punctuation(&text);
        }
        if self.opts.collapse_whitespace {
            text = collapse_ws(&text);
        }
        if self.opts.capitalize_sentences {
            text = capitalize_sentences(&text);
        }
        if self.opts.strip_trailing_period {
            if let Some(stripped) = text.strip_suffix('.') {
                // only strip a lone final period, not "..." or "e.g."
                if !stripped.ends_with('.') {
                    text = stripped.trim_end().to_string();
                }
            }
        }
        text
    }
}

fn collapse_ws(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_space = false;
    for c in s.chars() {
        if c.is_whitespace() {
            if !prev_space {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(c);
            prev_space = false;
        }
    }
    out.trim().to_string()
}

fn apply_spoken_punctuation(s: &str) -> String {
    // Word-boundary replacements for common dictated punctuation. Case-insensitive on the keyword.
    // Applied only when surrounded by spaces / string ends to avoid mangling real words.
    const MAP: &[(&str, &str)] = &[
        ("new line", "\n"),
        ("new paragraph", "\n\n"),
        ("comma", ","),
        ("period", "."),
        ("full stop", "."),
        ("question mark", "?"),
        ("exclamation mark", "!"),
        ("exclamation point", "!"),
        ("colon", ":"),
        ("semicolon", ";"),
        ("open paren", "("),
        ("close paren", ")"),
        ("dash", "-"),
        ("hyphen", "-"),
    ];
    let mut tokens: Vec<String> = s.split(' ').map(|t| t.to_string()).collect();
    for tok in tokens.iter_mut() {
        let low = tok.to_lowercase();
        for (kw, sym) in MAP {
            // single-word keywords only in this per-token pass
            if !kw.contains(' ') && low == *kw {
                *tok = (*sym).to_string();
            }
        }
    }
    let mut joined = tokens.join(" ");
    // multi-word keywords on the joined string
    for (kw, sym) in MAP {
        if kw.contains(' ') {
            joined = replace_ci_word(&joined, kw, sym);
        }
    }
    // tidy spaces before punctuation like " ," -> ","
    joined = joined
        .replace(" ,", ",")
        .replace(" .", ".")
        .replace(" ?", "?")
        .replace(" !", "!")
        .replace(" :", ":")
        .replace(" ;", ";");
    joined
}

fn replace_ci_word(haystack: &str, needle: &str, replacement: &str) -> String {
    let low_h = haystack.to_lowercase();
    let low_n = needle.to_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut idx = 0;
    while let Some(pos) = low_h[idx..].find(&low_n) {
        let start = idx + pos;
        let end = start + low_n.len();
        out.push_str(&haystack[idx..start]);
        out.push_str(replacement);
        idx = end;
    }
    out.push_str(&haystack[idx..]);
    out
}

fn capitalize_sentences(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut capitalize_next = true;
    for c in s.chars() {
        if capitalize_next && c.is_alphabetic() {
            out.extend(c.to_uppercase());
            capitalize_next = false;
        } else {
            out.push(c);
            if matches!(c, '.' | '!' | '?' | '\n') {
                capitalize_next = true;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_unchanged() {
        assert_eq!(IdentityProcessor.process("Hello."), "Hello.");
    }

    #[test]
    fn collapses_whitespace() {
        let p = CleanupProcessor::new(NormalizeOptions::default());
        assert_eq!(p.process("  a   b\t c \n"), "a b c");
    }

    #[test]
    fn spoken_punctuation_maps() {
        let p = CleanupProcessor::new(NormalizeOptions {
            spoken_punctuation: true,
            ..Default::default()
        });
        assert_eq!(p.process("hello comma world period"), "hello, world.");
    }

    #[test]
    fn capitalization() {
        let p = CleanupProcessor::new(NormalizeOptions {
            capitalize_sentences: true,
            ..Default::default()
        });
        assert_eq!(p.process("hello. how are you?"), "Hello. How are you?");
    }

    #[test]
    fn strip_trailing_period_only_single() {
        let p = CleanupProcessor::new(NormalizeOptions {
            strip_trailing_period: true,
            ..Default::default()
        });
        assert_eq!(p.process("hi there."), "hi there");
        assert_eq!(p.process("wait..."), "wait...");
    }

    #[test]
    fn does_not_capitalize_when_disabled() {
        let p = CleanupProcessor::new(NormalizeOptions::default());
        assert_eq!(p.process("hello world"), "hello world");
    }
}
