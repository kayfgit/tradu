//! Microsoft Azure Translator (v3) with the user's own key: the fallback while
//! Google's free endpoint is rate-limiting, or the only translator if configured.

use serde_json::{json, Value};

use crate::translate::{network_error, non_empty, DictEntry, Error, Translation};

const ENDPOINT: &str = "https://api.cognitive.microsofttranslator.com";

#[derive(Debug, Clone)]
pub struct Azure {
    pub key: String,
    /// The resource's region ("westeurope"); empty for a global resource.
    pub region: String,
    /// Use Azure for everything, not only while Google is refusing.
    pub always: bool,
}

/// Google's language codes (used throughout tradu) → Azure's.
pub fn to_azure(code: &str) -> &str {
    match code {
        "zh-CN" => "zh-Hans",
        "zh-TW" => "zh-Hant",
        "iw" => "he",
        "jw" => "jv",
        "tl" => "fil",
        "no" => "nb",
        "pt-PT" => "pt-pt",
        "fr-CA" => "fr-ca",
        "mni-Mtei" => "mni",
        "ku" => "kmr",
        "ckb" => "ku",
        "sr" => "sr-Cyrl",
        "mn" => "mn-Cyrl",
        "hmn" => "mww",
        other => other,
    }
}

/// Azure's language codes → Google's.
pub fn from_azure(code: &str) -> String {
    match code {
        "zh-Hans" => "zh-CN",
        "zh-Hant" => "zh-TW",
        "he" => "iw",
        "fil" => "tl",
        "nb" => "no",
        "pt-pt" | "pt-PT" => "pt-PT",
        "fr-ca" | "fr-CA" => "fr-CA",
        "mni" => "mni-Mtei",
        "kmr" => "ku",
        "ku" => "ckb",
        "sr-Cyrl" | "sr-Latn" => "sr",
        "mn-Cyrl" => "mn",
        "mww" => "hmn",
        other => other,
    }
    .to_owned()
}

/// Targets Azure can romanize ("Ie" for 家). Others are asked without it.
const ROMANIZED: &[&str] = &[
    "ja", "zh-Hans", "zh-Hant", "ko", "ru", "uk", "bg", "sr-Cyrl", "el", "ar", "fa", "he", "hi",
    "bn", "ta", "te", "th", "ur", "mr", "gu", "pa", "kn", "ml", "ne",
];

pub async fn translate(
    client: &reqwest::Client,
    azure: &Azure,
    text: &str,
    from: &str,
    to: &str,
) -> Result<Translation, Error> {
    let target = to_azure(to);
    let mut query = vec![("api-version", "3.0"), ("to", target)];
    if from != "auto" {
        query.push(("from", to_azure(from)));
    }
    let json = if ROMANIZED.contains(&target) {
        let mut romanized = query.clone();
        romanized.push(("toScript", "Latn"));
        // Not every language Azure translates into can be romanized; retry plainly.
        match post(client, azure, "/translate", &romanized, text).await {
            Err(e) if e.message.contains("HTTP 400") => post(client, azure, "/translate", &query, text).await?,
            other => other?,
        }
    } else {
        post(client, azure, "/translate", &query, text).await?
    };
    let mut translation = parse(&json);
    if from != "auto" {
        translation.detected.get_or_insert_with(|| from.to_owned());
    }

    // Single words get dictionary alternatives. Azure's dictionary pairs every
    // language with English only.
    let source = translation.detected.clone().unwrap_or_default();
    let word = !text.trim().is_empty() && !text.trim().contains(char::is_whitespace);
    if word && source != to && (source == "en" || to == "en") {
        let query = [("api-version", "3.0"), ("from", to_azure(&source)), ("to", target)];
        if let Ok(json) = post(client, azure, "/dictionary/lookup", &query, text.trim()).await {
            translation.dict = parse_lookup(&json);
        }
    }
    Ok(translation)
}

/// A minimal request, to tell whether a key and region work.
pub async fn check(client: &reqwest::Client, azure: &Azure) -> Result<(), Error> {
    translate(client, azure, "ok", "en", "es").await.map(|_| ())
}

async fn post(
    client: &reqwest::Client,
    azure: &Azure,
    path: &str,
    query: &[(&str, &str)],
    text: &str,
) -> Result<Value, Error> {
    let mut request = client
        .post(format!("{ENDPOINT}{path}"))
        .query(query)
        .header("Ocp-Apim-Subscription-Key", &azure.key)
        .header(reqwest::header::CONTENT_TYPE, "application/json")
        .body(json!([{ "Text": text }]).to_string());
    if !azure.region.is_empty() {
        request = request.header("Ocp-Apim-Subscription-Region", &azure.region);
    }
    let resp = request.send().await.map_err(network_error)?;
    let status = resp.status();
    let body = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(error(status.as_u16(), &body));
    }
    serde_json::from_str(&body).map_err(|e| format!("Bad response from Azure: {e}").into())
}

fn error(status: u16, body: &str) -> Error {
    let detail = serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|v| v["error"]["message"].as_str().map(str::to_owned))
        .unwrap_or_default();
    match status {
        401 => "Azure rejected the key or region. Set them again with :azure KEY REGION".to_owned(),
        403 => format!("Azure quota used up for now: {detail}"),
        429 => "Azure is limiting requests too. Wait a minute".to_owned(),
        _ => format!("Azure: {detail} (HTTP {status})"),
    }
    .into()
}

pub fn parse(json: &Value) -> Translation {
    let item = &json[0];
    let translation = &item["translations"][0];
    Translation {
        text: translation["text"].as_str().unwrap_or_default().to_owned(),
        detected: item["detectedLanguage"]["language"].as_str().map(from_azure),
        translit: non_empty(&translation["transliteration"]["text"]),
        provider: "azure",
        ..Default::default()
    }
}

/// Groups dictionary translations by part of speech, most confident first.
fn parse_lookup(json: &Value) -> Vec<DictEntry> {
    let mut entries: Vec<DictEntry> = Vec::new();
    for t in json[0]["translations"].as_array().into_iter().flatten() {
        let (Some(term), Some(pos)) = (t["displayTarget"].as_str(), t["posTag"].as_str()) else { continue };
        let pos = pos.to_lowercase();
        match entries.iter_mut().find(|e| e.pos == pos) {
            Some(entry) => entry.terms.push(term.to_owned()),
            None => entries.push(DictEntry { pos, terms: vec![term.to_owned()], base: None }),
        }
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_language_codes_both_ways() {
        for google in ["zh-CN", "zh-TW", "iw", "tl", "no", "pt-PT", "ku", "ckb", "ja", "de"] {
            assert_eq!(from_azure(to_azure(google)), google);
        }
    }

    #[test]
    fn parses_translation_with_romanization() {
        let t = parse(&serde_json::from_str(
            r#"[{"detectedLanguage":{"language":"en","score":1.0},
                "translations":[{"text":"家","to":"ja","transliteration":{"text":"ie","script":"Latn"}}]}]"#,
        ).unwrap());
        assert_eq!(t.text, "家");
        assert_eq!(t.detected.as_deref(), Some("en"));
        assert_eq!(t.translit.as_deref(), Some("ie"));
        assert_eq!(t.provider, "azure");
    }

    #[test]
    fn parses_detected_chinese_as_googles_code() {
        let t = parse(&serde_json::from_str(
            r#"[{"detectedLanguage":{"language":"zh-Hans","score":1.0},"translations":[{"text":"hello","to":"en"}]}]"#,
        ).unwrap());
        assert_eq!(t.detected.as_deref(), Some("zh-CN"));
    }

    #[test]
    fn groups_dictionary_by_part_of_speech() {
        let dict = parse_lookup(&serde_json::from_str(
            r#"[{"normalizedSource":"house","displaySource":"house","translations":[
                {"displayTarget":"casa","posTag":"NOUN","confidence":0.8},
                {"displayTarget":"vivienda","posTag":"NOUN","confidence":0.1},
                {"displayTarget":"albergar","posTag":"VERB","confidence":0.1}]}]"#,
        ).unwrap());
        assert_eq!(dict.len(), 2);
        assert_eq!((dict[0].pos.as_str(), dict[0].terms.as_slice()), ("noun", ["casa".to_owned(), "vivienda".to_owned()].as_slice()));
        assert_eq!(dict[1].pos, "verb");
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn live_rejects_a_bad_key() {
        let azure = Azure { key: "not-a-real-key".into(), region: "westeurope".into(), always: true };
        let err = check(&reqwest::Client::new(), &azure).await.unwrap_err();
        assert!(err.message.contains("rejected the key"), "{err:?}");
    }
}
