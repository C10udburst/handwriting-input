//! Layer-shell surface, outputs, fractional scale, viewporter.

use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::{Connection, Dispatch, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    Event as FracEvent, WpFractionalScaleV1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{Event as XdgEvent, ZxdgOutputV1};
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Event as LayerEvent, ZwlrLayerSurfaceV1,
};

use crate::app::App;

impl Dispatch<ZwlrLayerSurfaceV1, ()> for App {
    fn event(
        app: &mut Self,
        layer: &ZwlrLayerSurfaceV1,
        event: LayerEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            LayerEvent::Configure {
                serial,
                width,
                height,
            } => {
                layer.ack_configure(serial);
                let w = if width == 0 {
                    app.first_output_logical_width()
                } else {
                    width
                };
                let h = if height == 0 { app.cfg.height } else { height };
                app.logical_size = (w.max(1), h.max(1));
                app.configured = true;
                app.needs_surface_reconfigure = true;
                app.dirty = true;
            }
            LayerEvent::Closed => {
                app.quit = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WlOutput, ()> for App {
    fn event(
        app: &mut Self,
        output: &WlOutput,
        event: wayland_client::protocol::wl_output::Event,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        use wayland_client::protocol::wl_output::Event as OutEvent;
        let Some(entry) = app.outputs.iter_mut().find(|o| o.output.id() == output.id()) else {
            return;
        };
        match event {
            OutEvent::Scale { factor } => {
                entry.scale = factor.max(1);
                app.refresh_scale();
            }
            OutEvent::Geometry { .. } => {
                app.refresh_scale();
            }
            OutEvent::Done => {
                app.refresh_scale();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, ()> for App {
    fn event(
        app: &mut Self,
        xdg: &ZxdgOutputV1,
        event: XdgEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        match event {
            XdgEvent::LogicalSize { width, height } => {
                // xdg_outputs[i] corresponds to outputs[i] (created in order).
                if let Some(idx) = app.xdg_outputs.iter().position(|o| o.id() == xdg.id()) {
                    if let Some(entry) = app.outputs.get_mut(idx) {
                        entry.logical_size = (width.max(0) as u32, height.max(0) as u32);
                    }
                }
                app.dirty = true;
            }
            XdgEvent::LogicalPosition { .. } | XdgEvent::Done => {
                app.dirty = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _scale: &WpFractionalScaleV1,
        event: FracEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let FracEvent::PreferredScale { scale } = event {
            let f = scale.to_f64() as f32;
            if f > 0.0 {
                app.ppp = f;
                app.needs_surface_reconfigure = true;
                app.dirty = true;
            }
        }
    }
}
