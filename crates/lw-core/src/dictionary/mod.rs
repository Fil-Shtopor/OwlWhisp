//! User-editable replacement dictionary.
//!
//! Two kinds of entries:
//! - **exact**: the whole phrase must match (case-insensitively at word boundaries); the
//!   replacement is emitted verbatim (e.g. `"open wiser" → "OpenWritr"`).
//! - **word**: a single spoken word mapped to a term, matched at word boundaries, with optional
//!   case preservation of the surrounding capitalization.
//!
//! Longer phrases are applied before shorter ones so `"play write"` wins over `"write"`.

use serde::{Deserialize, Serialize};

/// One replacement rule.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Rule {
    /// The spoken form to match (case-insensitive).
    pub from: String,
    /// The replacement to emit.
    pub to: String,
    /// Whether to preserve the matched text's leading capitalization.
    #[serde(default)]
    pub case_aware: bool,
}

/// A collection of replacement rules.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Dictionary {
    /// The rules, applied longest-phrase-first.
    pub rules: Vec<Rule>,
}

impl Dictionary {
    /// An empty dictionary.
    pub fn new() -> Self {
        Self { rules: Vec::new() }
    }

    /// Add an exact phrase replacement.
    pub fn add_exact(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.rules.push(Rule {
            from: from.into(),
            to: to.into(),
            case_aware: false,
        });
    }

    /// Add a case-aware word replacement.
    pub fn add_case_aware(&mut self, from: impl Into<String>, to: impl Into<String>) {
        self.rules.push(Rule {
            from: from.into(),
            to: to.into(),
            case_aware: true,
        });
    }

    /// Apply all rules to `input`. Word-boundary, case-insensitive matching.
    pub fn apply(&self, input: &str) -> String {
        let mut rules: Vec<&Rule> = self.rules.iter().collect();
        // Longest `from` first so multi-word phrases beat their prefixes.
        rules.sort_by_key(|r| std::cmp::Reverse(r.from.chars().count()));

        let mut text = input.to_string();
        for rule in rules {
            if rule.from.is_empty() {
                continue;
            }
            text = replace_word_ci(&text, &rule.from, &rule.to, rule.case_aware);
        }
        text
    }
}

/// Case-insensitive, word-boundary replacement. A boundary is start/end of string or a non-alnum
/// char. When `case_aware`, if the match starts uppercase the replacement's first letter is
/// uppercased.
fn replace_word_ci(haystack: &str, needle: &str, replacement: &str, case_aware: bool) -> String {
    let h: Vec<char> = haystack.chars().collect();
    let n: Vec<char> = needle.chars().collect();
    let n_low: Vec<char> = needle.to_lowercase().chars().collect();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0usize;
    while i < h.len() {
        if i + n.len() <= h.len() {
            let window: String = h[i..i + n.len()].iter().collect();
            let window_low: Vec<char> = window.to_lowercase().chars().collect();
            let matches = window_low == n_low;
            let left_ok = i == 0 || !h[i - 1].is_alphanumeric();
            let right_idx = i + n.len();
            let right_ok = right_idx >= h.len() || !h[right_idx].is_alphanumeric();
            if matches && left_ok && right_ok {
                let repl = if case_aware && h[i].is_uppercase() {
                    let mut c = replacement.chars();
                    match c.next() {
                        Some(first) => first.to_uppercase().collect::<String>() + c.as_str(),
                        None => String::new(),
                    }
                } else {
                    replacement.to_string()
                };
                out.push_str(&repl);
                i += n.len();
                continue;
            }
        }
        out.push(h[i]);
        i += 1;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_phrase_replacement() {
        let mut d = Dictionary::new();
        d.add_exact("open wiser", "OpenWritr");
        assert_eq!(d.apply("i use open wiser daily"), "i use OpenWritr daily");
    }

    #[test]
    fn longest_phrase_wins() {
        let mut d = Dictionary::new();
        d.add_exact("play write", "Playwright");
        d.add_exact("write", "WRITE");
        assert_eq!(d.apply("run play write tests"), "run Playwright tests");
    }

    #[test]
    fn word_boundary_respected() {
        let mut d = Dictionary::new();
        d.add_exact("cat", "dog");
        assert_eq!(d.apply("the cat category"), "the dog category");
    }

    #[test]
    fn case_insensitive_match() {
        let mut d = Dictionary::new();
        d.add_exact("typescript", "TypeScript");
        assert_eq!(d.apply("TypeScript and typescript"), "TypeScript and TypeScript");
    }

    #[test]
    fn case_aware_preserves_leading_capital() {
        let mut d = Dictionary::new();
        d.add_case_aware("github", "GitHub");
        assert_eq!(d.apply("Github and github"), "GitHub and GitHub");
    }

    #[test]
    fn roundtrips_through_json() {
        let mut d = Dictionary::new();
        d.add_exact("a", "b");
        let json = serde_json::to_string(&d).unwrap();
        let back: Dictionary = serde_json::from_str(&json).unwrap();
        assert_eq!(back.rules.len(), 1);
    }
}
