//! Application state. Behaviour lives in the sibling submodules:
//! `setup` (Wayland objects), `render` (GPU + egui frame), `recognize`,
//! `send` (injection), `canvas` (gestures), `fonts`.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use egui::{Pos2, Rect};
use wayland_client::protocol::wl_output::WlOutput;
use wayland_client::protocol::wl_pointer::WlPointer;
use wayland_client::protocol::wl_seat::WlSeat;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::ZxdgOutputV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::ZwlrLayerSurfaceV1;

use crate::api::{Candidate, Outcome};
use crate::config::Config;
use crate::ink::Ink;
use crate::inject::{ClipboardCtl, InputMethodCtl, VirtualKeyboard};
use crate::wayland::Globals;

mod canvas;
mod fonts;
mod recognize;
mod render;
mod send;
mod setup;

pub use render::Gpu;

pub struct OutputInfo {
    pub output: WlOutput,
    pub scale: u32,
    /// Logical size from zxdg_output_v1 (0 while unknown).
    pub logical_size: (u32, u32),
}

/// UI intents collected during a frame, executed right after it.
#[derive(Debug, Clone)]
pub enum Action {
    /// Send text to the focused app; `clear` also clears the canvas.
    CommitText { text: String, clear: bool },
    /// Send a special keysym (Enter / Backspace).
    Keysym(u32),
    ClearCanvas,
    /// Remove the most recent stroke and re-recognise what is left.
    UndoStroke,
    /// Insert a space without touching the canvas.
    Space,
    /// Close the panel (and therefore the app).
    Quit,
}

pub struct App {
    pub cfg: Config,
    pub conn: Connection,
    pub qh: QueueHandle<App>,

    pub globals: Globals,
    pub surface: Option<WlSurface>,
    pub layer: Option<ZwlrLayerSurfaceV1>,
    pub outputs: Vec<OutputInfo>,
    pub xdg_outputs: Vec<ZxdgOutputV1>,
    pub fractional: Option<WpFractionalScaleV1>,
    pub viewport: Option<WpViewport>,
    pub have_fractional: bool,

    /// A layer configure has been received at least once.
    pub configured: bool,
    pub logical_size: (u32, u32),
    pub ppp: f32,
    pub needs_surface_reconfigure: bool,

    // input state
    pub pointer: Option<WlPointer>,
    pub touch: Option<wayland_client::protocol::wl_touch::WlTouch>,
    pub seat: Option<WlSeat>,
    pub pointer_pos: Option<Pos2>,
    pub pointer_down: bool,
    pub touch_id: Option<i32>,
    pub touch_pos: Option<Pos2>,
    /// egui events queued between frames.
    pub events: Vec<egui::Event>,

    // injection
    pub vk: Option<VirtualKeyboard>,
    pub im: Option<InputMethodCtl>,
    pub clipboard: Option<ClipboardCtl>,
    pub last_error: Option<String>,

    // rendering
    pub ctx: egui::Context,
    pub gpu: Option<Gpu>,
    pub dirty: bool,
    pub quit: bool,

    // canvas / recognition
    pub ink: Ink,
    pub cur: Option<crate::ink::Stroke>,
    pub canvas_rect: Rect,
    pub candidates: Vec<Candidate>,
    pub busy: bool,
    pub gen: u64,
    pub expected: usize,
    pub collected: Vec<Outcome>,
    /// None: unknown whether a multi-language request works.
    pub multi_ok: Option<bool>,
    pub seq: u32,
    pub rx: Receiver<Outcome>,
    pub tx: Sender<Outcome>,

    pub repaint_sched: Arc<Mutex<Option<Instant>>>,
    pub actions: Vec<Action>,
    pub start: Instant,
}

impl App {
    pub fn new(cfg: Config, conn: Connection, qh: QueueHandle<App>) -> App {
        let ctx = egui::Context::default();
        crate::theme::apply(&ctx, &cfg);
        fonts::install(&ctx, &cfg);
        let (tx, rx) = channel();
        // Bind globals during the first roundtrip.
        let _registry = conn.display().get_registry(&qh, ());
        App {
            cfg,
            conn,
            qh,
            globals: Globals::default(),
            surface: None,
            layer: None,
            outputs: Vec::new(),
            xdg_outputs: Vec::new(),
            fractional: None,
            viewport: None,
            have_fractional: false,
            configured: false,
            logical_size: (0, 0),
            ppp: 1.0,
            needs_surface_reconfigure: false,
            pointer: None,
            touch: None,
            seat: None,
            pointer_pos: None,
            pointer_down: false,
            touch_id: None,
            touch_pos: None,
            events: Vec::new(),
            vk: None,
            im: None,
            clipboard: None,
            last_error: None,
            ctx,
            gpu: None,
            dirty: true,
            quit: false,
            ink: Ink::default(),
            cur: None,
            canvas_rect: Rect::NOTHING,
            candidates: Vec::new(),
            busy: false,
            gen: 0,
            expected: 0,
            collected: Vec::new(),
            multi_ok: None,
            seq: 0,
            rx,
            tx,
            repaint_sched: Arc::new(Mutex::new(None)),
            actions: Vec::new(),
            start: Instant::now(),
        }
    }

    /// Poll timeout in ms for the (wayland fd, eventfd) poll: -1 = block.
    /// Also where deferred repaints become due.
    pub fn poll_timeout_ms(&mut self) -> i32 {
        if self.dirty {
            return 0;
        }
        let mut sched = self.repaint_sched.lock().unwrap();
        match *sched {
            Some(at) => {
                let now = Instant::now();
                if at <= now {
                    *sched = None;
                    self.dirty = true;
                    0
                } else {
                    (at - now).as_millis().clamp(1, 60_000) as i32
                }
            }
            None => -1,
        }
    }
}
