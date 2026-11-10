//! zwp_input_method_v2 client glue + zwlr-data-control data source.

use std::io::Write;

use wayland_client::{Connection, Dispatch, QueueHandle};
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_v2::{
    Event as ImEvent, ZwpInputMethodV2,
};
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_device_v1::{
    Event as DeviceEvent, ZwlrDataControlDeviceV1,
};
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
        let Some(ctl) = app.im.as_mut() else {
            return;
        };
        match event {
            ImEvent::Activate => {
                // activate resets all double-buffered state
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
            ImEvent::TextChangeCause { cause: _ } => {}
            ImEvent::ContentHintPurpose { hint, purpose } => {
                ctl.state.content_hint = hint;
                ctl.state.content_purpose = purpose;
            }
            ImEvent::Done => {
                ctl.state.serial = ctl.state.serial.wrapping_add(1);
            }
            ImEvent::Unavailable => {
                // Another input method was already active on this seat.
                eprintln!("input method unavailable (another IM on seat); using fallbacks");
                app.im = None;
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
        // We only set the clipboard; ignore offers from others.
    }
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
        match event {
            SourceEvent::Send { fd } => {
                let text = payload.0.lock().unwrap().clone().unwrap_or_default();
                let mut file = std::fs::File::from(fd);
                let _ = file.write_all(text.as_bytes());
            }
            SourceEvent::Cancelled => {}
            _ => {}
        }
    }
}
