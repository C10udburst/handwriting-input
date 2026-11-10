//! Layer surface, outputs, xdg-output logical sizes, fractional scale.

use wayland_client::protocol::wl_output::{Event as OutputEvent, WlOutput};
use wayland_client::{Connection, Dispatch, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::{
    Event as FractionalEvent, WpFractionalScaleV1,
};
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::{
    Event as XdgOutputEvent, ZxdgOutputV1,
};
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
                // 0 means "use the whole axis"; fall back to the output size.
                let w = if width == 0 {
                    app.output_logical_width()
                } else {
                    width
                };
                let h = if height == 0 {
                    app.cfg.height
                } else {
                    height
                };
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
        event: OutputEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        let Some(entry) = app.outputs.iter_mut().find(|o| o.output == *output) else {
            return;
        };
        match event {
            OutputEvent::Scale { factor } if factor > 0 => {
                entry.scale = factor as u32;
                app.refresh_scale();
            }
            OutputEvent::Done => {
                app.refresh_scale();
            }
            _ => {}
        }
    }
}

impl Dispatch<ZxdgOutputV1, ()> for App {
    fn event(
        app: &mut Self,
        xdg_output: &ZxdgOutputV1,
        event: XdgOutputEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // xdg_outputs[i] was created right after outputs[i].
        let Some(index) = app.xdg_outputs.iter().position(|o| o == xdg_output) else {
            return;
        };
        if let XdgOutputEvent::LogicalSize { width, height } = event {
            if let Some(entry) = app.outputs.get_mut(index) {
                entry.logical_size = (width.max(0) as u32, height.max(0) as u32);
            }
            app.dirty = true;
        }
    }
}

impl Dispatch<WpFractionalScaleV1, ()> for App {
    fn event(
        app: &mut Self,
        _scale: &WpFractionalScaleV1,
        event: FractionalEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        if let FractionalEvent::PreferredScale { scale } = event {
            // The protocol sends the numerator of a fraction over 120.
            let value = scale as f32 / 120.0;
            if value > 0.0 && (app.ppp - value).abs() > f32::EPSILON {
                app.ppp = value;
                app.needs_surface_reconfigure = true;
                app.dirty = true;
            }
        }
    }
}
