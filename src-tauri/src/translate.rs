//! Google Translate's keyless `gtx` endpoint: one request returns the translation,
//! romanization, detected language and (for single words) dictionary alternatives.

use serde::Serialize;
use serde_json::Value;

const ENDPOINT: &str = "https://translate.googleapis.com/translate_a/single";

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
}

#[derive(Debug, Serialize)]
pub struct DictEntry {
    pub pos: String,
    pub terms: Vec<String>,
}

pub fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .user_agent("Mozilla/5.0")
        .connect_timeout(std::time::Duration::from_secs(4))
        .timeout(std::time::Duration::from_secs(10))
        .pool_idle_timeout(std::time::Duration::from_secs(300))
        .build()
        .expect("http client")
}

/// Opens (or keeps alive) the TLS connection so the first translation after the
/// popup appears doesn't pay for the handshake.
pub async fn warm_up(client: &reqwest::Client) {
    let _ = client.head(ENDPOINT).send().await;
}

pub async fn google(
    client: &reqwest::Client,
    text: &str,
    from: &str,
    to: &str,
) -> Result<Translation, String> {
    let resp = client
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
        ])
        .form(&[("q", text)])
        .send()
        .await
        .map_err(network_error)?;

    let status = resp.status();
    if !status.is_success() {
        return Err(match status.as_u16() {
            429 => "Rate limited by Google, try again in a moment".into(),
            _ => format!("Translation failed: HTTP {status}"),
        });
    }
    let body = resp.text().await.map_err(|e| e.to_string())?;
    let json: Value = serde_json::from_str(&body).map_err(|e| format!("Bad response: {e}"))?;
    Ok(parse(&json))
}

/// reqwest's top-level message hides the cause (DNS, TLS, timeout…), so walk the chain.
fn network_error(e: reqwest::Error) -> String {
    if e.is_timeout() {
        return "Network error: timed out".into();
    }
    let mut cause: &dyn std::error::Error = &e;
    while let Some(source) = cause.source() {
        cause = source;
    }
    format!("Network error: {cause}")
}

fn non_empty(v: &Value) -> Option<String> {
    v.as_str().filter(|s| !s.is_empty()).map(str::to_owned)
}

pub fn parse(json: &Value) -> Translation {
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
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_word_with_dictionary() {
        let json: Value = serde_json::from_str(
            r#"{"sentences":[{"trans":"家","orig":"house","backend":10},{"translit":"Ie"}],
                "dict":[{"pos":"noun","terms":["家","家屋"],"entry":[]}],"src":"en"}"#,
        )
        .unwrap();
        let t = parse(&json);
        assert_eq!(t.text, "家");
        assert_eq!(t.translit.as_deref(), Some("Ie"));
        assert_eq!(t.detected.as_deref(), Some("en"));
        assert_eq!(t.dict[0].pos, "noun");
        assert_eq!(t.dict[0].terms, ["家", "家屋"]);
    }

    #[test]
    fn joins_multiple_sentences() {
        let json: Value = serde_json::from_str(
            r#"{"sentences":[{"trans":"Hallo. "},{"trans":"Wie geht's?"},{"translit":""}],"src":"en"}"#,
        )
        .unwrap();
        let t = parse(&json);
        assert_eq!(t.text, "Hallo. Wie geht's?");
        assert_eq!(t.translit, None);
        assert!(t.dict.is_empty());
    }

    #[tokio::test]
    #[ignore = "hits the network"]
    async fn live_translation() {
        let t = google(&client(), "house", "auto", "es").await.unwrap();
        assert_eq!(t.text.to_lowercase(), "casa", "{t:?}");
        assert_eq!(t.detected.as_deref(), Some("en"));
        assert!(!t.dict.is_empty());
    }
}
