//! Google handwriting API client: one request per language strategy.
//!
//! Response shape: a JSON array. A length-1 array is an error payload
//! (`[message]`); otherwise the candidate list lives at `response[1][0][1]`.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::{Config, MultiLang};
use crate::ink::{build_request, Ink, RequestContext};

/// One recognition response, tagged with the language it was requested for.
#[derive(Debug, Clone)]
pub struct Outcome {
    pub gen: u64,
    pub lang: String,
    pub result: Result<Vec<String>, String>,
}

#[derive(Debug, Clone)]
pub struct Candidate {
    pub text: String,
    pub lang: String,
}

/// Tag used for a single combined multi-language request.
pub const MULTI: &str = "multi";

/// Parse a Google IME handwriting response body.
pub fn parse_response(body: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(body).map_err(|e| format!("invalid JSON: {e}"))?;
    let array = value.as_array().ok_or("unexpected payload")?;
    if array.len() == 1 {
        return Err(array[0]
            .as_str()
            .unwrap_or("unknown recognition error")
            .to_string());
    }
    let list = array
        .get(1)
        .and_then(|entry| entry.get(0))
        .and_then(|entry| entry.get(1))
        .and_then(|list| list.as_array())
        .ok_or("unexpected response shape")?;
    Ok(list
        .iter()
        .filter_map(|candidate| candidate.as_str())
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect())
}

/// Blocking POST. Runs on a recognition worker thread.
pub fn request_blocking(
    cfg: &Config,
    ink: &Ink,
    width: u32,
    height: u32,
    ctx: &RequestContext,
) -> Result<Vec<String>, String> {
    let body = build_request(ink, width, height, ctx).to_string();
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .timeout_per_call(Some(Duration::from_secs(cfg.timeout_secs.max(1))))
        .build()
        .into();
    let response = agent
        .post(&cfg.server_url)
        .header("Content-Type", "application/json")
        .send(body)
        .map_err(|e| format!("request failed: {e}"))?;

    let status = response.status().as_u16();
    let text = response
        .into_body()
        .read_to_string()
        .map_err(|e| format!("reading response: {e}"))?;
    match status {
        200 => parse_response(&text),
        403 => Err("access denied (403)".into()),
        503 => Err("recognition service unavailable (503)".into()),
        other => Err(format!("server error ({other})")),
    }
}

/// One worker thread per language spec; results land on `tx` and wake the loop.
pub fn spawn_requests(
    cfg: Config,
    ink: Ink,
    width: u32,
    height: u32,
    specs: Vec<(String, RequestContext)>,
    gen: u64,
    tx: Sender<Outcome>,
    ctx: egui::Context,
) {
    for (lang, request_ctx) in specs {
        let cfg = cfg.clone();
        let ink = ink.clone();
        let tx = tx.clone();
        let ctx = ctx.clone();
        thread::spawn(move || {
            let result = request_blocking(&cfg, &ink, width, height, &request_ctx);
            let _ = tx.send(Outcome { gen, lang, result });
            // Recognition finished: wake the Wayland poll so it gets drained.
            ctx.request_repaint();
        });
    }
}

/// Language specs for one recognition round.
///
/// `probe` is `None` until we learn whether a single combined request works;
/// `Some(false)` means we must ask per language.
pub fn request_specs(
    cfg: &Config,
    probe: Option<bool>,
    sequence_id: u32,
    pre_context: &str,
) -> Vec<(String, RequestContext)> {
    let make = |language: Value, tag: String| {
        let mut ctx = RequestContext::new(
            language,
            if cfg.use_context { pre_context.to_string() } else { String::new() },
            cfg.num_candidates,
            cfg.use_context,
        );
        ctx.sequence_id = sequence_id;
        (tag, ctx)
    };

    if cfg.langs.len() == 1 {
        let lang = cfg.langs[0].clone();
        return vec![make(json!(lang.clone()), lang)];
    }

    let per_language = || {
        cfg.langs
            .iter()
            .map(|l| make(json!(l.clone()), l.clone()))
            .collect()
    };

    match (cfg.multi_lang, probe) {
        (MultiLang::Parallel, _) | (_, Some(false)) => per_language(),
        (_, _) => vec![make(json!(cfg.langs.clone()), MULTI.to_string())],
    }
}

/// Round-robin interleave of the per-language candidate lists, deduplicated.
pub fn merge(outcomes: &[Outcome], cap: usize) -> Vec<Candidate> {
    let mut lists: Vec<(String, Vec<String>)> = outcomes
        .iter()
        .filter_map(|o| {
            o.result
                .as_ref()
                .ok()
                .map(|candidates| (o.lang.clone(), candidates.clone()))
        })
        .collect();

    let mut out: Vec<Candidate> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    loop {
        let mut progressed = false;
        for (lang, list) in lists.iter_mut() {
            if list.is_empty() {
                continue;
            }
            let text = list.remove(0);
            progressed = true;
            if !seen.contains(&text) {
                seen.push(text.clone());
                out.push(Candidate {
                    text,
                    lang: lang.clone(),
                });
            }
        }
        if !progressed {
            break;
        }
    }
    out.truncate(cap);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    fn outcome(lang: &str, candidates: &[&str]) -> Outcome {
        Outcome {
            gen: 1,
            lang: lang.to_string(),
            result: Ok(candidates.iter().map(|s| s.to_string()).collect()),
        }
    }

    #[test]
    fn parses_the_nested_candidate_list() {
        let body = r#"[[1,2,3],[[null,["hello","hell","hallo"]]]]"#;
        assert_eq!(parse_response(body).unwrap(), vec!["hello", "hell", "hallo"]);
    }

    #[test]
    fn empty_candidates_are_dropped() {
        let body = r#"[[1],[[null,["ok","","  ","nope"]]]]"#;
        assert_eq!(parse_response(body).unwrap(), vec!["ok", "  ", "nope"]);
    }

    #[test]
    fn single_element_payload_is_reported_as_an_error() {
        let body = r#"["no candidates found"]"#;
        assert_eq!(parse_response(body).unwrap_err(), "no candidates found");
    }

    #[test]
    fn malformed_payloads_fail_loudly() {
        assert!(parse_response("not json").is_err());
        assert!(parse_response("[[1],[[null]]]").is_err());
        assert!(parse_response("{}").is_err());
    }

    #[test]
    fn one_language_means_one_request_tagged_with_it() {
        let mut cfg = Config::default();
        cfg.langs = vec!["pl".to_string()];
        let specs = request_specs(&cfg, None, 7, "tekst ");
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].0, "pl");
        assert_eq!(specs[0].1.language, json!("pl"));
        assert_eq!(specs[0].1.pre_context, "tekst ");
        assert_eq!(specs[0].1.sequence_id, 7);
    }

    #[test]
    fn multi_language_starts_combined_and_falls_back_per_language() {
        let mut cfg = Config::default();
        cfg.langs = vec!["pl".to_string(), "en".to_string()];

        let combined = request_specs(&cfg, None, 0, "");
        assert_eq!(combined.len(), 1, "first round should probe with one request");
        assert_eq!(combined[0].0, MULTI);
        assert_eq!(combined[0].1.language, json!(["pl", "en"]));

        let split = request_specs(&cfg, Some(false), 0, "");
        assert_eq!(split.len(), 2);
        assert_eq!(split[0].0, "pl");
        assert_eq!(split[1].0, "en");

        cfg.multi_lang = MultiLang::Parallel;
        assert_eq!(request_specs(&cfg, None, 0, "").len(), 2);
    }

    #[test]
    fn context_is_dropped_when_disabled() {
        let mut cfg = Config::default();
        cfg.langs = vec!["pl".to_string()];
        cfg.use_context = false;
        let specs = request_specs(&cfg, None, 0, "sekret");
        assert_eq!(specs[0].1.pre_context, "");
    }

    #[test]
    fn merge_interleaves_languages_dedupes_and_caps() {
        let merged = merge(
            &[outcome("pl", &["dom", "dam"]), outcome("en", &["dom", "done"])],
            8,
        );
        let texts: Vec<&str> = merged.iter().map(|c| c.text.as_str()).collect();
        assert_eq!(texts, vec!["dom", "dam", "done"], "round-robin, duplicate dropped");
        assert_eq!(merged[0].lang, "pl");

        let capped: Vec<String> = merge(
            &[outcome("pl", &["a", "b", "c"]), outcome("en", &["d", "e"])],
            3,
        )
        .iter()
        .map(|c| c.text.clone())
        .collect();
        assert_eq!(capped.len(), 3);
    }

    #[test]
    fn merge_ignores_failed_languages() {
        let failed = Outcome {
            gen: 1,
            lang: "de".to_string(),
            result: Err("timeout".to_string()),
        };
        let merged = merge(&[failed, outcome("en", &["hi"])], 8);
        assert_eq!(merged.len(), 1);
        assert_eq!(merged[0].lang, "en");
    }
}
