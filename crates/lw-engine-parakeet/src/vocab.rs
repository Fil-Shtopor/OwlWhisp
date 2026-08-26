//! SentencePiece-style vocabulary for Parakeet: loads `vocab.txt` and detokenizes token ids.
//!
//! `vocab.txt` has one entry per line. Most lines are `<piece> <id>`, but a piece may itself be a
//! space or contain spaces, so we split on the **last** space and only treat the tail as an id when
//! it parses as an integer. The unicode `▁` (U+2581) marks a word boundary → space. `<blk>` is the
//! blank id (8192 for v3). Special tokens (`<unk>`, `<pad>`, `<|...|>`) are dropped from output.

use crate::{Error, Result};

/// Blank token id for Parakeet TDT 0.6B v3 (vocab size 8192, blank = 8192).
pub const BLANK_ID: usize = 8192;

/// A loaded vocabulary.
#[derive(Clone, Debug)]
pub struct Vocab {
    pieces: Vec<String>,
    blank_id: usize,
}

impl Vocab {
    /// Load from a `vocab.txt` file.
    pub fn load(path: &std::path::Path) -> Result<Self> {
        let text =
            std::fs::read_to_string(path).map_err(|e| Error::Io(format!("{}: {e}", path.display())))?;
        Self::parse(&text)
    }

    /// Parse from the text of a `vocab.txt`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut pieces: Vec<String> = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                continue;
            }
            let piece = match line.rsplit_once(' ') {
                Some((head, tail)) if tail.parse::<i64>().is_ok() => head.to_string(),
                _ => line.to_string(),
            };
            pieces.push(piece);
        }
        if pieces.is_empty() {
            return Err(Error::Other("empty vocab".into()));
        }
        // blank is the last entry named "<blk>" if present, else index (len-1).
        let blank_id = pieces
            .iter()
            .position(|p| p == "<blk>")
            .unwrap_or(pieces.len() - 1);
        Ok(Self { pieces, blank_id })
    }

    /// Number of pieces.
    pub fn len(&self) -> usize {
        self.pieces.len()
    }

    /// Whether the vocab is empty (never true after a successful parse).
    pub fn is_empty(&self) -> bool {
        self.pieces.is_empty()
    }

    /// The blank id.
    pub fn blank_id(&self) -> usize {
        self.blank_id
    }

    /// Detokenize a sequence of ids into text.
    pub fn detokenize(&self, ids: &[usize]) -> String {
        let mut s = String::new();
        for &id in ids {
            if id >= self.pieces.len() {
                continue;
            }
            let piece = &self.pieces[id];
            if is_special(piece) {
                continue;
            }
            s.push_str(piece);
        }
        s.replace('\u{2581}', " ").trim().to_string()
    }
}

fn is_special(piece: &str) -> bool {
    matches!(piece, "<unk>" | "<pad>" | "<blk>" | "<s>" | "</s>")
        || (piece.starts_with("<|") && piece.ends_with("|>"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_id_suffixed_lines() {
        let v = Vocab::parse("<unk> 0\n\u{2581}the 5\ncat 6\n<blk> 7\n").unwrap();
        assert_eq!(v.len(), 4);
        assert_eq!(v.blank_id(), 3);
    }

    #[test]
    fn detokenizes_with_word_boundaries() {
        // pieces: 0 <unk>, 1 ▁the, 2 ▁cat, 3 s, 4 <blk>
        let v = Vocab::parse("<unk> 0\n\u{2581}the 1\n\u{2581}cat 2\ns 3\n<blk> 4\n").unwrap();
        assert_eq!(v.detokenize(&[1, 2, 3]), "the cats");
    }

    #[test]
    fn drops_special_tokens() {
        let v = Vocab::parse("<|nospeech|> 0\n\u{2581}hi 1\n<blk> 2\n").unwrap();
        assert_eq!(v.detokenize(&[0, 1]), "hi");
    }

    #[test]
    fn handles_piece_that_is_a_space_token() {
        // A line whose piece is "▁" and id 1
        let v = Vocab::parse("\u{2581} 1\n\u{2581}a 2\n<blk> 3\n").unwrap();
        assert_eq!(v.detokenize(&[0, 1]), "a");
    }
}
