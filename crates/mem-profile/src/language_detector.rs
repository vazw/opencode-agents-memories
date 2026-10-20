//! Pure language detection for profile learning (W3.2).
//!
//! Port of upstream `src/services/language-detector.ts` with the PRD-ratified
//! mechanism swap: `whatlang` (ISO 639-3, 69 languages) replaces `franc-min`
//! (the D-W1 ruling). Everything above this module still speaks ISO 639-1
//! codes and English language names, exactly like upstream's callers, so the
//! [`LANGUAGES`] table is the only bridging surface.
//!
//! Measured consequences pinned by the upstream corpus (`tests/language_detector.rs`):
//! - `whatlang` classifies `"ab"` as Estonian with 0.005 confidence, so the
//!   `< 5 chars` guard mirrors upstream's `franc(..., { minLength: 5 })`;
//! - `is_reliable()` is **not** a filter — short English is deliberately
//!   unreliable (0.411) and must still resolve to `en`;
//! - detection runs on the joined **raw** prompt contents, before any
//!   truncation and before the English analysis scaffold is added.

use crate::prompt::UserPrompt;

/// A resolved output language, carried as its ISO 639-1 code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Language(&'static str);

impl Language {
    /// The fallback language for empty/undetectable/unmapped input.
    pub const ENGLISH: Language = Language("en");

    /// The ISO 639-1 code (`"en"`, `"zh"`, …).
    pub fn code(self) -> &'static str {
        self.0
    }

    /// The English name of the language (`"English"`, `"Chinese"`, …).
    pub fn name(self) -> &'static str {
        get_language_name(self.0)
    }
}

impl Default for Language {
    fn default() -> Self {
        Self::ENGLISH
    }
}

/// `whatlang`'s 69 detectible languages mapped to `(ISO 639-1, English name)`.
///
/// `whatlang` has no ISO 639-1 surface of its own, so this table is the
/// bridging contract (upstream used `iso-639-3`'s `iso6393To1` map plus a
/// 4-entry fallback map for languages without a 639-1 code).
pub(crate) const LANGUAGES: &[(whatlang::Lang, &str, &str)] = &[
    (whatlang::Lang::Epo, "eo", "Esperanto"),
    (whatlang::Lang::Eng, "en", "English"),
    (whatlang::Lang::Rus, "ru", "Russian"),
    (whatlang::Lang::Cmn, "zh", "Chinese"),
    (whatlang::Lang::Spa, "es", "Spanish"),
    (whatlang::Lang::Por, "pt", "Portuguese"),
    (whatlang::Lang::Ita, "it", "Italian"),
    (whatlang::Lang::Ben, "bn", "Bengali"),
    (whatlang::Lang::Fra, "fr", "French"),
    (whatlang::Lang::Deu, "de", "German"),
    (whatlang::Lang::Ukr, "uk", "Ukrainian"),
    (whatlang::Lang::Kat, "ka", "Georgian"),
    (whatlang::Lang::Ara, "ar", "Arabic"),
    (whatlang::Lang::Hin, "hi", "Hindi"),
    (whatlang::Lang::Jpn, "ja", "Japanese"),
    (whatlang::Lang::Heb, "he", "Hebrew"),
    (whatlang::Lang::Yid, "yi", "Yiddish"),
    (whatlang::Lang::Pol, "pl", "Polish"),
    (whatlang::Lang::Amh, "am", "Amharic"),
    (whatlang::Lang::Jav, "jv", "Javanese"),
    (whatlang::Lang::Kor, "ko", "Korean"),
    (whatlang::Lang::Nob, "nb", "Norwegian Bokmål"),
    (whatlang::Lang::Dan, "da", "Danish"),
    (whatlang::Lang::Swe, "sv", "Swedish"),
    (whatlang::Lang::Fin, "fi", "Finnish"),
    (whatlang::Lang::Tur, "tr", "Turkish"),
    (whatlang::Lang::Nld, "nl", "Dutch"),
    (whatlang::Lang::Hun, "hu", "Hungarian"),
    (whatlang::Lang::Ces, "cs", "Czech"),
    (whatlang::Lang::Ell, "el", "Greek"),
    (whatlang::Lang::Bul, "bg", "Bulgarian"),
    (whatlang::Lang::Bel, "be", "Belarusian"),
    (whatlang::Lang::Mar, "mr", "Marathi"),
    (whatlang::Lang::Kan, "kn", "Kannada"),
    (whatlang::Lang::Ron, "ro", "Romanian"),
    (whatlang::Lang::Slv, "sl", "Slovenian"),
    (whatlang::Lang::Hrv, "hr", "Croatian"),
    (whatlang::Lang::Srp, "sr", "Serbian"),
    (whatlang::Lang::Mkd, "mk", "Macedonian"),
    (whatlang::Lang::Lit, "lt", "Lithuanian"),
    (whatlang::Lang::Lav, "lv", "Latvian"),
    (whatlang::Lang::Est, "et", "Estonian"),
    (whatlang::Lang::Tam, "ta", "Tamil"),
    (whatlang::Lang::Vie, "vi", "Vietnamese"),
    (whatlang::Lang::Urd, "ur", "Urdu"),
    (whatlang::Lang::Tha, "th", "Thai"),
    (whatlang::Lang::Guj, "gu", "Gujarati"),
    (whatlang::Lang::Uzb, "uz", "Uzbek"),
    (whatlang::Lang::Pan, "pa", "Punjabi"),
    (whatlang::Lang::Aze, "az", "Azerbaijani"),
    (whatlang::Lang::Ind, "id", "Indonesian"),
    (whatlang::Lang::Tel, "te", "Telugu"),
    (whatlang::Lang::Pes, "fa", "Persian"),
    (whatlang::Lang::Mal, "ml", "Malayalam"),
    (whatlang::Lang::Ori, "or", "Odia"),
    (whatlang::Lang::Mya, "my", "Burmese"),
    (whatlang::Lang::Nep, "ne", "Nepali"),
    (whatlang::Lang::Sin, "si", "Sinhala"),
    (whatlang::Lang::Khm, "km", "Khmer"),
    (whatlang::Lang::Tuk, "tk", "Turkmen"),
    (whatlang::Lang::Aka, "ak", "Akan"),
    (whatlang::Lang::Zul, "zu", "Zulu"),
    (whatlang::Lang::Sna, "sn", "Shona"),
    (whatlang::Lang::Afr, "af", "Afrikaans"),
    (whatlang::Lang::Lat, "la", "Latin"),
    (whatlang::Lang::Slk, "sk", "Slovak"),
    (whatlang::Lang::Cat, "ca", "Catalan"),
    (whatlang::Lang::Tgl, "tl", "Tagalog"),
    (whatlang::Lang::Hye, "hy", "Armenian"),
];

/// Upstream `detectLanguage` — ISO 639-1 code for `text`, `en` when the input
/// is empty, whitespace-only, shorter than the detection floor, or unmapped.
///
/// The 5-char floor mirrors `franc`'s `minLength: 5`; confidence and
/// `is_reliable()` are deliberately ignored (see the module docs).
pub fn detect_language(text: &str) -> Language {
    if text.trim().is_empty() || text.chars().count() < 5 {
        return Language::ENGLISH;
    }
    match whatlang::detect(text) {
        Some(info) => language_for(info.lang()),
        None => Language::ENGLISH,
    }
}

/// The table row for a `whatlang` language, or `ENGLISH` when unmapped.
fn language_for(lang: whatlang::Lang) -> Language {
    LANGUAGES
        .iter()
        .find(|(candidate, _, _)| *candidate == lang)
        .map(|(_, code, _)| Language(code))
        .unwrap_or(Language::ENGLISH)
}

/// Upstream `getLanguageName` — English language name for an ISO 639-1 code
/// (case-insensitive), `"English"` for an unknown code.
pub fn get_language_name(code: &str) -> &'static str {
    LANGUAGES
        .iter()
        .find(|(_, candidate, _)| candidate.eq_ignore_ascii_case(code))
        .map(|(_, _, name)| *name)
        .unwrap_or("English")
}

/// The raw prompt contents joined exactly as upstream's
/// `prompts.map((p) => p.content).join("\n\n")`.
pub fn join_prompt_contents(prompts: &[UserPrompt]) -> String {
    let mut joined = String::new();
    for (index, prompt) in prompts.iter().enumerate() {
        if index > 0 {
            joined.push_str("\n\n");
        }
        joined.push_str(&prompt.content);
    }
    joined
}

/// Upstream `resolveProfileLanguageName`: the language the profile-analysis
/// model must write in.
///
/// `Some(value)` where `value` is neither empty nor `"auto"` is an explicit
/// override; everything else (`None`, `Some("")`, `Some("auto")`) detects on
/// the joined **raw** prompt contents — before truncation, and never on the
/// English instruction scaffold (which would bias short non-English prompts
/// toward English).
pub fn resolve_profile_language_name(
    prompts: &[UserPrompt],
    configured: Option<&str>,
) -> &'static str {
    match configured {
        Some(value) if !value.is_empty() && value != "auto" => get_language_name(value),
        _ => detect_language(&join_prompt_contents(prompts)).name(),
    }
}
