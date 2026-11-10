//! Stroke model and serialization to the Google IME `ink` JSON format.
//!
//! The wire format (verified against the original app and Chrome OS' own
//! handwriting IME): every stroke is `[ [x0, x1, ...], [y0, y1, ...], [] ]` in
//! canvas pixels, wrapped in
//! `{"options":"enable_pre_space","requests":[{...}]}`.

use serde_json::{json, Value};

#[derive(Clone, Default, Debug)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
}

impl Stroke {
    /// Add a point, dropping duplicates that would only bloat the request.
    pub fn push(&mut self, x: f32, y: f32) {
        if let Some([px, py]) = self.points.last() {
            if (*px - x).abs() < 0.5 && (*py - y).abs() < 0.5 {
                return;
            }
        }
        self.points.push([x, y]);
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn to_json(&self) -> Value {
        let xs: Vec<Value> = self.points.iter().map(|p| json!((p[0] as f64).round())).collect();
        let ys: Vec<Value> = self.points.iter().map(|p| json!((p[1] as f64).round())).collect();
        json!([xs, ys, []])
    }
}

#[derive(Clone, Default, Debug)]
pub struct Ink {
    pub strokes: Vec<Stroke>,
}

impl Ink {
    pub fn clear(&mut self) {
        self.strokes.clear();
    }

    pub fn is_empty(&self) -> bool {
        self.strokes.is_empty()
    }

    pub fn to_json(&self) -> Value {
        Value::Array(self.strokes.iter().map(|s| s.to_json()).collect())
    }
}

/// Everything that varies between recognition rounds.
#[derive(Clone, Debug)]
pub struct RequestContext {
    /// `"pl"` for one language, `["pl","en",...]` for a combined request.
    pub language: Value,
    /// Text already in the field before the caret (the `pre_context` field).
    pub pre_context: String,
    /// Ask the server for at most this many candidates (0 = server default).
    pub max_completions: usize,
    /// Also send the richer Chrome-OS-style context keys.
    pub use_context: bool,
    pub sequence_id: u32,
    pub starting_language: String,
}

impl RequestContext {
    pub fn new(language: Value, pre_context: String, max_completions: usize, use_context: bool) -> Self {
        let starting_language = language
            .as_str()
            .map(str::to_string)
            .or_else(|| language.get(0).and_then(Value::as_str).map(str::to_string))
            .unwrap_or_else(|| "en".to_string());
        Self {
            language,
            pre_context,
            max_completions,
            use_context,
            sequence_id: 0,
            starting_language,
        }
    }
}

/// Build one request body for `ink` drawn into a `width`x`height` canvas.
pub fn build_request(ink: &Ink, width: u32, height: u32, ctx: &RequestContext) -> Value {
    let mut request = json!({
        "writing_guide": {
            "writing_area_width": width,
            "writing_area_height": height
        },
        "ink": ink.to_json(),
        "language": ctx.language,
    });
    if !ctx.pre_context.is_empty() {
        request["pre_context"] = json!(ctx.pre_context);
        request["pre_edit_string"] = json!(ctx.pre_context);
    }
    if ctx.max_completions > 0 {
        request["max_completions"] = json!(ctx.max_completions);
    }
    if ctx.use_context {
        request["use_pre_space"] = json!(false);
        request["use_bigram_model"] = json!(true);
        request["incoming_output"] = json!("");
        request["commit_edit"] = json!(false);
        request["sequence_id"] = json!(ctx.sequence_id);
        request["starting_language"] = json!(ctx.starting_language);
        request["use_vertical_writing"] = json!(false);
    }
    json!({ "options": "enable_pre_space", "requests": [request] })
}
