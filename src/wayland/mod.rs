//! Registry globals and their `Dispatch` implementations.

pub mod im;
pub mod input;
pub mod shell;

use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_registry;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_shm::WlShm;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::wp::viewporter::client::wp_viewporter::WpViewporter;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1;
use wayland_protocols_misc::zwp_input_method_v2::client::zwp_input_method_manager_v2::ZwpInputMethodManagerV2;
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_manager_v1::ZwlrDataControlManagerV1;
use wayland_protocols_wlr::data_control::v1::client::zwlr_data_control_offer_v1::ZwlrDataControlOfferV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1;

use crate::app::{App, OutputInfo};

/// Managers bound once from the registry.
#[derive(Default)]
pub struct Globals {
    pub compositor: Option<WlCompositor>,
    pub layer_shell: Option<ZwlrLayerShellV1>,
    pub seat: Option<WlSeat>,
    pub shm: Option<WlShm>,
    pub xdg_output_mgr: Option<ZxdgOutputManagerV1>,
    pub fractional_mgr: Option<WpFractionalScaleManagerV1>,
    pub viewporter: Option<WpViewporter>,
    pub vk_mgr: Option<ZwpVirtualKeyboardManagerV1>,
    pub data_control_mgr: Option<ZwlrDataControlManagerV1>,
    pub im_mgr: Option<ZwpInputMethodManagerV2>,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        let g = &mut app.globals;
        match interface.as_str() {
            "wl_compositor" => {
                g.compositor = Some(registry.bind::<WlCompositor, _, _>(name, version.min(6), qh, ()));
            }
            "zwlr_layer_shell_v1" => {
                g.layer_shell =
                    Some(registry.bind::<ZwlrLayerShellV1, _, _>(name, version.min(4), qh, ()));
            }
            "wl_seat" if g.seat.is_none() => {
                g.seat = Some(registry.bind::<WlSeat, _, _>(name, version.min(9), qh, ()));
            }
            "wl_shm" => {
                g.shm = Some(registry.bind::<WlShm, _, _>(name, version.min(1), qh, ()));
            }
            "zxdg_output_manager_v1" => {
                g.xdg_output_mgr =
                    Some(registry.bind::<ZxdgOutputManagerV1, _, _>(name, version.min(3), qh, ()));
            }
            "wp_fractional_scale_manager_v1" => {
                g.fractional_mgr = Some(
                    registry
                        .bind::<WpFractionalScaleManagerV1, _, _>(name, version.min(1), qh, ()),
                );
            }
            "wp_viewporter" => {
                g.viewporter =
                    Some(registry.bind::<WpViewporter, _, _>(name, version.min(1), qh, ()));
            }
            "zwp_virtual_keyboard_manager_v1" => {
                g.vk_mgr = Some(
                    registry
                        .bind::<ZwpVirtualKeyboardManagerV1, _, _>(name, version.min(1), qh, ()),
                );
            }
            "zwlr_data_control_manager_v1" => {
                g.data_control_mgr = Some(
                    registry
                        .bind::<ZwlrDataControlManagerV1, _, _>(name, version.min(1), qh, ()),
                );
            }
            "zwp_input_method_manager_v2" => {
                g.im_mgr = Some(
                    registry
                        .bind::<ZwpInputMethodManagerV2, _, _>(name, version.min(1), qh, ()),
                );
            }
            "wl_output" => {
                let output = registry.bind::<WlOutput, _, _>(name, version.min(4), qh, ());
                app.outputs.push(OutputInfo {
                    output: output.clone(),
                    scale: 1,
                    logical_size: (0, 0),
                });
                // Logical (scale-divided) size, for exact panel placement.
                if let Some(mgr) = &g.xdg_output_mgr {
                    app.xdg_outputs.push(mgr.get_xdg_output(&output, qh, ()));
                }
            }
            _ => {}
        }
    }
}

// Objects that send no events worth handling.
wayland_client::delegate_noop!(App: ignore WlCompositor);
wayland_client::delegate_noop!(App: ignore WlShm);
wayland_client::delegate_noop!(App: ignore WlSurface);
wayland_client::delegate_noop!(App: ignore ZwlrLayerShellV1);
wayland_client::delegate_noop!(App: ignore ZxdgOutputManagerV1);
wayland_client::delegate_noop!(App: ignore WpFractionalScaleManagerV1);
wayland_client::delegate_noop!(App: ignore WpViewporter);
wayland_client::delegate_noop!(App: ignore WpViewport);
wayland_client::delegate_noop!(App: ignore ZwlrDataControlManagerV1);
wayland_client::delegate_noop!(App: ignore ZwlrDataControlOfferV1);
wayland_client::delegate_noop!(App: ignore ZwpInputMethodManagerV2);
wayland_client::delegate_noop!(App: ignore ZwpVirtualKeyboardManagerV1);
wayland_client::delegate_noop!(App: ignore ZwpVirtualKeyboardV1);

use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1;
