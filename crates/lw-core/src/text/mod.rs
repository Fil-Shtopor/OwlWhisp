//! Text post-processing pipeline: `audio → STT → normalize → dictionary → punctuation →
//! [optional LLM] → final text`.
//!
//! Each stage implements [`TextProcessor`]. Stages are cheap and deterministic except the optional
//! LLM stage, which is off by default and always falls back to its input on any failure — the app
//! works fully offline with the LLM disabled.

mod normalize;

pub use normalize::{CleanupProcessor, IdentityProcessor, NormalizeOptions};

use crate::dictionary::Dictionary;

/// A text transformation stage.
pub trait TextProcessor: Send {
    /// A short name for diagnostics.
    fn name(&self) -> &str;
    /// Transform the text. Must never panic; on internal failure, return the input unchanged.
    fn process(&self, input: &str) -> String;
}

/// Applies a [`Dictionary`] of replacements as a pipeline stage.
pub struct DictionaryProcessor {
    dict: Dictionary,
}

impl DictionaryProcessor {
    /// Wrap a dictionary.
    pub fn new(dict: Dictionary) -> Self {
        Self { dict }
    }
}

impl TextProcessor for DictionaryProcessor {
    fn name(&self) -> &str {
        "dictionary"
    }
    fn process(&self, input: &str) -> String {
        self.dict.apply(input)
    }
}

/// An ordered pipeline of processors.
pub struct TextPipeline {
    stages: Vec<Box<dyn TextProcessor>>,
}

impl TextPipeline {
    /// Empty pipeline (identity).
    pub fn new() -> Self {
        Self { stages: Vec::new() }
    }

    /// Add a stage (builder style).
    pub fn with(mut self, stage: Box<dyn TextProcessor>) -> Self {
        self.stages.push(stage);
        self
    }

    /// Push a stage.
    pub fn push(&mut self, stage: Box<dyn TextProcessor>) {
        self.stages.push(stage);
    }

    /// Names of the stages, in order.
    pub fn stage_names(&self) -> Vec<&str> {
        self.stages.iter().map(|s| s.name()).collect()
    }

    /// Run all stages in order.
    pub fn run(&self, input: &str) -> String {
        let mut text = input.to_string();
        for stage in &self.stages {
            text = stage.process(&text);
        }
        text
    }
}

impl Default for TextPipeline {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dictionary::Dictionary;

    #[test]
    fn empty_pipeline_is_identity() {
        let p = TextPipeline::new();
        assert_eq!(p.run("hello world"), "hello world");
    }

    #[test]
    fn pipeline_runs_in_order() {
        let mut dict = Dictionary::new();
        dict.add_exact("playwright test", "Playwright test");
        let p = TextPipeline::new()
            .with(Box::new(CleanupProcessor::new(NormalizeOptions::default())))
            .with(Box::new(DictionaryProcessor::new(dict)));
        // cleanup collapses spaces, dictionary fixes the term
        let out = p.run("  playwright   test  ");
        assert_eq!(out, "Playwright test");
    }
}
