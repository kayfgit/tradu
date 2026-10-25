//! DeepL API (Free or Pro) with the user's own key: the fallback while Google's
//! free endpoint is rate-limiting, or the only translator if configured.

use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde::Serialize;
use serde_json::{json, Value};

use crate::translate::{network_error, Error, Translation};

#[derive(Debug, Clone)]
pub struct Deepl {
    pub key: String,
}

impl Deepl {
    /// Free-plan keys end in ":fx" and have a host of their own.
    fn endpoint(&self) -> &'static str {
        if self.key.ends_with(":fx") { "https://api-free.deepl.com" } else { "https://api.deepl.com" }
    }

    fn auth(&self) -> String {
        format!("DeepL-Auth-Key {}", self.key)
    }
}

/// Google's language code (used throughout tradu) → DeepL's, as a source.
pub fn source_code(code: &str) -> String {
    match code {
        "zh-CN" | "zh-TW" => "zh",
        "pt-PT" => "pt",
        "iw" => "he",
        "no" => "nb",
        other => other,
    }
    .to_uppercase()
}

/// As a target, DeepL wants the variant for English, Portuguese and Chinese.
pub fn target_code(code: &str) -> String {
    match code {
        "en" => "en-us",
        "pt" => "pt-br",
        "pt-PT" => "pt-pt",
        "zh-CN" => "zh-hans",
        "zh-TW" => "zh-hant",
        "iw" => "he",
        "no" => "nb",
        other => other,
    }
    .to_uppercase()
}

/// DeepL's detected language → Google's code.
pub fn from_deepl(code: &str) -> String {
    let lower = code.to_ascii_lowercase();
    match lower.as_str() {
        "zh" => "zh-CN".to_owned(),
        "he" => "iw".to_owned(),
        "nb" => "no".to_owned(),
        _ => lower,
    }
}

pub async fn translate(
    client: &reqwest::Client,
    deepl: &Deepl,
    text: &str,
    from: &str,
    to: &str,
) -> Result<Translation, Error> {
    let mut body = json!({ "text": [text], "target_lang": target_code(to) });
    if from != "auto" {
        body["source_lang"] = json!(source_code(from));
    }
    let resp = client
        .post(format!("{}/v2/translate", deepl.endpoint()))
        .header(AUTHORIZATION, deepl.auth())
        .header(CONTENT_TYPE, "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(network_error)?;
    let mut translation = parse(&read(resp).await?);
    if translation.detected.is_none() && from != "auto" {
        translation.detected = Some(from.to_owned());
    }
    Ok(translation)
}

#[derive(Debug, Clone, Serialize)]
pub struct Usage {
    pub used: u64,
    pub limit: u64,
}

/// Checks a key without spending characters: this month's usage and quota.
pub async fn usage(client: &reqwest::Client, deepl: &Deepl) -> Result<Usage, Error> {
    let resp = client
        .get(format!("{}/v2/usage", deepl.endpoint()))
        .header(AUTHORIZATION, deepl.auth())
        .send()
        .await
        .map_err(network_error)?;
    let json = read(resp).await?;
    Ok(Usage {
        used: json["character_count"].as_u64().unwrap_or_default(),
        limit: json["character_limit"].as_u64().unwrap_or_default(),
    })
}

async fn read(resp: reqwest::Response) -> Result<Value, Error> {
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(error(status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|e| format!("Bad response from DeepL: {e}").into())
}

fn error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["message"].as_str().map(str::to_owned))
        .unwrap_or_default();
    match status {
        401 | 403 => "DeepL rejected the key. Set it again with :deepl KEY".to_owned(),
        456 => "DeepL's monthly character quota is used up".to_owned(),
        429 => "DeepL is limiting requests too. Wait a minute".to_owned(),
        400 if detail.contains("lang") => "DeepL doesn't support this language (it covers about 35)".to_owned(),
        _ => format!("DeepL: {detail} (HTTP {status})"),
    }
    .into()
}

pub fn parse(json: &Value) -> Translation {
    let translation = &json["translations"][0];
    Translation {
        text: translation["text"].as_str().unwrap_or_default().to_owned(),
        detected: translation["detected_source_language"].as_str().map(from_deepl),
        provider: "deepl",
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_language_codes() {
        assert_eq!(target_code("en"), "EN-US");
        assert_eq!(target_code("pt"), "PT-BR");
        assert_eq!(target_code("zh-TW"), "ZH-HANT");
        assert_eq!(target_code("ja"), "JA");
        assert_eq!(source_code("zh-CN"), "ZH");
        assert_eq!(source_code("pt-PT"), "PT");
        for google in ["de", "ja", "iw", "no", "zh-CN"] {
            assert_eq!(from_deepl(&source_code(google)), google);
        }
    }

    #[test]
    fn parses_translation() {
        let t = parse(&serde_json::from_str(
            r#"{"translations":[{"detected_source_language":"DE","text":"I'll send you the documents."}]}"#,
        ).unwrap());
        assert_eq!(t.text, "I'll send you the documents.");
        assert_eq!(t.detected.as_deref(), Some("de"));
        assert_eq!(t.provider, "deepl");
    }

    #[test]
    fn explains_errors() {
        assert!(error(403, "").message.contains("rejected the key"));
        assert!(error(456, "").message.contains("quota"));
        assert!(error(400, r#"{"message":"Value for 'target_lang' not supported."}"#).message.contains("doesn't support"));
    }

    #[test]
    fn picks_the_free_host_for_free_keys() {
        assert_eq!(Deepl { key: "abc:fx".into() }.endpoint(), "https://api-free.deepl.com");
        assert_eq!(Deepl { key: "abc".into() }.endpoint(), "https://api.deepl.com");
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn live_rejects_a_bad_key() {
        let deepl = Deepl { key: "00000000-0000-0000-0000-000000000000:fx".into() };
        let err = usage(&reqwest::Client::new(), &deepl).await.unwrap_err();
        assert!(err.message.contains("rejected the key"), "{err:?}");
    }
}
