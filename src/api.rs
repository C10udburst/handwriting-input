//! Google handwriting API client (single language + multi-language strategies).
//!
//! Response shape: a JSON array — a length-1 array is an error payload
//! (`[message]`); otherwise candidates are at `response[1][0][1]`.

use std::sync::mpsc::Sender;
use std::thread;
use std::time::Duration;

use serde_json::{json, Value};

use crate::config::Config;
use crate::ink::{build_request, Ink, RequestContext};

/// One recognition request outcome, tagged with the language spec we sent.
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

/// Parse a Google IME handwriting response body.
pub fn parse_response(body: &str) -> Result<Vec<String>, String> {
    let v: Value = serde_json::from_str(body).map_err(|e| format!("server sent invalid JSON: {e}"))?;
    let arr = v.as_array().ok_or("server sent an unexpected payload")?;
    if arr.len() == 1 {
        let msg = arr[0].as_str().unwrap_or("unknown recognition error").to_string();
        return Err(msg);
    }
    let list = arr
        .get(1)
        .and_then(|x| x.get(0))
        .and_then(|x| x.get(1))
        .and_then(|x| x.as_array())
        .ok_or("unexpected response format")?;
    Ok(list
        .iter()
        .filter_map(|s| s.as_str().map(str::to_string))
        .filter(|s| !s.is_empty())
        .collect())
}

/// Blocking single request.
pub fn request_blocking(
    cfg: &Config,
    ink: &Ink,
    width: u32,
    height: u32,
    ctx: &RequestContext,
) -> Result<Vec<String>, String> {
    let body = build_request(ink, width, height, ctx).to_string();
    let timeout = Duration::from_secs(cfg.timeout_secs.max(1));
    let resp = ureq::post(&cfg.server_url)
        .header("Content-Type", "application/json")
        .config()
        .timeout_per_call(Some(timeout))
        .send(body)
        .map_err(|e| format!("recognition request failed: {e}"))?;
    let status = resp.status().as_u16();
    let text = resp
        .body_mut()
        .read_to_string()
        .map_err(|e| format!("failed reading response: {e}"))?;
    match status {
        200 => parse_response(&text),
        403 => Err("access denied (403)".into()),
        503 => Err("recognition server unavailable (503)".into()),
        other => Err(format!("recognition server error ({other})")),
    }
}

/// Spawn one worker thread per language spec; results arrive on `tx`.
pub fn spawn_requests(
    cfg: Config,
    ink: Ink,
    width: u32,
    height: u32,
    specs: Vec<(String, RequestContext)>,
    gen: u64,
    tx: Sender<Outcome>,
) {
    for (tag, ctx) in specs {
        let cfg = cfg.clone();
        let ink = ink.clone();
        let tx = tx.clone();
        thread::spawn(move || {
            let result = request_blocking(&cfg, &ink, width, height, &ctx);
            let _ = tx.send(Outcome { gen, lang: tag, result });
            // Wake the Wayland poll so drain_results can pick it up.
            let _ = egui::Context::try_from(&cfg);
        });
    }
}

/// Language specs to send for one recognition round.
pub fn request_specs(
    cfg: &Config,
    probe: Option<bool>,
    sequence_id: u32,
    pre_context: &str,
) -> Vec<(String, RequestContext)> {
    let base_ctx = RequestContext {
        starting_language: cfg.langs[0].clone(),
        pre_edit_string: if cfg.use_context { pre_context.to_string() } else { String::new() },
        sequence_id,
        use_context: cfg.use_context,
    };

    if cfg.langs.len() == 1 {
        let l = cfg.langs[0].clone();
        return vec![(l.clone(), base_ctx)];
    }

    let per_lang = || {
        cfg.langs
            .iter()
            .map(|l| {
                (
                    l.clone(),
                    RequestContext {
                        starting_language: cfg.langs[0].clone(),
                        pre_edit_string: if cfg.use_context { pre_context.to_string() } else { String::new() },
                        sequence_id,
                        use_context: cfg.use_context,
                    },
                )
            })
            .collect()
    };

    match (cfg.multi_lang, probe) {
        (MultiLang::Parallel, _) | (_, Some(false)) => per_lang(),
        (_, Some(true)) | (_, None) => {
            vec![("multi".to_string(), base_ctx)]
        }
    }
}

/// Round-robin interleave of per-language candidate lists with dedup.
pub fn merge(outcomes: &[Outcome], per_lang_cap: usize) -> Vec<Candidate> {
    let mut lists: Vec<(String, Vec<String>)> = outcomes
        .iter()
        .filter_map(|o| o.result.as_ref().ok().cloned().map(|v| (o.lang.clone(), v)))
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
            if text.chars().count() > per_lang_cap.saturating_mul(8) {
                continue;
            }
            if !seen.contains(&text) {
                seen.push(text.clone());
                out.push(Candidate { text, lang });
            }
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    out.truncate(per_lang_cap.saturating_mul(outcomes.len().max(1)));
    out
}