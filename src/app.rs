//! Application state, frame loop logic, injection orchestration, recognition.

use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use egui::{Pos2, RawInput, Rect, Vec2};
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, QueueHandle};

use wayland_protocols::wp::fractional_scale::v1::client::wp_fractional_scale_v1::WpFractionalScaleV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;
use wayland_protocols::xdg::xdg_output::zv1::client::zxdg_output_v1::ZxdgOutputV1;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::Layer;
use wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_surface_v1::{
    Anchor, KeyboardInteractivity, ZwlrLayerSurfaceV1,
};

use crate::api::{self, Candidate, Outcome};
use crate::config::{Config, PositionMode, SendMode};
use crate::ink::{Ink, Stroke};
use crate::inject::keymap::{KEYSYM_BACKSPACE, KEYSYM_RETURN};
use crate::inject::{ClipboardCtl, ClipboardPayload, InputMethodCtl, VirtualKeyboard};
use crate::wayland::Globals;

pub struct OutputInfo {
    pub output: wayland_client::protocol::wl_output::WlOutput,
    pub scale: u32,
    pub logical_size: (u32, u32),
}

pub struct Gpu {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub config: wgpu::SurfaceConfiguration,
}

/// Actions collected by the UI during a frame, executed right after the frame.
#[derive(Debug, Clone)]
pub enum Action {
    /// Inject a candidate/word into the focused app; optionally clear the canvas.
    CommitText { text: String, clear: bool },
    /// Inject a special keysym (Enter, Backspace).
    Keysym(u32),
    /// Copy text to clipboard (no typing).
    CopyText(String),
    ClearCanvas,
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

    pub configured: bool,
    pub logical_size: (u32, u32),
    pub ppp: f32,
    pub needs_surface_reconfigure: bool,

    pub pointer: Option<wayland_client::protocol::wl_pointer::WlPointer>,
    pub touch: Option<wayland_client::protocol::wl_touch::WlTouch>,
    pub pointer_in: bool,
    pub pointer_down: bool,
    pub pointer_pos: Option<Pos2>,
    pub touch_id: Option<i32>,
    pub touch_pos: Option<Pos2>,
    pub last_serial: u32,

    /// egui events queued between frames.
    pub events: Vec<egui::Event>,

    pub vk: Option<VirtualKeyboard>,
    pub im: Option<InputMethodCtl>,
    pub clipboard: Option<ClipboardCtl>,

    pub ctx: egui::Context,
    pub gpu: Option<Gpu>,
    pub renderer: Option<egui_wgpu::Renderer>,

    pub dirty: bool,
    pub quit: bool,

    pub ink: Ink,
    pub cur: Option<Stroke>,
    pub canvas_rect: Rect,
    pub last_release: Option<Instant>,

    pub candidates: Vec<Candidate>,
    pub error: Option<String>,
    pub busy: bool,
    pub gen: u64,
    pub expected: usize,
    pub collected: Vec<Outcome>,
    /// None: unknown, Some(true): combined multi request works.
    pub probe: Option<bool>,
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
        setup_fonts(&ctx, &cfg);
        let (tx, rx) = channel();
        // The registry we feed into the initial roundtrip.
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
            pointer_in: false,
            pointer_down: false,
            pointer_pos: None,
            touch_id: None,
            touch_pos: None,
            last_serial: 0,
            events: Vec::new(),
            vk: None,
            im: None,
            clipboard: None,
            ctx,
            gpu: None,
            renderer: None,
            dirty: true,
            quit: false,
            ink: Ink::default(),
            cur: None,
            canvas_rect: Rect::NOT_EMPTY,
            last_release: None,
            candidates: Vec::new(),
            error: None,
            busy: false,
            gen: 0,
            expected: 0,
            collected: Vec::new(),
            probe: None,
            seq: 0,
            rx,
            tx,
            repaint_sched: Arc::new(Mutex::new(None)),
            actions: Vec::new(),
            start: Instant::now(),
        }
    }

    // ------------------------------------------------------------------
    // setup after the initial roundtrip (globals bound)
    // ------------------------------------------------------------------

    pub fn after_globals(&mut self) -> Result<(), String> {
        if self.globals.compositor.is_none() {
            return Err("compositor does not advertise wl_compositor".into());
        }
        if self.globals.layer_shell.is_none() {
            return Err("wlroots layer-shell (zwlr_layer_shell_v1) is not available".into());
        }

        let qh = self.qh.clone();
        let compositor = self.globals.compositor.as_ref().unwrap();
        let shell = self.globals.layer_shell.as_ref().unwrap();

        let surface = compositor.create_surface(&qh, ());
        let output = self.outputs.first().map(|o| o.output.clone());
        let layer = shell.get_layer_surface(
            &surface,
            output.as_ref(),
            match self.cfg.layer {
                crate::config::LayerMode::Top => Layer::Top,
                crate::config::LayerMode::Overlay => Layer::Overlay,
            },
            "handwriting-input".into(),
            &qh,
            (),
        );

        match self.cfg.position {
            PositionMode::Bottom => {
                layer.set_anchor(Anchor::Bottom | Anchor::Left | Anchor::Right);
                // width 0 + left/right anchor => full output width
                layer.set_size(0, self.cfg.height);
            }
            PositionMode::Center => {
                let out_w = self.first_output_logical_width() as i32;
                let out_h = self.first_output_logical_height() as i32;
                let w = (out_w * 3 / 4).clamp(320, 1400);
                let h = self.cfg.height.min(out_h.max(120)) as i32;
                layer.set_anchor(Anchor::empty());
                layer.set_size(w as u32, h as u32);
                layer.set_margin(
                    ((out_h - h) / 2).max(0),
                    ((out_w - w) / 2).max(0),
                    ((out_h - h) - (out_h - h) / 2).max(0),
                    ((out_w - w) - (out_w - w) / 2).max(0),
                );
            }
        }
        // We never take keyboard focus: the previously focused app keeps it.
        layer.set_keyboard_interactivity(KeyboardInteractivity::None);
        layer.set_exclusive_zone(0);
        if shell.version() >= 3 {
            layer.set_layer(match self.cfg.layer {
                crate::config::LayerMode::Top => Layer::Top,
                crate::config::LayerMode::Overlay => Layer::Overlay,
            });
        }

        // Scaling.
        if let Some(mgr) = &self.globals.fractional_mgr {
            self.fractional = Some(mgr.get_fractional_scale(&surface, &qh, ()));
            self.have_fractional = true;
            if let Some(vp_mgr) = &self.globals.viewporter {
                self.viewport = Some(vp_mgr.get_viewport(&surface, &qh, ()));
            }
        } else {
            let s = self.outputs.first().map(|o| o.scale).unwrap_or(1);
            surface.set_buffer_scale(s as i32);
        }

        self.surface = Some(surface);
        self.layer = Some(layer);

        // Injection layers (selection order decided in inject_text).
        if let Some(seat) = self.globals.seat.clone() {
            if let Some(mgr) = &self.globals.vk_mgr {
                let vk = mgr.create_virtual_keyboard(&seat, &qh, ());
                self.vk = Some(VirtualKeyboard::new(vk));
            }
            if let Some(mgr) = &self.globals.data_control_mgr {
                let device = mgr.get_data_device(&seat, &qh, ());
                self.clipboard = Some(ClipboardCtl {
                    mgr: mgr.clone(),
                    device,
                    payload: ClipboardPayload::default(),
                });
            }
            if let Some(mgr) = &self.globals.im_mgr {
                let im = mgr.get_input_method(&seat, &qh, ());
                self.im = Some(InputMethodCtl {
                    im,
                    state: Default::default(),
                });
            }
        }

        let mut note = String::new();
        if self.vk.is_none() {
            note.push_str("no virtual keyboard; ");
        }
        if self.im.is_none() {
            note.push_str("no input method; ");
        }
        if !self.vk.is_some() && !self.im.is_some() && self.clipboard.is_some() {
            note.push_str("falling back to clipboard copy only");
        }
        if !note.is_empty() {
            eprintln!("injection: {note}");
        }
        Ok(())
    }

    pub fn first_output_logical_width(&self) -> u32 {
        self.outputs
            .first()
            .and_then(|o| (o.logical_size.0 > 0).then_some(o.logical_size.0))
            .unwrap_or(1920)
    }

    pub fn first_output_logical_height(&self) -> u32 {
        self.outputs
            .first()
            .and_then(|o| (o.logical_size.1 > 0).then_some(o.logical_size.1))
            .unwrap_or(1080)
    }

    /// Recompute the integer-scale fallback ppp (fractional overrides it).
    pub fn refresh_scale(&mut self) {
        if self.have_fractional {
            return;
        }
        let s = self.outputs.first().map(|o| o.scale).unwrap_or(1) as f32;
        if (self.ppp - s).abs() > f32::EPSILON {
            self.ppp = s;
            self.needs_surface_reconfigure = true;
            self.dirty = true;
            if s >= 1.0 {
                if let Some(surface) = &self.surface {
                    surface.set_buffer_scale(s as i32);
                }
            }
        }
    }

    // ------------------------------------------------------------------
    // GPU / rendering
    // ------------------------------------------------------------------

    pub fn init_gpu(&mut self) -> Result<(), String> {
        use wgpu::rwh::{RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle};

        let surface_obj = self.surface.as_ref().ok_or("no wayland surface")?;
        let surf_ptr = surface_obj
            .id()
            .as_ptr()
            .map_err(|_| "invalid wl_surface".to_string())?
            .cast::<core::ffi::c_void>();
        let disp_ptr = self
            .conn
            .display()
            .display_id()
            .as_ptr()
            .map_err(|_| "invalid wl_display".to_string())?
            .cast::<core::ffi::c_void>();

        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor {
            backends: wgpu::Backends::VULKAN | wgpu::Backends::GL,
            ..Default::default()
        });

        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: Some(RawDisplayHandle::Wayland(WaylandDisplayHandle::new(disp_ptr))),
                raw_window_handle: RawWindowHandle::Wayland(WaylandWindowHandle::new(surf_ptr)),
            })
        }
        .map_err(|e| format!("wgpu surface creation failed: {e}"))?;

        let adapters = pollster::block_on(instance.request_adapter(wgpu::Backends::VULKAN | wgpu::Backends::GL));
        let adapter = adapters
            .iter()
            .find(|a| a.is_surface_supported(&surface))
            .ok_or("no GPU adapter supports the Wayland surface")?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("handwriting-input"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| format!("GPU device request failed: {e}"))?;

        let (w, h) = self.buffer_size();
        let mut config = surface.get_default_config(adapter, w, h);
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);
        self.renderer = Some(egui_wgpu::Renderer::new(
            &device,
            config.format,
            egui_wgpu::RendererOptions::default(),
        ));
        self.gpu = Some(Gpu {
            instance,
            surface,
            device,
            queue,
            format: config.format,
            config,
        });
        Ok(())
    }

    pub fn buffer_size(&self) -> (u32, u32) {
        let (lw, lh) = self.logical_size;
        let bw = ((lw as f32 * self.ppp).round().max(1.0)) as u32;
        let bh = ((lh as f32 * self.ppp).round().max(1.0)) as u32;
        (bw.max(1), bh.max(1))
    }

    fn reconfigure_if_needed(&mut self) {
        if !self.needs_surface_reconfigure {
            return;
        }
        let (bw, bh) = self.buffer_size();
        if let Some(vp) = &self.viewport {
            // Buffer is physical; surface (and destination) logical.
            vp.set_destination(self.logical_size.0 as i32, self.logical_size.1 as i32);
        }
        if let Some(gpu) = self.gpu.as_mut() {
            if gpu.config.width != bw || gpu.config.height != bh {
                gpu.config.width = bw;
                gpu.config.height = bh;
                gpu.surface.configure(&gpu.device, &gpu.config);
            }
        }
        self.needs_surface_reconfigure = false;
    }

    pub fn run_frame(&mut self) -> Result<(), String> {
        self.reconfigure_if_needed();
        if self.gpu.is_none() {
            return Ok(());
        }

        // --- egui pass -------------------------------------------------
        let mut raw = RawInput::default();
        raw.time = Some(self.start.elapsed().as_secs_f64());
        raw.screen_rect = Some(Rect::from_size(Vec2::new(
            self.logical_size.0 as f32,
            self.logical_size.1 as f32,
        )));
        raw.events = std::mem::take(&mut self.events);
        raw.focused = true;
        if let Some(info) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            info.native_pixels_per_point = Some(self.ppp);
            info.inner_rect = raw.screen_rect;
        }

        // egui::Context is an Arc; cloning decouples it from `&mut self`.
        let ctx = self.ctx.clone();
        let full = ctx.run(raw, |c| crate::ui::build(c, self));

        // --- actions collected during the UI pass -----------------------
        let actions = std::mem::take(&mut self.actions);
        for a in actions {
            self.do_action(a);
        }
        self.sync_preedit();

        // --- render ------------------------------------------------------
        let (bw, bh) = self.buffer_size();
        let sd = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [bw, bh],
            pixels_per_point: self.ppp,
        };
        let clipped = ctx.tessellate(full.shapes, full.pixels_per_point.max(0.1));

        let gpu = self.gpu.as_mut().ok_or("gpu gone")?;
        let renderer = self.renderer.as_mut().ok_or("renderer gone")?;
        for (id, delta) in &full.textures_delta.set {
            renderer.update_texture(&gpu.device, &gpu.queue, *id, delta);
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("hw-frame"),
            });
        let mut cmbs = renderer.update_buffers(&gpu.device, &gpu.queue, &mut encoder, &clipped, &sd);

        let current = gpu.surface.get_current_texture();
        let texture = match current {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.surface.configure(&gpu.device, &gpu.config);
                self.dirty = true;
                return Ok(());
            }
            _ => {
                self.dirty = true;
                return Ok(());
            }
        };
        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let mut pass = encoder.render_pass(&wgpu::RenderPassDescriptor {
                label: Some("hw-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::theme::clear_color(&self.cfg)),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            renderer.render(&mut pass, &clipped, &sd);
        }
        cmbs.push(encoder.finish());
        gpu.queue.submit(cmbs);
        gpu.queue.present(texture);

        for id in &full.textures_delta.free {
            renderer.free_texture(id);
        }

        // --- Wayland commit ----------------------------------------------
        if let Some(surface) = &self.surface {
            surface.damage_buffer(0, 0, bw as i32, bh as i32);
            surface.commit();
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // canvas gestures (driven from Wayland input dispatch)
    // ------------------------------------------------------------------

    pub fn on_pointer_press(&mut self, p: Pos2) {
        if !self.canvas_rect.contains(p) {
            return;
        }
        // Quick double-press clears (matches the original app's double click).
        if self
            .last_release
            .map(|t| t.elapsed() < Duration::from_millis(350))
            .unwrap_or(false)
            && (!self.ink.is_empty() || !self.candidates.is_empty())
        {
            self.clear_canvas();
            self.last_release = None;
            return;
        }
        let lp = p - self.canvas_rect.min.to_vec2();
        let mut s = Stroke::default();
        s.push(lp.x, lp.y);
        self.cur = Some(s);
    }

    pub fn on_pointer_motion(&mut self, p: Pos2) {
        if let Some(s) = self.cur.as_mut() {
            let lp = p - self.canvas_rect.min.to_vec2();
            s.push(lp.x, lp.y);
        }
    }

    pub fn on_pointer_release(&mut self, _p: Pos2) {
        if let Some(s) = self.cur.take() {
            if !s.is_empty() {
                self.ink.strokes.push(s);
            }
            self.last_release = Some(Instant::now());
            self.start_recognize();
        }
    }

    pub fn clear_canvas(&mut self) {
        self.ink.clear();
        self.candidates.clear();
        self.error = None;
        self.cur = None;
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // recognition
    // ------------------------------------------------------------------

    pub fn pre_context(&self) -> String {
        if !self.cfg.use_context {
            return String::new();
        }
        self.im
            .as_ref()
            .filter(|c| c.state.active)
            .and_then(|c| c.context_before(self.cfg.context_chars))
            .unwrap_or_default()
    }

    pub fn start_recognize(&mut self) {
        if self.ink.is_empty() {
            self.candidates.clear();
            return;
        }
        self.seq = self.seq.wrapping_add(1);
        let (w, h) = self.recognition_canvas_size();
        let ctxs = api::request_specs(&self.cfg, self.probe, self.seq, &self.pre_context());
        self.gen += 1;
        self.expected = ctxs.len();
        self.collected.clear();
        self.busy = true;
        self.error = None;
        api::spawn_requests(
            self.cfg.clone(),
            self.ink.clone(),
            w as u32,
            h as u32,
            ctxs,
            self.gen,
            self.tx.clone(),
            self.ctx.clone(),
        );
    }

    fn recognition_canvas_size(&self) -> (f32, f32) {
        let s = self.canvas_rect.size();
        if s.x > 1.0 && s.y > 1.0 {
            (s.x, s.y)
        } else {
            (self.logical_size.0 as f32, self.logical_size.1 as f32)
        }
    }

    /// Poll recognition results; must be called from the main loop.
    pub fn drain_results(&mut self) {
        let mut got = false;
        while let Ok(o) = self.rx.try_recv() {
            if o.gen != self.gen {
                continue; // stale generation
            }
            self.collected.push(o);
            got = true;
        }
        if !got || !self.busy {
            return;
        }
        if self.collected.len() < self.expected {
            return;
        }
        // Multi-language probe resolution: if the combined request failed,
        // redo the round per-language (once; cached for the session).
        if self.probe.is_none() && self.collected.len() == 1 && self.collected[0].lang == "multi" {
            if self.collected[0].result.is_err() {
                self.probe = Some(false);
                self.start_recognize();
                return;
            } else {
                self.probe = Some(true);
            }
        }
        let errs: Vec<String> = self
            .collected
            .iter()
            .filter_map(|o| o.result.as_ref().err().cloned())
            .collect();
        self.candidates = api::merge(&self.collected, self.cfg.num_candidates);
        if errs.len() == self.collected.len() && self.candidates.is_empty() {
            self.error = Some(errs.into_iter().next().unwrap_or_default());
        } else if !errs.is_empty() {
            self.error = Some(format!(
                "{} of {} languages failed ({})",
                errs.len(),
                self.collected.len(),
                errs[0]
            ));
        } else {
            self.error = None;
        }
        self.busy = false;
        self.dirty = true;
    }

    // ------------------------------------------------------------------
    // injection
    // ------------------------------------------------------------------

    fn key_delay(&self) -> Duration {
        Duration::from_millis(self.cfg.key_delay_ms)
    }

    pub fn inject_text(&mut self, text: &str) -> Result<(), String> {
        if text.is_empty() {
            return Ok(());
        }
        let mode = self.cfg.send;
        let allow_im = matches!(mode, SendMode::Auto | SendMode::InputMethod);
        let allow_vk = matches!(mode, SendMode::Auto | SendMode::Virtual);

        if allow_im {
            if let Some(ctl) = &self.im {
                if ctl.state.active && !ctl.state.unavailable {
                    ctl.commit_text(text);
                    return Ok(());
                }
            }
            if matches!(mode, SendMode::InputMethod) {
                return Err("input method is not active (no text field focused)".into());
            }
        }
        if allow_vk {
            if let Some(vk) = &mut self.vk {
                return vk.send_text(text, self.key_delay());
            }
            if matches!(mode, SendMode::Virtual) {
                return Err("compositor has no virtual keyboard".into());
            }
        }
        // clipboard fallback
        self.copy_to_clipboard(text)
    }

    pub fn inject_backspace(&mut self) -> Result<(), String> {
        let mode = self.cfg.send;
        if matches!(mode, SendMode::Auto | SendMode::InputMethod) {
            if let Some(ctl) = &self.im {
                if ctl.state.active && ctl.delete_before_caret(None) {
                    return Ok(());
                }
            }
        }
        if matches!(mode, SendMode::Auto | SendMode::Virtual) {
            if let Some(vk) = &mut self.vk {
                return vk.send_keysym(KEYSYM_BACKSPACE, self.key_delay());
            }
        }
        Err("no way to send Backspace (no virtual keyboard / input method)".into())
    }

    pub fn inject_enter(&mut self) -> Result<(), String> {
        // input-method has no keysym request; Enter is a plain key.
        if let Some(vk) = &mut self.vk {
            return vk.send_keysym(KEYSYM_RETURN, self.key_delay());
        }
        Err("Enter requires the virtual keyboard protocol".into())
    }

    pub fn copy_to_clipboard(&mut self, text: &str) -> Result<(), String> {
        match &self.clipboard {
            Some(_ctl) => {
                let ctl = self.clipboard.as_ref().unwrap();
                let payload = ctl.payload.clone();
                *payload.0.lock().unwrap() = Some(text.to_string());
                let mgr = ctl.mgr.clone();
                let device = ctl.device.clone();
                let source = mgr.create_data_source(&self.qh, ());
                device.set_selection(self.last_serial, Some(&source));
                Ok(())
            }
            None => Err("no clipboard device (ext-data-control unavailable)".into()),
        }
    }

    pub fn do_action(&mut self, action: Action) {
        match action {
            Action::CommitText { text, clear } => {
                let res = match self.cfg.send {
                    SendMode::Clipboard | SendMode::None => self.copy_to_clipboard(&text),
                    _ => self.inject_text(&text),
                };
                if let Err(e) = res {
                    self.error = Some(format!("send failed: {e}"));
                } else if clear && self.cfg.clear_after_send {
                    self.ink.clear();
                    self.candidates.clear();
                }
            }
            Action::Keysym(k) => {
                let res = if k == KEYSYM_BACKSPACE {
                    self.inject_backspace()
                } else {
                    self.inject_enter()
                };
                if let Err(e) = res {
                    self.error = Some(format!("send failed: {e}"));
                }
            }
            Action::CopyText(t) => {
                if let Err(e) = self.copy_to_clipboard(&t) {
                    self.error = Some(format!("clipboard failed: {e}"));
                }
            }
            Action::ClearCanvas => self.clear_canvas(),
        }
        self.dirty = true;
    }

    fn sync_preedit(&mut self) {
        if !self.cfg.preedit_preview {
            return;
        }
        let Some(ctl) = &self.im else { return };
        if !ctl.state.active || ctl.state.unavailable {
            return;
        }
        let want = if !self.ink.is_empty() && self.cur.is_some() {
            self.candidates.first().map(|c| c.text.clone())
        } else {
            None
        };
        if want != ctl.state.last_preedit_sent {
            match &want {
                Some(t) => ctl.set_preedit(t),
                None => ctl.clear_preedit(),
            }
            ctl.state.last_preedit_sent = want;
        }
    }

    // ------------------------------------------------------------------
    // event loop helpers
    // ------------------------------------------------------------------

    /// Poll timeout (ms) for the wayland/eventfd poll: -1 block, 0 now.
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
                    let ms = (at - now).as_millis() as i64;
                    ms.clamp(1, 60_000) as i32
                }
            }
            None => -1,
        }
    }
}

// Font loading ----------------------------------------------------------------

fn setup_fonts(ctx: &egui::Context, cfg: &Config) {
    let mut defs = egui::FontDefinitions::default();
    let mut added = false;
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(f) = &cfg.font_file {
        candidates.push(std::path::PathBuf::from(f));
    }
    if cfg.auto_cjk_font {
        for p in [
            "/run/current-system/sw/share/opentype/noto/NotoSansCJK-Regular.ttc",
            "/run/current-system/sw/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/run/current-system/sw/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
            "/run/current-system/sw/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        ] {
            candidates.push(std::path::PathBuf::from(p));
        }
    }
    for path in candidates {
        if let Ok(bytes) = std::fs::read(&path) {
            let data = egui::FontData::from_owned(bytes, 0);
            defs.font_data.insert("cjk".to_string(), std::sync::Arc::new(data));
            added = true;
            break;
        }
    }
    ctx.set_fonts(defs);
    if added {
        ctx.style_mut(|style| {
            for (_k, font_id) in style.text_styles.iter_mut() {
                font_id
                    .font_names
                    .push(egui::FontFamily::Name("cjk".into()));
            }
        });
    }
}
