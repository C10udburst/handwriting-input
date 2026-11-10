//! Stroke model and serialization to the Google IME `ink` JSON format.

use serde_json::{json, Value};

#[derive(Clone, Default, Debug)]
pub struct Stroke {
    pub points: Vec<[f32; 2]>,
}

impl Stroke {
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

/// Parameters for one request round.
#[derive(Clone, Debug)]
pub struct RequestContext {
    pub language: serde_json::Value,
    pub starting_language: String,
    pub pre_edit_string: String,
    pub sequence_id: u32,
    pub use_context: bool,
}

/// Build the request body. Extra Chrome-OS-style context keys are sent when
/// `use_context` is set; Google's endpoint ignores fields it does not know.
pub fn build_request(ink: &Ink, width: u32, height: u32, ctx: &RequestContext) -> Value {
    let request = if ctx.use_context {
        json!({
            "writing_guide": {
                "writing_area_width": width,
                "writing_area_height": height
            },
            "ink": ink.to_json(),
            "language": ctx.language,
            "use_pre_space": false,
            "use_bigram_model": true,
            "pre_edit_string": ctx.pre_edit_string,
            "incoming_output": "",
            "commit_edit": false,
            "sequence_id": ctx.sequence_id,
            "starting_language": ctx.starting_language,
            "use_vertical_writing": false
        })
    } else {
        json!({
            "writing_guide": {
                "writing_area_width": width,
                "writing_area_height": height
            },
            "ink": ink.to_json(),
            "language": ctx.language
        })
    };
    json!({
        "options": "enable_pre_space",
        "requests": [request]
    })
}
