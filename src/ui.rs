//! egui layout: candidate/control strip on top, handwriting canvas below.

use egui::{Align, Color32, Frame, Layout, Pos2, RichText, Shape, Stroke, Ui, Vec2};

use crate::app::{Action, App};
use crate::ink::{Ink, Stroke as InkStroke};
use crate::inject::{KEYSYM_BACKSPACE, KEYSYM_RETURN};

/// Height of the strip holding buttons and candidates.
const STRIP_HEIGHT: f32 = 82.0;

/// Build the whole frame inside the root `Ui`.
pub fn build(root: &mut Ui, app: &mut App) {
    let dark = crate::theme::is_dark(&app.cfg);

    egui::Panel::top("controls")
        .exact_size(STRIP_HEIGHT)
        .show(root, |ui| controls(ui, app));

    egui::CentralPanel::default()
        .frame(
            Frame::new()
                .fill(if dark {
                    Color32::from_rgb(24, 24, 32)
                } else {
                    Color32::from_rgb(246, 246, 250)
                })
                .inner_margin(6.0),
        )
        .show(root, |ui| canvas(ui, app, dark));
}

fn controls(ui: &mut Ui, app: &mut App) {
    let mut actions: Vec<Action> = Vec::new();
    ui.spacing_mut().item_spacing.x = 6.0;

    ui.horizontal(|ui| {
        for (label, hint, action) in [
            ("×", "Clear the canvas", Action::ClearCanvas),
            ("undo", "Undo the last stroke", Action::UndoStroke),
            ("space", "Type a space in the focused app", Action::Space),
            (
                "del",
                "Backspace in the focused app",
                Action::Keysym(KEYSYM_BACKSPACE),
            ),
            (
                "enter",
                "Enter in the focused app",
                Action::Keysym(KEYSYM_RETURN),
            ),
        ] {
            let size = if label.len() == 1 { 20.0 } else { 14.0 };
            let width = if label.len() == 1 { 34.0 } else { 52.0 };
            let button = egui::Button::new(RichText::new(label).size(size))
                .min_size(Vec2::new(width, 28.0));
            if ui.add(button).on_hover_text(hint).clicked() {
                actions.push(action);
            }
        }
        ui.separator();
        if app.busy {
            ui.spinner();
        }
        if app.ink.is_empty() && app.candidates.is_empty() && !app.busy {
            ui.label(RichText::new("draw here").weak().italics());
        }

        // Close button, pinned to the right edge of the strip.
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            let quit = egui::Button::new(RichText::new("quit").size(13.0))
                .min_size(Vec2::new(44.0, 24.0));
            if ui
                .add(quit)
                .on_hover_text("Close the handwriting panel")
                .clicked()
            {
                actions.push(Action::Quit);
            }
        });
    });

    ui.horizontal(|ui| {
        for candidate in &app.candidates {
            let button = egui::Button::new(RichText::new(&candidate.text).size(20.0))
                .min_size(Vec2::new(34.0, 30.0));
            if ui.add(button).clicked() {
                actions.push(Action::CommitText {
                    text: candidate.text.clone(),
                    clear: true,
                });
            }
            // Show which language produced it when several are in play.
            if app.cfg.langs.len() > 1 {
                ui.label(RichText::new(&candidate.lang).size(9.0).weak());
            }
        }
        for punctuation in &app.cfg.punctuation {
            if ui
                .add(egui::Button::new(RichText::new(punctuation).size(18.0)))
                .clicked()
            {
                actions.push(Action::CommitText {
                    text: punctuation.clone(),
                    clear: false,
                });
            }
        }
    });

    if let Some(error) = &app.last_error {
        ui.label(
            RichText::new(error)
                .size(12.0)
                .color(Color32::from_rgb(220, 90, 90)),
        );
    }

    app.actions = actions;
}

fn canvas(ui: &mut Ui, app: &mut App, dark: bool) {
    let (rect, _) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    app.canvas_rect = rect;
    let painter = ui.painter_at(rect);

    // Faint writing guides, echoing the squared paper of the original app.
    let guide = if dark {
        Color32::from_gray(48)
    } else {
        Color32::from_gray(214)
    };
    for i in 1..3 {
        let y = rect.min.y + rect.height() * i as f32 / 3.0;
        painter.line_segment(
            [
                Pos2::new(rect.min.x + 4.0, y),
                Pos2::new(rect.max.x - 4.0, y),
            ],
            Stroke::new(1.0, guide),
        );
    }

    let style = Stroke::new(
        app.cfg.stroke_width,
        if dark {
            Color32::from_gray(235)
        } else {
            Color32::from_gray(15)
        },
    );
    for stroke in &app.ink.strokes {
        draw_stroke(&painter, rect.min.to_vec2(), stroke, style);
    }
    if let Some(stroke) = &app.cur {
        draw_stroke(&painter, rect.min.to_vec2(), stroke, style);
    }
}

/// One polyline per stroke: tessellating a single path avoids the dark seams
/// that appear where separate anti-aliased segments overlap.
fn draw_stroke(painter: &egui::Painter, offset: Vec2, stroke: &InkStroke, style: Stroke) {
    let points: Vec<Pos2> = stroke
        .points
        .iter()
        .map(|p| Pos2::new(p[0] + offset.x, p[1] + offset.y))
        .collect();
    if points.len() == 1 {
        painter.circle_filled(points[0], style.width * 0.6, style.color);
    } else {
        painter.add(Shape::line(points, style));
    }
}

/// Keep the unused-import lint quiet about the Ink alias used in signatures.
#[allow(dead_code)]
fn _ink_alias(_: &Ink) {}
