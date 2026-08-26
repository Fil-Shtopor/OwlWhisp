//! Word-level overlap merge for chunked long-audio transcription.
//!
//! When audio is split into overlapping windows and each is decoded independently, adjacent chunk
//! texts overlap. This joins them by finding the longest word-level overlap between the tail of the
//! accumulated text and the head of the next chunk, allowing up to a few trailing words of the
//! previous chunk to be discarded as right-edge hallucination (the OpenWritr heuristic).

/// Maximum trailing words of the previous chunk that may be dropped as hallucinated right-context.
const MAX_TRAILING_DROP: usize = 6;

/// Normalize a word for comparison: lowercase, alphanumerics only.
fn norm(w: &str) -> String {
    w.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(|c| c.to_lowercase())
        .collect()
}

/// Merge `next` onto `acc`, removing the overlap. Both are plain transcript strings.
pub fn merge(acc: &str, next: &str) -> String {
    if acc.is_empty() {
        return next.to_string();
    }
    if next.is_empty() {
        return acc.to_string();
    }
    let a_words: Vec<&str> = acc.split_whitespace().collect();
    let b_words: Vec<&str> = next.split_whitespace().collect();
    let a_norm: Vec<String> = a_words.iter().map(|w| norm(w)).collect();
    let b_norm: Vec<String> = b_words.iter().map(|w| norm(w)).collect();

    let max_overlap = a_words.len().min(b_words.len());
    for overlap in (1..=max_overlap).rev() {
        for trailing in 0..=MAX_TRAILING_DROP.min(a_words.len().saturating_sub(overlap)) {
            let a_end = a_words.len() - trailing;
            if a_end < overlap {
                continue;
            }
            let a_tail = &a_norm[a_end - overlap..a_end];
            let b_head = &b_norm[..overlap];
            if a_tail == b_head {
                let mut merged: Vec<&str> = a_words[..a_end].to_vec();
                merged.extend_from_slice(&b_words[overlap..]);
                return merged.join(" ");
            }
        }
    }
    // No overlap found: concatenate with a space.
    format!("{} {}", acc.trim_end(), next.trim_start())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_cases() {
        assert_eq!(merge("", "hello"), "hello");
        assert_eq!(merge("hello", ""), "hello");
    }

    #[test]
    fn simple_overlap() {
        assert_eq!(
            merge("the quick brown", "brown fox jumps"),
            "the quick brown fox jumps"
        );
    }

    #[test]
    fn multi_word_overlap() {
        assert_eq!(merge("a b c d e", "d e f g"), "a b c d e f g");
    }

    #[test]
    fn drops_trailing_hallucination() {
        // "xyz" is a hallucinated trailing word in the first chunk.
        assert_eq!(merge("hello world xyz", "world again"), "hello world again");
    }

    #[test]
    fn no_overlap_concatenates() {
        assert_eq!(merge("hello", "goodbye"), "hello goodbye");
    }

    #[test]
    fn overlap_is_case_insensitive() {
        assert_eq!(merge("The Quick Brown", "brown FOX"), "The Quick Brown FOX");
    }
}
