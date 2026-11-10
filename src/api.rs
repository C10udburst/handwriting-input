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
