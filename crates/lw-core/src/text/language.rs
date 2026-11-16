//! Languages used by deterministic cleanup and the local translation picker.

use serde::{Deserialize, Serialize};

/// A text language. Automatic is a source-language preference, never a translation target.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextLanguage {
    /// Prefer the engine's language, otherwise use conservative script/word hints.
    #[default]
    Auto,
    /// English.
    English,
    /// Russian.
    Russian,
    /// Ukrainian.
    Ukrainian,
    /// Spanish.
    Spanish,
    /// Italian.
    Italian,
    /// French.
    French,
    /// German.
    German,
    /// Portuguese.
    Portuguese,
    /// Dutch.
    Dutch,
    /// Polish.
    Polish,
    /// Czech.
    Czech,
    /// Turkish.
    Turkish,
    /// Mandarin Chinese, simplified output for translation.
    Chinese,
    /// Japanese.
    Japanese,
    /// Korean.
    Korean,
    /// Arabic.
    Arabic,
    /// Hindi.
    Hindi,
}

impl TextLanguage {
    /// Source choices, including automatic detection.
    pub const ALL: [Self; 18] = [
        Self::Auto,
        Self::English,
        Self::Russian,
        Self::Ukrainian,
        Self::Spanish,
        Self::Italian,
        Self::French,
        Self::German,
        Self::Portuguese,
        Self::Dutch,
        Self::Polish,
        Self::Czech,
        Self::Turkish,
        Self::Chinese,
        Self::Japanese,
        Self::Korean,
        Self::Arabic,
        Self::Hindi,
    ];

    /// Stable language code.
    pub fn code(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::English => "en",
            Self::Russian => "ru",
            Self::Ukrainian => "uk",
            Self::Spanish => "es",
            Self::Italian => "it",
            Self::French => "fr",
            Self::German => "de",
            Self::Portuguese => "pt",
            Self::Dutch => "nl",
            Self::Polish => "pl",
            Self::Czech => "cs",
            Self::Turkish => "tr",
            Self::Chinese => "zh",
            Self::Japanese => "ja",
            Self::Korean => "ko",
            Self::Arabic => "ar",
            Self::Hindi => "hi",
        }
    }

    /// Human-readable picker label; also used in the translator's task prompt.
    pub fn label(self) -> &'static str {
        match self {
            Self::Auto => "Automatic",
            Self::English => "English",
            Self::Russian => "Russian",
            Self::Ukrainian => "Ukrainian",
            Self::Spanish => "Spanish",
            Self::Italian => "Italian",
            Self::French => "French",
            Self::German => "German",
            Self::Portuguese => "Portuguese",
            Self::Dutch => "Dutch",
            Self::Polish => "Polish",
            Self::Czech => "Czech",
            Self::Turkish => "Turkish",
            Self::Chinese => "Chinese (Simplified)",
            Self::Japanese => "Japanese",
            Self::Korean => "Korean",
            Self::Arabic => "Arabic",
            Self::Hindi => "Hindi",
        }
    }

    /// ISO code, including engine codes such as `en-US` or `zh-Hans`.
    pub fn from_code(code: &str) -> Option<Self> {
        let base = code.split(['-', '_']).next().unwrap_or(code);
        Self::ALL
            .into_iter()
            .find(|l| l.code().eq_ignore_ascii_case(base))
    }

    /// Hesitations only; discourse words such as "like", "типа", "donc" or "那个" stay intact.
    pub(crate) fn fillers(self) -> &'static [&'static str] {
        match self {
            Self::Auto => &[],
            Self::English => &["um", "uh", "umm", "uhh"],
            Self::Russian => &["эм", "эмм", "ээ", "эээ", "э-э", "э-э-э"],
            Self::Ukrainian => &["ем", "емм", "ее", "е-е", "е-е-е", "эм"],
            Self::Spanish => &["eh", "ehm", "eem"],
            Self::Italian => &["ehm", "uhm", "emh"],
            Self::French => &["euh", "euhm", "heu"],
            Self::German => &["äh", "ähm", "öh", "öhm"],
            Self::Portuguese => &["ãh", "ahn", "humm"],
            Self::Dutch => &["eh", "ehm", "uh", "uhm"],
            Self::Polish => &["yyy", "eee", "y-y-y"],
            Self::Czech => &["ehm", "éé", "eee"],
            Self::Turkish => &["ıı", "ııı", "ıııı"],
            Self::Chinese => &["呃", "呃呃"],
            Self::Japanese => &["えーと", "ええと"],
            Self::Korean => &["음", "으음"],
            Self::Arabic => &["أمم", "اممم", "أممم"],
            Self::Hindi => &["उम्म", "अं"],
        }
    }
}

impl std::fmt::Display for TextLanguage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.label())
    }
}

/// Choose a cleanup language without loading another model. Ambiguous Latin text stays automatic.
pub fn cleanup_language(input: &str, preferred: TextLanguage, reported: Option<&str>) -> TextLanguage {
    if preferred != TextLanguage::Auto {
        return preferred;
    }
    if let Some(language) = reported
        .and_then(TextLanguage::from_code)
        .filter(|l| *l != TextLanguage::Auto)
    {
        return language;
    }
    if input.chars().any(|c| matches!(c, '\u{3040}'..='\u{30ff}')) {
        return TextLanguage::Japanese;
    }
    if input.chars().any(|c| matches!(c, '\u{ac00}'..='\u{d7af}')) {
        return TextLanguage::Korean;
    }
    if input.chars().any(|c| matches!(c, '\u{4e00}'..='\u{9fff}')) {
        return TextLanguage::Chinese;
    }
    if input.chars().any(|c| matches!(c, '\u{0900}'..='\u{097f}')) {
        return TextLanguage::Hindi;
    }
    if input.chars().any(|c| matches!(c, '\u{0600}'..='\u{06ff}')) {
        return TextLanguage::Arabic;
    }
    if input
        .chars()
        .any(|c| matches!(c, 'і' | 'ї' | 'є' | 'ґ' | 'І' | 'Ї' | 'Є' | 'Ґ'))
    {
        return TextLanguage::Ukrainian;
    }
    if input.chars().any(|c| matches!(c, '\u{0400}'..='\u{04ff}')) {
        return TextLanguage::Russian;
    }
    const HINTS: &[(TextLanguage, &[&str])] = &[
        (
            TextLanguage::English,
            &[
                "the", "this", "that", "please", "hello", "send", "can", "you", "my", "is", "with",
                "tomorrow",
            ],
        ),
        (
            TextLanguage::Spanish,
            &[
                "hola", "necesito", "mañana", "revisa", "informe", "quiero", "esto", "gracias", "para",
                "los", "las",
            ],
        ),
        (
            TextLanguage::Italian,
            &[
                "ciao", "vorrei", "domani", "questo", "grazie", "sono", "puoi", "favore", "della", "gli",
            ],
        ),
        (
            TextLanguage::French,
            &[
                "bonjour", "je", "veux", "demain", "merci", "vous", "avec", "pour", "les", "est", "envoyer",
            ],
        ),
        (
            TextLanguage::German,
            &[
                "ich", "bitte", "morgen", "datei", "acht", "nicht", "das", "die", "und", "ist", "möchte",
            ],
        ),
        (
            TextLanguage::Portuguese,
            &[
                "olá", "amanhã", "obrigado", "quero", "você", "não", "preciso", "arquivo", "favor", "isso",
            ],
        ),
        (
            TextLanguage::Dutch,
            &[
                "hallo", "graag", "morgen", "bestand", "niet", "het", "een", "ik", "zijn", "stuur",
            ],
        ),
        (
            TextLanguage::Polish,
            &[
                "proszę",
                "jutro",
                "plik",
                "chcę",
                "jest",
                "dziękuję",
                "cześć",
                "wyślij",
            ],
        ),
        (
            TextLanguage::Czech,
            &["prosím", "zítra", "soubor", "chci", "děkuji", "ahoj", "pošli"],
        ),
        (
            TextLanguage::Turkish,
            &[
                "merhaba",
                "lütfen",
                "yarın",
                "dosya",
                "istiyorum",
                "teşekkürler",
                "bu",
                "bir",
            ],
        ),
    ];
    let words: Vec<String> = input
        .split(|c: char| !c.is_alphabetic())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut best = (TextLanguage::Auto, 0usize);
    let mut tied = false;
    for &(language, hints) in HINTS {
        let score = words.iter().filter(|w| hints.contains(&w.as_str())).count();
        if score > best.1 {
            best = (language, score);
            tied = false;
        } else if score == best.1 && score > 0 {
            tied = true;
        }
    }
    if best.1 > 0 && !tied {
        return best.0;
    }
    // Recognizable non-lexical forms help on very short dictations. Bare `um` is deliberately
    // excluded: it is a German preposition. Ambiguous text can be assigned a language in Settings.
    for &(language, forms) in &[
        (TextLanguage::English, &["uh", "umm", "uhh"][..]),
        (TextLanguage::German, &["äh", "ähm", "öhm"][..]),
        (TextLanguage::French, &["euh", "euhm"][..]),
        (TextLanguage::Italian, &["ehm", "uhm"][..]),
    ] {
        if words.iter().any(|w| forms.contains(&w.as_str())) {
            return language;
        }
    }
    TextLanguage::Auto
}
