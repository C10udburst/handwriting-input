//! wl_seat / wl_pointer / wl_touch -> egui events + canvas strokes.

use egui::{Event, Pos2, Vec2, Sense};
use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_client::protocol::wl_pointer::{self, WlPointer};
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_touch::{self, WlTouch};
use wayland_client::protocol::wl_surface::WlSurface;

use crate::app::App;

impl Dispatch<WlSeat, ()> for App {
    fn event(
        app: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capability { capabilities } = event {
            let caps = wl_seat::Capability::from_bits_truncate(capabilities.bits());
            if caps.contains(wl_seat::Capability::Pointer) && app.pointer.is_none() {
                app.pointer = Some(seat.get_pointer(qh, ()));
            }
            if caps.contains(wl_seat::Capability::Touch) && app.touch.is_none() {
                app.touch = Some(seat.get_touch(qh, ()));
            }
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
        use wl_pointer::Event;
        match event {
            Event::Enter { x, y, .. } => {
                app.pointer_in = true;
                let p = Pos2::new(x.to_f64() as f32, y.to_f64() as f32);
                app.pointer_pos = Some(p);
                app.events.push(Event::PointerMoved(p));
            }
            Event::Leave { serial: _ } => {
                app.pointer_in = false;
                app.pointer_pos = None;
                app.events.push(Event::PointerGone);
            }
            Event::Motion { x, y, .. } => {
                let p = Pos2::new(x.to_f64() as f32, y.to_f64() as f32);
                app.pointer_pos = Some(p);
                app.events.push(Event::PointerMoved(p));
            }
            Event::Button { serial: _, button, state, .. } => {
                let pressed = state == wl_pointer::WlPointerButtonState::Pressed;
                if button == wl_pointer::WlPointerButtonState::Primary as u32 {
                    let at = app.pointer_pos.unwrap_or(Pos2::ZERO);
                    app.pointer_down = pressed;
                    app.events.push(Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                    if pressed {
                        app.on_pointer_press(at);
                    } else {
                        app.on_pointer_release(at);
                    }
                }
            }
            Event::Axis { axis, value, .. } => {
                let delta = match axis {
                    wl_pointer::WlPointerAxis::ScrollVertically => Vec2::new(0.0, -value.to_f64() as f32),
                    wl_pointer::WlPointerAxis::ScrollHorizontally => {
                        Vec2::new(-value.to_f64() as f32, 0.0)
                    }
                    _ => return,
                };
                app.events.push(Event::MouseWheel {
                    unit: egui::MouseWheelUnit::PixelDelta,
                    delta,
                    modifiers: Default::default(),
                });
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
        use wl_touch::Event;
        match event {
            Event::Down { serial, x, y, id, .. } => {
                app.last_serial = serial;
                let p = Pos2::new(x.to_f64() as f32, y.to_f64() as f32);
                app.touch_id = Some(id);
                app.touch_pos = Some(p);
                app.pointer_pos = Some(p);
                app.pointer_in = true;
                app.pointer_down = true;
                app.events.push(Event::PointerMoved(p));
                app.events.push(Event::PointerButton {
                    pos: p,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                });
                app.on_pointer_press(p);
            }
            Event::Motion { x, y, id, .. } => {
                if app.touch_id == Some(id) {
                    let p = Pos2::new(x.to_f64() as f32, y.to_f64() as f32);
                    app.touch_pos = Some(p);
                    app.pointer_pos = Some(p);
                    app.events.push(Event::PointerMoved(p));
                    app.on_pointer_motion(p);
                }
            }
            Event::Up { id, .. } | Event::Cancel { id, .. } => {
                if app.touch_id == Some(id) {
                    app.touch_id = None;
                    let at = app.touch_pos.take().unwrap_or(Pos2::ZERO);
                    app.pointer_down = false;
                    app.pointer_in = false;
                    app.events.push(Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        modifiers: Default::default(),
                    });
                    app.events.push(Event::PointerGone);
                    app.on_pointer_release(at);
                }
            }
            _ => {}
        }
    }
}

wayland_client::delegate_noop!(App, WlPointer);
wayland_client::delegate_noop!(App, WlTouch);