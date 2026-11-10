//! Recognition rounds: build specs, spawn workers, merge what comes back.

use super::App;
use crate::api::{self, MULTI};

impl App {
    /// Text before the caret, if the focused app shares it (drives `pre_context`).
    pub fn pre_context(&self) -> String {
        if !self.cfg.use_context {
            return String::new();
        }
        self.im
            .as_ref()
            .filter(|ctl| ctl.state.active)
            .and_then(|ctl| ctl.context_before(self.cfg.context_chars))
            .unwrap_or_default()
    }

    /// Canvas size the recognition request should describe.
    fn request_size(&self) -> (u32, u32) {
        let size = self.canvas_rect.size();
        if size.x > 1.0 && size.y > 1.0 {
            (size.x as u32, size.y as u32)
        } else {
            self.logical_size
        }
    }

    /// Start a recognition round for the current ink (bumps the generation).
    pub fn start_recognize(&mut self) {
        if self.ink.is_empty() {
            self.candidates.clear();
            self.busy = false;
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let specs = api::request_specs(&self.cfg, self.multi_ok, self.seq, &self.pre_context());
        let (width, height) = self.request_size();
        self.gen += 1;
        self.expected = specs.len();
        self.collected.clear();
        self.busy = true;
        self.last_error = None;
        api::spawn_requests(
            self.cfg.clone(),
            self.ink.clone(),
            width,
            height,
            specs,
            self.gen,
            self.tx.clone(),
            self.ctx.clone(),
        );
        self.dirty = true;
    }

    /// Drain finished recognition results; called from the main loop.
    pub fn drain_results(&mut self) {
        let mut received = false;
        while let Ok(outcome) = self.rx.try_recv() {
            if outcome.gen == self.gen {
                self.collected.push(outcome);
                received = true;
            }
        }
        if !received || !self.busy || self.collected.len() < self.expected {
            return;
        }

        // Multi-language probe: if the combined request failed the first time,
        // redo the round per language and remember that for the session.
        if self.multi_ok.is_none()
            && self.collected.len() == 1
            && self.collected[0].lang == MULTI
            && self.collected[0].result.is_err()
        {
            self.multi_ok = Some(false);
            self.start_recognize();
            return;
        }
        if self.multi_ok.is_none() && self.collected.len() == 1 && self.collected[0].lang == MULTI {
            self.multi_ok = Some(true);
        }

        let errors: Vec<String> = self
            .collected
            .iter()
            .filter_map(|o| o.result.as_ref().err().cloned())
            .collect();
        self.candidates = api::merge(&self.collected, self.cfg.num_candidates);
        self.last_error = if errors.len() == self.collected.len() && self.candidates.is_empty() {
            errors.first().cloned()
        } else if !errors.is_empty() {
            Some(format!(
                "{} of {} languages failed ({})",
                errors.len(),
                self.collected.len(),
                errors[0]
            ))
        } else {
            None
        };
        self.busy = false;
        self.dirty = true;
    }
}
