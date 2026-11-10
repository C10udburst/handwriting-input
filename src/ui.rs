//! egui layout: candidate strip + handwriting canvas.

use egui::{Color32, Pos2, RichText, Stroke, Ui, Vec2};

use crate::app::{Action, App};
use crate::inject::keymap::{KEYSYM_BACKSPACE, KEYSYM_RETURN};
use crate::ink::{Ink, Stroke as InkStroke};

pub fn build(ctx: &egui::Context, app: &mut App) {
    let dark = crate::theme::is_dark(&app.cfg);

    // ---- top strip: controls + candidates + punctuation ----
    egui::Panel::top("strip")
        .exact_height(strip_height(&app.cfg))
        .show(ctx, |ui| {
            let mut actions: Vec<Action> = Vec::new();

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if ui
                    .add(egui::Button::new(RichText::new("✕").size(18.0)).min_size(Vec2::new(38.0, 30.0)))
                    .on_hover_text("Clear canvas")
                    .clicked()
                {
                    actions.push(Action::ClearCanvas);
                }
                if ui
                    .add(egui::Button::new(RichText::new("⌫").size(18.0)).min_size(Vec2::new(38.0, 30.0)))
                    .on_hover_text("Send Backspace to the focused app")
                    .clicked()
                {
                    actions.push(Action::Keysym(KEYSYM_BACKSPACE));
                }
                if ui
                    .add(egui::Button::new(RichText::new("⏎").size(18.0)).min_size(Vec2::new(38.0, 30.0)))
                    .on_hover_text("Send Enter to the focused app")
                    .clicked()
                {
                    actions.push(Action::Keysym(KEYSYM_RETURN));
                }
                ui.separator();
                if app.busy {
                    ui.spinner();
                }
                if app.candidates.is_empty() && !app.busy && app.ink.is_empty() {
                    ui.label(RichText::new("draw here").weak().italics());
                }
            });

            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                if !app.candidates.is_empty() {
                    for c in &app.candidates {
                        let btn = egui::Button::new(RichText::new(&c.text).size(20.0)).min_side(30.0);
                        let resp = ui.add(btn);
                        if resp.clicked() {
                            actions.push(Action::CommitText {
                                text: c.text.clone(),
                                clear: true,
                            });
                        }
                        if app.candidates.len() > 1 || c.lang != "multi" {
                            ui.label(RichText::new(&c.lang).size(9.0).weak());
                        }
                    }
                }
                for p in &app.cfg.punctuation {
                    if ui.add(egui::Button::new(RichText::new(p).size(18.0))).clicked() {
                        actions.push(Action::CommitText {
                            text: p.clone(),
                            clear: false,
                        });
                    }
                }
            });

            if let Some(err) = &app.error {
                ui.label(
                    RichText::new(err)
                        .size(12.0)
                        .color(Color32::from_rgb(220, 80, 80)),
                );
            }

            app.actions = actions;
        });

    // ---- canvas ----
    egui::CentralPanel::default()
        .frame(
            egui::Frame::new()
                .fill(if dark {
                    Color32::from_rgb(24, 24, 32)
                } else {
                    Color32::from_rgb(246, 246, 250)
                })
                .inner_margin(6.0),
        )
        .show(ctx, |ui| canvas(ui, app, dark));
}

fn strip_height(cfg: &crate::config::Config) -> f32 {
    let mut h = 84.0;
    if !cfg.punctuation.is_empty() {
        h = h.max(84.0);
    }
    h
}

fn canvas(ui: &mut Ui, app: &mut App, dark: bool) {
    let (rect, _resp) = ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
    app.canvas_rect = rect;
    let painter = ui.painter_at(rect);

    // Subtle writing guides (like the original squared background).
    let guide = if dark {
        Color32::from_gray(48)
    } else {
        Color32::from_gray(214)
    };
    let h = rect.height();
    for i in 1..3 {
        let y = rect.min.y + h * i as f32 / 3.0;
        painter.line_segment(
            [Pos2::new(rect.min.x + 4.0, y), Pos2::new(rect.max.x - 4.0, y)],
            Stroke::new(1.0, guide),
        );
    }

    let ink_color = if dark {
        Color32::from_gray(235)
    } else {
        Color32::from_gray(15)
    };
    let stroke = Stroke::new(app.cfg.stroke_width, ink_color);

    draw_ink(&painter, rect.min.to_vec2(), &app.ink, stroke);
    if let Some(cur) = &app.cur {
        draw_stroke(&painter, rect.min.to_vec2(), cur, stroke);
    }
}

fn draw_ink(painter: &egui::Painter, offset: Vec2, ink: &Ink, stroke: Stroke) {
    for s in &ink.strokes {
        draw_stroke(painter, offset, s, stroke);
    }
}

fn draw_stroke(painter: &egui::Painter, offset: Vec2, s: &InkStroke, stroke: Stroke) {
    if s.points.is_empty() {
        return;
    }
    let pts: Vec<Pos2> = s
        .points
        .iter()
        .map(|p| Pos2::new(p[0] + offset.x, p[1] + offset.y))
        .collect();
    if pts.len() == 1 {
        painter.circle_filled(pts[0], stroke.width * 0.6, stroke.color);
        return;
    }
    for w in pts.windows(2) {
        painter.line_segment([w[0], w[1]], stroke);
    }
}
