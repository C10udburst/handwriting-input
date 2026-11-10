//! `zwp_input_method_v2` state tracking and the `zwlr_data_control` source.

use std::io::Write;

use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    Event as ImEvent, ZwpInputMethodV2,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    Event as DeviceEvent, ZwlrDataControlDeviceV1, EVT_DATA_OFFER_OPCODE,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_source_v1::{
    Event as SourceEvent, ZwlrDataControlSourceV1,
};

use crate::app::App;
use crate::inject::ClipboardPayload;

impl Dispatch<ZwpInputMethodV2, ()> for App {
    fn event(
        app: &mut Self,
        _im: &ZwpInputMethodV2,
        event: ImEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // A second IM on the seat makes this one permanently useless.
        if matches!(event, ImEvent::Unavailable) {
            eprintln!("input method unavailable (another IM holds the seat); using fallbacks");
            app.im = None;
            app.dirty = true;
            return;
        }
        let Some(ctl) = app.im.as_mut() else {
            return;
        };
        match event {
            ImEvent::Activate => {
                // Activation resets all double-buffered state.
                ctl.state.active = true;
                ctl.state.surrounding.clear();
                ctl.state.cursor = 0;
                ctl.state.anchor = 0;
                ctl.state.supports_surrounding = false;
                ctl.state.last_preedit_sent = None;
                app.dirty = true;
            }
            ImEvent::Deactivate => {
                ctl.state.active = false;
                ctl.state.last_preedit_sent = None;
                app.dirty = true;
            }
            ImEvent::SurroundingText {
                text,
                cursor,
                anchor,
            } => {
                ctl.state.supports_surrounding = true;
                ctl.state.surrounding = text;
                ctl.state.cursor = cursor as usize;
                ctl.state.anchor = anchor as usize;
            }
            // Store the raw enum values; `u32` conversion keeps the code
            // independent of unknown-variant handling.
            ImEvent::ContentType { hint, purpose } => {
                ctl.state.content_hint = hint.into();
                ctl.state.content_purpose = purpose.into();
            }
            ImEvent::TextChangeCause { .. } => {}
            ImEvent::Done => {
                ctl.state.serial = ctl.state.serial.wrapping_add(1);
            }
            _ => {}
        }
    }
}

impl Dispatch<ZwlrDataControlDeviceV1, ()> for App {
    fn event(
        _app: &mut Self,
        _device: &ZwlrDataControlDeviceV1,
        _event: DeviceEvent,
        _: &(),
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // We only claim the clipboard; offers from others are ignored.
    }

    // The `data_offer` event hands us a new offer object.
    wayland_client::event_created_child!(App, ZwlrDataControlDeviceV1, [
        EVT_DATA_OFFER_OPCODE => (ZwlrDataControlOfferV1, ())
    ]);
}

impl Dispatch<ZwlrDataControlSourceV1, ClipboardPayload> for App {
    fn event(
        _app: &mut Self,
        _source: &ZwlrDataControlSourceV1,
        event: SourceEvent,
        payload: &ClipboardPayload,
        _: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
        // The compositor asks per mime type; we only ever offer plain text.
        if let SourceEvent::Send { fd, mime_type } = event {
            if mime_type.starts_with("text/") {
                let text = payload.0.lock().unwrap().clone().unwrap_or_default();
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(text.as_bytes());
            }
        }
    }
}
