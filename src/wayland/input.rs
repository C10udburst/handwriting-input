//! `wl_seat` / `wl_pointer` / `wl_touch` → egui events + canvas strokes.

use egui::{Event, Modifiers, MouseWheelSource, MouseWheelUnit, Pos2, TouchPhase, Vec2};
use wayland_client::WEnum;
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::{Connection, Dispatch, QueueHandle};

use crate::app::App;

/// `BTN_LEFT` from the Linux input protocol.
const BTN_LEFT: u32 = 0x110;

impl Dispatch<WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_seat::Event::Capabilities { capabilities } = event else {
            return;
        };
        let Ok(caps) = capabilities.into_result() else {
            return;
        };
        if caps.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
            app.pointer = Some(seat.get_pointer(qh, ()));
        }
        if caps.contains(wl_seat::Capability::Touch) && app.touch.is_none() {
            app.touch = Some(seat.get_touch(qh, ()));
        }
    }
}

impl Dispatch<WlPointer, ()> for App {
    fn event(
        app: &mut Self,
        _pointer: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => {
                let at = Pos2::new(surface_x as f32, surface_y as f32);
                app.pointer_pos = Some(at);
                app.events.push(Event::PointerMoved(at));
                if app.pointer_down {
                    app.on_motion(at);
                }
                app.dirty = true;
            }
            wl_pointer::Event::Leave { .. } => {
                app.pointer_pos = None;
                app.events.push(Event::PointerGone);
                if app.pointer_down {
                    // Left the surface while pressed: end the stroke.
                    app.pointer_down = false;
                    app.on_release(app.canvas_rect.min);
                }
                app.dirty = true;
            }
            wl_pointer::Event::Button {
                button,
                state,
                ..
            } if button == BTN_LEFT => {
                let pressed = matches!(state, WEnum::Value(wl_pointer::ButtonState::Pressed));
                let at = app.pointer_pos.unwrap_or(Pos2::ZERO);
                app.pointer_down = pressed;
                app.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Modifiers::default(),
                });
                if pressed {
                    app.on_press(at);
                } else {
                    app.on_release(at);
                }
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                let f = value as f32;
                let delta = match axis {
                    WEnum::Value(wl_pointer::Axis::VerticalScroll) => Vec2::new(0.0, -f),
                    WEnum::Value(wl_pointer::Axis::HorizontalScroll) => Vec2::new(-f, 0.0),
                    _ => return,
                };
                app.events.push(Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    delta,
                    phase: TouchPhase::Move,
                    source: MouseWheelSource::Wheel,
                    modifiers: Modifiers::default(),
                });
                app.dirty = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlTouch, ()> for App {
    fn event(
        app: &mut Self,
        _touch: &WlTouch,
        event: wl_touch::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            wl_touch::Event::Down { id, x, y, .. } => {
                let at = Pos2::new(x as f32, y as f32);
                app.touch_id = Some(id);
                app.touch_pos = Some(at);
                app.pointer_pos = Some(at);
                app.pointer_down = true;
                app.events.push(Event::PointerMoved(at));
                app.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Modifiers::default(),
                });
                app.on_press(at);
            }
            wl_touch::Event::Motion { id, x, y, .. } if app.touch_id == Some(id) => {
                let at = Pos2::new(x as f32, y as f32);
                app.touch_pos = Some(at);
                app.pointer_pos = Some(at);
                app.events.push(Event::PointerMoved(at));
                app.on_motion(at);
            }
            wl_touch::Event::Up { id, .. } if app.touch_id == Some(id) => {
                let at = app.touch_pos.take().unwrap_or(Pos2::ZERO);
                app.touch_id = None;
                app.pointer_down = false;
                app.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::default(),
                });
                app.events.push(Event::PointerGone);
                app.on_release(at);
            }
            wl_touch::Event::Cancel { .. } => {
                let at = app.touch_pos.take().unwrap_or(Pos2::ZERO);
                app.touch_id = None;
                app.pointer_down = false;
                app.events.push(Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Modifiers::default(),
                });
                app.events.push(Event::PointerGone);
                app.on_release(at);
            }
            _ => {}
        }
    }
}
