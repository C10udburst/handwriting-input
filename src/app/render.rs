//! GPU state, surface configuration, and the per-frame egui → wgpu → Wayland path.

use std::ffi::c_void;
use std::ptr::NonNull;

use egui::{Pos2, RawInput, Rect, Vec2};
use wayland_client::Proxy;

use crate::app::App;

/// Everything GPU-side for our single surface.
///
/// `instance` and `format` are stored but never read: the surface borrows the
/// instance and the renderer was built against that format.
#[allow(dead_code)]
pub struct Gpu {
    pub instance: wgpu::Instance,
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
    pub config: wgpu::SurfaceConfiguration,
    pub renderer: egui_wgpu::Renderer,
}

impl Gpu {
    /// Vulkan first, GL (EGL) second; both present onto the Wayland surface.
    pub fn new(
        display_ptr: *mut c_void,
        surface_ptr: *mut c_void,
        width: u32,
        height: u32,
    ) -> Result<Gpu, String> {
        use wgpu::rwh::{
            RawDisplayHandle, RawWindowHandle, WaylandDisplayHandle, WaylandWindowHandle,
        };

        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::VULKAN | wgpu::Backends::GL;
        let instance = wgpu::Instance::new(desc);

        let surface = unsafe {
            instance.create_surface_unsafe(wgpu::SurfaceTargetUnsafe::RawHandle {
                raw_display_handle: NonNull::new(display_ptr).map(|p| {
                    RawDisplayHandle::Wayland(WaylandDisplayHandle::new(p))
                }),
                raw_window_handle: NonNull::new(surface_ptr)
                    .map(|p| RawWindowHandle::Wayland(WaylandWindowHandle::new(p)))
                    .ok_or("wl_surface pointer is null")?,
            })
        }
        .map_err(|e| format!("wgpu surface creation failed: {e}"))?;

        let options = wgpu::RequestAdapterOptions {
            compatible_surface: Some(&surface),
            ..Default::default()
        };
        let adapter = pollster::block_on(instance.request_adapter(&options))
            .map_err(|e| format!("no usable GPU adapter: {e}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("handwriting-input"),
            required_features: wgpu::Features::empty(),
            required_limits: adapter.limits(),
            ..Default::default()
        }))
        .map_err(|e| format!("GPU device request failed: {e}"))?;

        let mut config = surface
            .get_default_config(&adapter, width.max(1), height.max(1))
            .ok_or("compositor surface has no usable configuration")?;
        config.present_mode = wgpu::PresentMode::AutoVsync;
        surface.configure(&device, &config);

        let format = config.format;
        let renderer = egui_wgpu::Renderer::new(&device, format, egui_wgpu::RendererOptions::default());

        Ok(Gpu {
            instance,
            surface,
            device,
            queue,
            format,
            config,
            renderer,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        if self.config.width != width || self.config.height != height {
            self.config.width = width;
            self.config.height = height;
            self.surface.configure(&self.device, &self.config);
        }
    }
}

impl App {
    /// Buffer (pixel) size for the current logical size and scale factor.
    pub fn buffer_size(&self) -> (u32, u32) {
        let (lw, lh) = self.logical_size;
        let bw = (lw as f32 * self.ppp).round().max(1.0) as u32;
        let bh = (lh as f32 * self.ppp).round().max(1.0) as u32;
        (bw.max(1), bh.max(1))
    }

    /// Build the GPU state. Must run after the first layer configure.
    pub fn init_gpu(&mut self) -> Result<(), String> {
        let surface = self.surface.clone().ok_or("no Wayland surface")?;
        let surface_ptr = surface.id().as_ptr();
        let display_ptr = self.conn.backend().display_ptr();
        let (bw, bh) = self.buffer_size();
        self.gpu = Some(Gpu::new(display_ptr.cast(), surface_ptr.cast(), bw, bh)?);
        Ok(())
    }

    fn prepare_surface(&mut self) {
        if let Some(vp) = self.viewport.clone() {
            // Buffer holds pixels; the surface destination is in logical units.
            vp.set_destination(self.logical_size.0 as i32, self.logical_size.1 as i32);
        }
        let (bw, bh) = self.buffer_size();
        if let Some(gpu) = self.gpu.as_mut() {
            gpu.resize(bw, bh);
        }
        self.needs_surface_reconfigure = false;
    }

    /// One egui pass + wgpu submit + Wayland commit.
    pub fn run_frame(&mut self) -> Result<(), String> {
        if self.gpu.is_none() {
            return Ok(());
        }
        self.prepare_surface();

        let (lw, lh) = self.logical_size;
        let mut raw = RawInput::default();
        raw.time = Some(self.start.elapsed().as_secs_f64());
        raw.screen_rect = Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(lw as f32, lh as f32)));
        raw.events = std::mem::take(&mut self.events);
        raw.focused = true;
        if let Some(info) = raw.viewports.get_mut(&egui::ViewportId::ROOT) {
            info.native_pixels_per_point = Some(self.ppp);
            info.inner_rect = raw.screen_rect;
        }

        // `egui::Context` is an Arc; cloning decouples it from `&mut self`.
        let ctx = self.ctx.clone();
        let full = ctx.run_ui(raw, |ui| crate::ui::build(ui, self));

        let actions = std::mem::take(&mut self.actions);
        for action in actions {
            self.do_action(action);
        }
        self.sync_preedit();

        self.render(full)?;

        if let Some(surface) = self.surface.clone() {
            let (bw, bh) = self.buffer_size();
            surface.damage_buffer(0, 0, bw as i32, bh as i32);
            surface.commit();
        }
        Ok(())
    }

    fn render(&mut self, full: egui::FullOutput) -> Result<(), String> {
        let (bw, bh) = self.buffer_size();
        let sd = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [bw, bh],
            pixels_per_point: full.pixels_per_point.max(0.1),
        };
        let clipped = self.ctx.tessellate(full.shapes, full.pixels_per_point.max(0.1));

        let gpu = self.gpu.as_mut().ok_or("gpu missing")?;
        for (id, deltas) in &full.textures_delta.set {
            for delta in deltas {
                gpu.renderer
                    .update_texture(&gpu.device, &gpu.queue, *id, delta);
            }
        }

        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("handwriting-frame"),
            });
        let user_buffers =
            gpu.renderer
                .update_buffers(&gpu.device, &gpu.queue, &mut encoder, &clipped, &sd);

        let texture = match gpu.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => t,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                gpu.resize(bw, bh);
                self.dirty = true;
                return Ok(());
            }
            // Timeout / Occluded: skip this frame, try again soon.
            _ => {
                self.dirty = true;
                return Ok(());
            }
        };

        let view = texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("handwriting-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(crate::theme::clear_color(&self.cfg)),
                        store: wgpu::StoreOp::Store,
                    },
                    depth_slice: None,
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            gpu.renderer
                .render(&mut pass.forget_lifetime(), &clipped, &sd);
        }
        let mut buffers = user_buffers;
        buffers.push(encoder.finish());
        gpu.queue.submit(buffers);
        // `present` is infallible here: surface problems surface as Outdated /
        // Lost on the next `get_current_texture`, which we handle above.
        gpu.queue.present(texture);

        for id in &full.textures_delta.free {
            gpu.renderer.free_texture(id);
        }
        Ok(())
    }
}
