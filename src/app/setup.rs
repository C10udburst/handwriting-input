//! Wayland object setup performed after the first roundtrip: layer surface,
//! scaling, and the three injection layers.

use wayland_client::Proxy;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity,
};

use super::App;
use crate::config::{Config, LayerMode, PositionMode};
use crate::inject::{ClipboardCtl, InputMethodCtl, VirtualKeyboard};

fn layer_of(cfg: &Config) -> Layer {
    match cfg.layer {
        LayerMode::Top => Layer::Top,
        LayerMode::Overlay => Layer::Overlay,
    }
}

impl App {
    /// Create the layer surface, hook up scaling, and bind injection objects.
    pub fn after_globals(&mut self) -> Result<(), String> {
        if self.globals.compositor.is_none() {
            return Err("compositor does not advertise wl_compositor".into());
        }
        let shell = self
            .globals
            .layer_shell
            .clone()
            .ok_or("zwlr_layer_shell_v1 is not available (not a wlroots compositor?)")?;
        let compositor = self.globals.compositor.clone().unwrap();
        let qh = self.qh.clone();

        let surface = compositor.create_surface(&qh, ());
        let output = self.outputs.first().map(|o| o.output.clone());
        let layer = shell.get_layer_surface(
            &surface,
            output.as_ref(),
            layer_of(&self.cfg),
            "handwriting-input".into(),
            &qh,
            (),
        );

        match self.cfg.position {
            PositionMode::Bottom => {
                layer.set_anchor(Anchor::Bottom | Anchor::Left | Anchor::Right);
                // width 0 with both side anchors => span the whole output width
                layer.set_size(0, self.cfg.height);
            }
            PositionMode::Center => {
                let out_w = self.output_logical_width() as i32;
                let out_h = self.output_logical_height() as i32;
                let w = (out_w * 3 / 4).clamp(320, 1400);
                let h = (self.cfg.height as i32).clamp(120, out_h.max(120));
                let vpad = ((out_h - h) / 2).max(0);
                let hpad = ((out_w - w) / 2).max(0);
                layer.set_anchor(Anchor::empty());
                layer.set_size(w as u32, h as u32);
                layer.set_margin(vpad, hpad, vpad, hpad);
            }
        }
        // Never grab the keyboard: the app that had focus keeps receiving keys.
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        if shell.version() >= 4 {
            layer.set_layer(layer_of(&self.cfg));
        }

        // Scaling: fractional + viewporter when available, integer buffer_scale otherwise.
        if let Some(mgr) = self.globals.fractional_mgr.clone() {
            self.fractional = Some(mgr.get_fractional_scale(&surface, &qh, ()));
            self.have_fractional = true;
            if let Some(vp_mgr) = self.globals.viewporter.clone() {
                self.viewport = Some(vp_mgr.get_viewport(&surface, &qh, ()));
            }
        } else {
            let s = self.integer_scale();
            surface.set_buffer_scale(s as i32);
        }

        self.surface = Some(surface);
        self.layer = Some(layer);
        self.setup_injection();
        // A layer surface is only mapped (and only then configured) once its
        // wl_surface is committed, so do that here before waiting for it.
        if let Some(surface) = self.surface.as_ref() {
            surface.commit();
        }
        Ok(())
    }

    /// Bind virtual keyboard / input method / data control for the seat.
    fn setup_injection(&mut self) {
        let Some(seat) = self.globals.seat.clone() else {
            eprintln!("injection: no wl_seat, cannot inject text");
            return;
        };
        self.seat = Some(seat.clone());
        let qh = self.qh.clone();

        if let Some(mgr) = self.globals.vk_mgr.clone() {
            self.vk = Some(VirtualKeyboard::new(mgr.create_virtual_keyboard(&seat, &qh, ())));
        }
        if let Some(mgr) = self.globals.data_control_mgr.clone() {
            let device = mgr.get_data_device(&seat, &qh, ());
            self.clipboard = Some(ClipboardCtl::new(mgr, device));
        }
        if let Some(mgr) = self.globals.im_mgr.clone() {
            self.im = Some(InputMethodCtl::new(mgr.get_input_method(&seat, &qh, ())));
        }

        let mut missing = Vec::new();
        if self.im.is_none() {
            missing.push("no zwp_input_method_v2");
        }
        if self.vk.is_none() {
            missing.push("no zwp_virtual_keyboard_v1");
        }
        if self.clipboard.is_none() {
            missing.push("no zwlr_data_control");
        }
        if !missing.is_empty() {
            eprintln!("injection: {}", missing.join(", "));
        }
    }

    pub fn integer_scale(&self) -> u32 {
        self.outputs.first().map(|o| o.scale.max(1)).unwrap_or(1)
    }

    pub fn output_logical_width(&self) -> u32 {
        self.outputs
            .first()
            .and_then(|o| (o.logical_size.0 > 0).then_some(o.logical_size.0))
            .unwrap_or(1920)
    }

    pub fn output_logical_height(&self) -> u32 {
        self.outputs
            .first()
            .and_then(|o| (o.logical_size.1 > 0).then_some(o.logical_size.1))
            .unwrap_or(1080)
    }

    /// Integer-scale fallback: recompute ppp when fractional scale is absent.
    pub fn refresh_scale(&mut self) {
        if self.have_fractional {
            return;
        }
        let s = self.integer_scale() as f32;
        if (self.ppp - s).abs() > f32::EPSILON {
            self.ppp = s;
            self.needs_surface_reconfigure = true;
            self.dirty = true;
            if let Some(surface) = self.surface.clone() {
                surface.set_buffer_scale(s as i32);
            }
        }
    }
}
