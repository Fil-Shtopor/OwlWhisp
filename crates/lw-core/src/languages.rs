//! Human-readable names for the language codes the catalog uses.
//!
//! The catalog stores ISO-639-1 codes because that is what the model publishers use and what the
//! engines emit, but a list reading `af, am, ar, as, az, ba, …` is unusable for the one question a
//! reader actually has: *is my language in there?* Nobody scans a hundred two-letter codes looking
//! for `uk`. The names live here rather than in the frontend so `lw models info` and the app say
//! the same thing, and so a code that has no name is visible in one place.

/// Code to English name. Sorted by code; looked up with a binary search.
static NAMES: &[(&str, &str)] = &[
    ("af", "Afrikaans"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("as", "Assamese"),
    ("ast", "Asturian"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("be", "Belarusian"),
    ("bg", "Bulgarian"),
    ("bn", "Bengali"),
    ("bo", "Tibetan"),
    ("br", "Breton"),
    ("bs", "Bosnian"),
    ("ca", "Catalan"),
    ("ceb", "Cebuano"),
    ("cs", "Czech"),
    ("cy", "Welsh"),
    ("da", "Danish"),
    ("de", "German"),
    ("el", "Greek"),
    ("el-GR", "Greek"),
    ("en", "English"),
    ("es", "Spanish"),
    ("et", "Estonian"),
    ("eu", "Basque"),
    ("fa", "Persian"),
    ("fi", "Finnish"),
    ("fil", "Filipino"),
    ("fo", "Faroese"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("gu", "Gujarati"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hr", "Croatian"),
    ("ht", "Haitian Creole"),
    ("hu", "Hungarian"),
    ("hy", "Armenian"),
    ("id", "Indonesian"),
    ("ig", "Igbo"),
    ("is", "Icelandic"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("ka", "Georgian"),
    ("kea", "Kabuverdianu"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("kn", "Kannada"),
    ("ko", "Korean"),
    ("ky", "Kyrgyz"),
    ("la", "Latin"),
    ("lb", "Luxembourgish"),
    ("lg", "Ganda"),
    ("ln", "Lingala"),
    ("lo", "Lao"),
    ("lt", "Lithuanian"),
    ("lv", "Latvian"),
    ("mg", "Malagasy"),
    ("mi", "Maori"),
    ("mi-NZ", "Maori"),
    ("mk", "Macedonian"),
    ("ml", "Malayalam"),
    ("mn", "Mongolian"),
    ("mr", "Marathi"),
    ("ms", "Malay"),
    ("mt", "Maltese"),
    ("my", "Burmese"),
    ("nb", "Norwegian Bokmal"),
    ("ne", "Nepali"),
    ("nl", "Dutch"),
    ("nn", "Norwegian Nynorsk"),
    ("no", "Norwegian"),
    ("nso", "Northern Sotho"),
    ("ny", "Chichewa"),
    ("oc", "Occitan"),
    ("om", "Oromo"),
    ("pa", "Punjabi"),
    ("pl", "Polish"),
    ("ps", "Pashto"),
    ("pt", "Portuguese"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sd", "Sindhi"),
    ("sg", "Sango"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("sn", "Shona"),
    ("so", "Somali"),
    ("sq", "Albanian"),
    ("sr", "Serbian"),
    ("st", "Southern Sotho"),
    ("su", "Sundanese"),
    ("sv", "Swedish"),
    ("sw", "Swahili"),
    ("ta", "Tamil"),
    ("te", "Telugu"),
    ("tg", "Tajik"),
    ("th", "Thai"),
    ("tk", "Turkmen"),
    ("tl", "Tagalog"),
    ("tn", "Tswana"),
    ("tr", "Turkish"),
    ("tt", "Tatar"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("wo", "Wolof"),
    ("xh", "Xhosa"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
    ("yue", "Cantonese"),
    ("zh", "Chinese"),
    ("zh-TW", "Chinese (Traditional)"),
    ("zu", "Zulu"),
];

/// The English name for a language code, or `None` when this table does not have one.
///
/// Returning `None` rather than a placeholder is deliberate: the caller decides whether to show
/// the bare code, and a missing name is a gap in this table rather than a fact about the model.
pub fn language_name(code: &str) -> Option<&'static str> {
    let key = code.trim();
    NAMES
        .binary_search_by(|(c, _)| (*c).cmp(key))
        .ok()
        .map(|i| NAMES[i].1)
}

/// The name if there is one, otherwise the code itself, so a list never has a hole in it.
pub fn language_label(code: &str) -> String {
    language_name(code).map_or_else(|| code.trim().to_string(), str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Catalog;

    #[test]
    fn the_table_is_sorted_so_the_binary_search_is_valid() {
        for pair in NAMES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{} must sort before {}",
                pair[0].0,
                pair[1].0
            );
        }
    }

    #[test]
    fn every_language_in_the_shipped_catalog_has_a_name() {
        // The point of the table. A catalog entry whose languages render as bare codes defeats it,
        // and adding a model is exactly when that happens.
        let catalog = Catalog::builtin().unwrap();
        let mut missing: Vec<&str> = Vec::new();
        for entry in catalog.iter() {
            for code in &entry.languages {
                if language_name(code).is_none() {
                    missing.push(code);
                }
            }
        }
        missing.sort_unstable();
        missing.dedup();
        assert!(missing.is_empty(), "no name for: {missing:?}");
    }

    #[test]
    fn an_unknown_code_falls_back_to_itself() {
        assert_eq!(language_name("zz"), None);
        assert_eq!(language_label("zz"), "zz");
        assert_eq!(language_label(" ru "), "Russian");
    }
}
