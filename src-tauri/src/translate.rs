//! Translation through Google Translate's keyless endpoints.
//!
//! One `gtx` request returns the translation, romanization, detected language,
//! dictionary alternatives and Google's spelling/script correction. On top of it:
//! - romanized Japanese and Chinese go through Google's input method first, since
//!   `gtx` converts them word by word and garbles particles ("wa" → 和);
//! - when Google rate-limits us, requests stop for a cooldown instead of
//!   prolonging the block.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;

use crate::romaji;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";
const INPUT_TOOLS: &str = "https://inputtools.google.com/request";

/// The first cooldown after a 429; it doubles while Google keeps refusing.
const COOLDOWN_MIN: Duration = Duration::from_secs(30);
const COOLDOWN_MAX: Duration = Duration::from_secs(300);
/// Longest `Retry-After` we honor.
const RETRY_AFTER_MAX: Duration = Duration::from_secs(1800);

#[derive(Debug, Serialize, Default)]
pub struct Translation {
    pub text: String,
    /// Language Google detected for the input (always set, most useful with `from = auto`).
    pub detected: Option<String>,
    /// Romanization of the translation (e.g. "Ie" for 家).
    pub translit: Option<String>,
    /// Romanization of the input.
    pub src_translit: Option<String>,
    pub dict: Vec<DictEntry>,
    /// Set when what was translated isn't exactly what was typed.
    pub correction: Option<Correction>,
}

#[derive(Debug, Serialize)]
pub struct DictEntry {
    pub pos: String,
    pub terms: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct Correction {
    /// The text that was translated instead, e.g. "er läuft schnell" or "ありがとう".
    pub text: String,
    pub kind: CorrectionKind,
}

#[derive(Debug, Serialize, Clone, Copy, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum CorrectionKind {
    /// A typo or missing accents ("lauft" → "läuft").
    Spelling,
    /// Romanized input written in its own script ("arigatou" → "ありがとう").
    Script,
}

#[derive(Debug, Serialize)]
pub struct Error {
    pub message: String,
    /// Set while rate-limited: when it's worth trying again.
    pub retry_in_ms: Option<u64>,
}

impl From<String> for Error {
    fn from(message: String) -> Self {
        Self { message, retry_in_ms: None }
    }
}

pub struct Request<'a> {
    pub text: &'a str,
    pub from: &'a str,
    pub to: &'a str,
    /// The user's recent languages, which hint at what romanized input is.
    pub pair: &'a [String],
    /// Translate exactly what was typed, skipping corrections.
    pub as_typed: bool,
}

pub struct Translator {
    client: reqwest::Client,
    cooldown: Mutex<Cooldown>,
}

#[derive(Default)]
struct Cooldown {
    until: Option<Instant>,
    length: Duration,
}

impl Translator {
    pub fn new() -> Self {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0")
            .connect_timeout(Duration::from_secs(4))
            .timeout(Duration::from_secs(10))
            .pool_idle_timeout(Duration::from_secs(300))
            .build()
            .expect("http client");
        Self { client, cooldown: Mutex::default() }
    }

    pub async fn translate(&self, req: Request<'_>) -> Result<Translation, Error> {
        let Request { text, from, to, pair, as_typed } = req;

        // Romanized input with the source set explicitly, e.g. romaji with Japanese.
        if !as_typed && is_romanized(text, from) {
            if let Some(native) = self.convert_script(text, from).await {
                return self.translate_corrected(native, from, to, CorrectionKind::Script).await;
            }
        }

        let first = self.gtx(text, from, to).await?;
        if as_typed {
            return Ok(first);
        }
        let detected = first.detected.as_deref().unwrap_or_default();
        let in_pair = |lang: &str| pair.iter().any(|l| l == lang);

        // Google recognised romanized Japanese/Chinese but converts it badly. Short
        // romaji often isn't recognised at all ("arigatou" reads as Indonesian), so
        // also try when Japanese is one of the user's languages and the detected
        // language isn't.
        let script_lang = match &first.correction {
            Some(c) if c.kind == CorrectionKind::Script && romaji::input_method(detected).is_some() => {
                Some(detected)
            }
            _ if from == "auto" && in_pair("ja") && !in_pair(detected) && romaji::looks_like_romaji(text) => {
                Some("ja")
            }
            _ => None,
        };
        if let Some(lang) = script_lang {
            if let Some(native) = self.convert_script(text, lang).await {
                return self.translate_corrected(native, lang, to, CorrectionKind::Script).await;
            }
        }

        // Google noticed a typo but echoed the input back untranslated.
        if let Some(c) = &first.correction {
            if c.kind == CorrectionKind::Spelling && first.text.trim().eq_ignore_ascii_case(text.trim()) {
                let corrected = c.text.clone();
                return self.translate_corrected(corrected, from, to, CorrectionKind::Spelling).await;
            }
        }
        Ok(first)
    }

    async fn translate_corrected(
        &self,
        text: String,
        from: &str,
        to: &str,
        kind: CorrectionKind,
    ) -> Result<Translation, Error> {
        let mut translation = self.gtx(&text, from, to).await?;
        translation.correction = Some(Correction { text, kind });
        Ok(translation)
    }

    async fn gtx(&self, text: &str, from: &str, to: &str) -> Result<Translation, Error> {
        self.check_cooldown()?;
        let resp = self
            .client
            .post(ENDPOINT)
            .query(&[
                ("client", "gtx"),
                ("sl", from),
                ("tl", to),
                ("hl", "en"), // language of the part-of-speech labels, matching the UI
                ("dj", "1"),
                ("dt", "t"),
                ("dt", "bd"),
                ("dt", "rm"),
                ("dt", "qca"), // spelling and script corrections
            ])
            .form(&[("q", text)])
            .send()
            .await
            .map_err(network_error)?;

        let status = resp.status();
        if status.as_u16() == 429 {
            let retry_after = resp
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()?.parse().ok())
                .map(Duration::from_secs);
            return Err(self.rate_limited(retry_after));
        }
        if !status.is_success() {
            return Err(format!("Translation failed: HTTP {status}").into());
        }
        let body = resp.text().await.map_err(|e| e.to_string())?;
        let json: Value = serde_json::from_str(&body).map_err(|e| format!("Bad response: {e}"))?;
        self.cooldown.lock().unwrap().length = Duration::ZERO;
        Ok(parse(&json, text))
    }

    /// Writes romanized `text` in `lang`'s own script with Google's input method.
    async fn convert_script(&self, text: &str, lang: &str) -> Option<String> {
        let method = romaji::input_method(lang)?;
        let input = romaji::prepare(text, lang);
        let body = self
            .client
            .get(INPUT_TOOLS)
            .query(&[("text", input.as_str()), ("itc", method), ("num", "1"), ("cp", "0"), ("cs", "1"), ("ie", "utf-8"), ("oe", "utf-8")])
            .send()
            .await
            .ok()?
            .error_for_status()
            .ok()?
            .text()
            .await
            .ok()?;
        parse_input_method(&serde_json::from_str(&body).ok()?)
    }

    fn check_cooldown(&self) -> Result<(), Error> {
        match self.cooldown.lock().unwrap().until {
            Some(until) if until > Instant::now() => Err(rate_limit_error(until - Instant::now())),
            _ => Ok(()),
        }
    }

    fn rate_limited(&self, retry_after: Option<Duration>) -> Error {
        let mut cooldown = self.cooldown.lock().unwrap();
        let now = Instant::now();
        // Several requests in flight can all get the 429; count it once.
        if let Some(until) = cooldown.until.filter(|&u| u > now) {
            return rate_limit_error(until - now);
        }
        cooldown.length = (cooldown.length * 2).clamp(COOLDOWN_MIN, COOLDOWN_MAX);
        let wait = retry_after.map_or(cooldown.length, |r| r.min(RETRY_AFTER_MAX).max(cooldown.length));
        cooldown.until = Some(now + wait);
        rate_limit_error(wait)
    }
}

fn rate_limit_error(wait: Duration) -> Error {
    Error {
        message: "Google is limiting requests from this connection".into(),
        retry_in_ms: Some(wait.as_millis() as u64),
    }
}

/// Latin-letter input for a language we can convert to its own script.
fn is_romanized(text: &str, lang: &str) -> bool {
    romaji::input_method(lang).is_some()
        && text.is_ascii()
        && text.bytes().any(|b| b.is_ascii_alphabetic())
        && (lang != "ja" || romaji::looks_like_romaji(text))
}

/// reqwest's top-level message hides the cause (DNS, TLS, timeout…), so walk the chain.
fn network_error(e: reqwest::Error) -> Error {
    if e.is_timeout() {
        return "Network error: timed out".to_owned().into();
    }
    let mut cause: &dyn std::error::Error = &e;
    while let Some(source) = cause.source() {
        cause = source;
    }
    format!("Network error: {cause}").into()
}

fn non_empty(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

/// Parses a `gtx` response for the request text `input`.
pub fn parse(json: &Value, input: &str) -> Translation {
    let mut out = Translation::default();
    for s in json["sentences"].as_array().into_iter().flatten() {
        if let Some(t) = s["trans"].as_str() {
            out.text.push_str(t);
        }
        out.translit = out.translit.or_else(|| non_empty(&s["translit"]));
        out.src_translit = out.src_translit.or_else(|| non_empty(&s["src_translit"]));
    }
    out.detected = non_empty(&json["src"]);
    out.dict = json["dict"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|d| {
            Some(DictEntry {
                pos: d["pos"].as_str().unwrap_or_default().to_owned(),
                terms: d["terms"]
                    .as_array()?
                    .iter()
                    .filter_map(|t| t.as_str().map(str::to_owned))
                    .collect(),
            })
        })
        .collect();
    out.correction = non_empty(&json["spell"]["spell_res"])
        .filter(|s| s.trim() != input.trim())
        .map(|text| {
            let script = json["spell"]["correction_type"]
                .as_array()
                .is_some_and(|types| types.iter().any(|t| t.as_i64() == Some(6)));
            let kind = if script { CorrectionKind::Script } else { CorrectionKind::Spelling };
            Correction { text, kind }
        });
    out
}

/// Joins the top candidate of every segment of an input-method response.
fn parse_input_method(json: &Value) -> Option<String> {
    if json[0] != "SUCCESS" {
        return None;
    }
    let out: String = json[1].as_array()?.iter().filter_map(|seg| seg[1][0].as_str()).collect();
    (!out.is_empty()).then_some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn json(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn parses_word_with_dictionary() {
        let t = parse(
            &json(
                r#"{"sentences":[{"trans":"家","orig":"house","backend":10},{"translit":"Ie"}],
                    "dict":[{"pos":"noun","terms":["家","家屋"],"entry":[]}],"src":"en","spell":{}}"#,
            ),
            "house",
        );
        assert_eq!(t.text, "家");
        assert_eq!(t.translit.as_deref(), Some("Ie"));
        assert_eq!(t.detected.as_deref(), Some("en"));
        assert_eq!(t.dict[0].pos, "noun");
        assert_eq!(t.dict[0].terms, ["家", "家屋"]);
        assert!(t.correction.is_none());
    }

    #[test]
    fn joins_multiple_sentences() {
        let t = parse(
            &json(r#"{"sentences":[{"trans":"Hallo. "},{"trans":"Wie geht's?"},{"translit":""}],"src":"en"}"#),
            "Hello. How are you?",
        );
        assert_eq!(t.text, "Hallo. Wie geht's?");
        assert_eq!(t.translit, None);
        assert!(t.dict.is_empty());
    }

    #[test]
    fn parses_corrections() {
        let spelling = parse(
            &json(r#"{"sentences":[{"trans":"he runs fast"}],"src":"de",
                "spell":{"spell_res":"er läuft schnell","correction_type":[1]}}"#),
            "er lauft schnell",
        );
        let c = spelling.correction.unwrap();
        assert_eq!((c.text.as_str(), c.kind), ("er läuft schnell", CorrectionKind::Spelling));

        let script = parse(
            &json(r#"{"sentences":[{"trans":"eat"}],"src":"ja","spell":{"spell_res":"食べる","correction_type":[6]}}"#),
            "taberu",
        );
        assert_eq!(script.correction.unwrap().kind, CorrectionKind::Script);
    }

    #[test]
    fn parses_input_method_segments() {
        let out = parse_input_method(&json(
            r#"["SUCCESS",[["nihao",["你好"],[],{}],["woshixuesheng",["我是学生"],[],{}]]]"#,
        ));
        assert_eq!(out.as_deref(), Some("你好我是学生"));
        assert_eq!(parse_input_method(&json(r#""INVALID_INPUT_METHOD_NAME""#)), None);
    }

    #[test]
    fn rate_limit_cooldown_doubles_and_blocks() {
        let t = Translator::new();
        assert_eq!(t.rate_limited(None).retry_in_ms, Some(30_000));
        // While cooling down, requests fail without touching the network...
        assert!(t.check_cooldown().is_err());
        // ...and a second 429 from the same burst doesn't double the wait.
        assert!(t.rate_limited(None).retry_in_ms.unwrap() <= 30_000);
        t.cooldown.lock().unwrap().until = None;
        assert_eq!(t.rate_limited(None).retry_in_ms, Some(60_000));
    }

    fn request<'a>(text: &'a str, to: &'a str, pair: &'a [String]) -> Request<'a> {
        Request { text, from: "auto", to, pair, as_typed: false }
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn live_translation() {
        let t = Translator::new().translate(request("house", "es", &[])).await.unwrap();
        assert_eq!(t.text.to_lowercase(), "casa", "{t:?}");
        assert_eq!(t.detected.as_deref(), Some("en"));
        assert!(!t.dict.is_empty());
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn live_romaji() {
        let pair = ["en".to_owned(), "ja".to_owned()];
        let t = Translator::new().translate(request("kore wa nan desu ka", "en", &pair)).await.unwrap();
        let c = t.correction.as_ref().expect("converted");
        assert_eq!((c.text.as_str(), c.kind), ("これはなんですか", CorrectionKind::Script), "{t:?}");
        assert!(t.text.to_lowercase().contains("what"), "{t:?}");
    }
}
