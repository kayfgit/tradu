//! Romanized input for languages with their own script: recognising romaji, and
//! rewriting Latin text the way an input method expects so it converts cleanly.

const VOWELS: &[u8] = b"aeiou";

/// Consonant onsets of a romaji syllable, longest first.
const ONSETS: &[&str] = &[
    "ky", "gy", "sh", "sy", "ch", "cy", "ty", "dy", "ny", "hy", "by", "py", "my", "ry", "jy",
    "ts", "dz", "k", "g", "s", "z", "t", "d", "n", "h", "b", "p", "m", "y", "r", "w", "j", "f",
    "v",
];

/// Whether every word of `text` reads as Japanese romaji ("arigatou",
/// "kore wa nan desu ka"). Plenty of short English words do too, so callers only
/// trust this when the language detector also missed.
pub fn looks_like_romaji(text: &str) -> bool {
    if !text.is_ascii() {
        return false;
    }
    let mut words = text
        .split(|c: char| !(c.is_ascii_alphabetic() || c == '\'' || c == '-'))
        .filter(|w| !w.is_empty())
        .peekable();
    words.peek().is_some() && words.all(|w| is_romaji_word(&w.to_ascii_lowercase()))
}

fn is_romaji_word(word: &str) -> bool {
    let b = word.as_bytes();
    let mut i = 0;
    while i < b.len() {
        let rest = &b[i..];
        let next = rest.get(1).copied();
        let vowel_next = next.is_some_and(|c| VOWELS.contains(&c));
        if VOWELS.contains(&rest[0]) || rest[0] == b'-' || rest[0] == b'\'' {
            i += 1; // vowel, long-vowel mark (ra-men), or n' separator
        } else if rest[0] == b'n' && !vowel_next && next != Some(b'y') {
            i += 1; // syllabic ん: before a consonant or at the end
        } else if next == Some(rest[0]) && b"kgsztdhbpmrjfcw".contains(&rest[0]) {
            i += 1; // doubled consonant (small っ): gakkou, matte
        } else if rest.starts_with(b"tch") {
            i += 1; // matcha
        } else {
            let onset = ONSETS.iter().find(|o| {
                rest.starts_with(o.as_bytes()) && rest.get(o.len()).is_some_and(|c| VOWELS.contains(c))
            });
            match onset {
                Some(o) => i += o.len() + 1,
                None => return false,
            }
        }
    }
    true
}

/// Input-method code for languages whose romanized input we convert ourselves.
pub fn input_method(lang: &str) -> Option<&'static str> {
    match lang {
        "ja" => Some("ja-t-i0-und"),
        "zh-CN" | "zh" => Some("zh-t-i0-pinyin"),
        "zh-TW" => Some("zh-hant-t-i0-pinyin"),
        _ => None,
    }
}

/// Rewrites romanized `text` for an input method: words joined (the IME needs the
/// context), Japanese particles typed the way IMEs expect (は "ha", を "wo",
/// へ "he" — learners write "wa", "o", "e"), and punctuation in full width so
/// it survives conversion.
pub fn prepare(text: &str, lang: &str) -> String {
    let japanese = lang == "ja";
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    let flush = |word: &mut String, out: &mut String| {
        let lower = word.to_ascii_lowercase();
        out.push_str(match lower.as_str() {
            "wa" if japanese => "ha",
            "o" | "wo" if japanese => "wo",
            "e" if japanese => "he",
            _ => &lower,
        });
        word.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() || c == '-' || c == '\'' {
            word.push(c);
            continue;
        }
        flush(&mut word, &mut out);
        match (c, japanese) {
            (c, _) if c.is_whitespace() => {}
            ('.', true) => out.push('。'),
            (',', true) => out.push('、'),
            ('.', false) => out.push('。'),
            (',', false) => out.push('，'),
            ('?', _) => out.push('？'),
            ('!', _) => out.push('！'),
            (':', _) => out.push('：'),
            (';', _) => out.push('；'),
            (c, _) => out.push(c),
        }
    }
    flush(&mut word, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_romaji() {
        for text in ["arigatou", "konnichiwa", "kore wa nan desu ka?", "gakkou e ikimasu", "ra-men", "matcha", "shinbun", "kyou wa ii tenki desu ne"] {
            assert!(looks_like_romaji(text), "{text}");
        }
    }

    #[test]
    fn rejects_other_languages() {
        for text in ["hello", "gracias amigo", "läuft", "the quick brown fox", "привет", "", "123"] {
            assert!(!looks_like_romaji(text), "{text}");
        }
    }

    #[test]
    fn prepares_japanese_particles_and_punctuation() {
        assert_eq!(prepare("watashi wa gakusei desu.", "ja"), "watashihagakuseidesu。");
        assert_eq!(prepare("Kore wa nan desu ka?", "ja"), "korehanandesuka？");
        assert_eq!(prepare("hon o yomimasu, gakkou e ikimasu", "ja"), "honwoyomimasu、gakkouheikimasu");
        // Only standalone words are particles.
        assert_eq!(prepare("konnichiwa", "ja"), "konnichiwa");
    }

    #[test]
    fn prepares_pinyin_without_particle_rules() {
        assert_eq!(prepare("ni hao, wo shi xuesheng.", "zh-CN"), "nihao，woshixuesheng。");
    }
}
