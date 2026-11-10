//! Canvas gestures: drawing strokes and handing them to recognition.

use egui::Pos2;

use super::App;
use crate::ink::Stroke;

impl App {
    /// Pointer/touch went down: start a new stroke.
    pub fn on_press(&mut self, at: Pos2) {
        if !self.canvas_rect.contains(at) {
            return;
        }
        let mut stroke = Stroke::default();
        stroke.push(at.x - self.canvas_rect.min.x, at.y - self.canvas_rect.min.y);
        self.cur = Some(stroke);
        self.dirty = true;
    }

    pub fn on_motion(&mut self, at: Pos2) {
        if let Some(stroke) = self.cur.as_mut() {
            stroke.push(at.x - self.canvas_rect.min.x, at.y - self.canvas_rect.min.y);
            self.dirty = true;
        }
    }

    /// Pointer/touch released: finalise the stroke and ask for recognition.
    pub fn on_release(&mut self, at: Pos2) {
        if let Some(mut stroke) = self.cur.take() {
            if !self.canvas_rect.contains(at) {
                // Released outside the canvas: still keep what was drawn.
                stroke.push(at.x - self.canvas_rect.min.x, at.y - self.canvas_rect.min.y);
            }
            if !stroke.is_empty() {
                self.ink.strokes.push(stroke);
            }
            self.start_recognize();
        }
        self.dirty = true;
    }

    pub fn clear_canvas(&mut self) {
        self.ink.clear();
        self.cur = None;
        self.candidates.clear();
        self.last_error = None;
        self.abandon_recognition();
        self.dirty = true;
    }

    /// Undo the last stroke: exactly what the user meant to erase. The dropped
    /// stroke can carry an in-flight request's result away with it, so bump the
    /// generation counter and ask again about what remains.
    pub fn undo_stroke(&mut self) {
        if self.ink.strokes.pop().is_none() {
            return;
        }
        self.cur = None;
        self.candidates.clear();
        self.last_error = None;
        self.abandon_recognition();
        if !self.ink.is_empty() {
            self.start_recognize();
        }
        self.dirty = true;
    }

    /// Drop in-flight results so they can no longer update the UI.
    pub fn abandon_recognition(&mut self) {
        self.gen += 1;
        self.busy = false;
        self.collected.clear();
        self.expected = 0;
    }
}
