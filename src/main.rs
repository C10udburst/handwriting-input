//! Entry point: config → Wayland connect → roundtrip → GPU init → egui event loop.

mod api;
mod app;
mod config;
mod ink;
mod inject;
mod theme;
mod ui;
mod wayland;

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::time::Instant;

use clap::Parser;
use wayland_client::Connection;

use app::App;

/// Wake-up pipe (eventfd) written by egui's repaint callback (any thread).
#[derive(Clone)]
struct Wake(File);

impl Wake {
    fn new() -> std::io::Result<Wake> {
        let fd = unsafe {
            libc::eventfd(0, libc::EFD_CLOEXEC | libc::EFD_NONBLOCK)
        };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(Wake(unsafe { File::from_raw_fd(fd) }))
    }
    fn notify(&self) {
        let mut f = &self.0;
        let _ = f.write_all(&1u64.to_ne_bytes());
    }
    fn drain(&self) {
        let mut buf = [0u8; 16];
        let mut f = &self.0;
        while f.read(&mut buf).unwrap_or(0) > 0 {}
    }
    fn fd(&self) -> i32 {
        self.0.as_raw_fd()
    }
}

fn main() -> std::process::ExitCode {
    let cli = config::Cli::parse();
    let cfg = match config::Config::load(&cli.config) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("config error: {e}");
            return std::process::ExitCode::FAILURE;
        }
    };
    match run(cfg) {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("handwriting-input: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn run(cfg: config::Config) -> Result<(), String> {
    let conn = Connection::connect_to_env()
        .map_err(|e| format!("cannot connect to a Wayland compositor: {e}"))?;
    let mut queue = conn.new_event_queue::<App>();
    let qh = queue.handle();

    let mut app = App::new(cfg, conn.clone(), qh);
    queue
        .roundtrip(&mut app)
        .map_err(|e| format!("wayland roundtrip: {e}"))?;
    app.after_globals()?;

    // Wait for the first layer-surface configure (it tells us the size).
    while !app.configured && !app.quit {
        queue
            .blocking_dispatch(&mut app)
            .map_err(|e| format!("wayland: {e}"))?;
    }
    if app.quit {
        return Ok(());
    }

    app.init_gpu()?;

    // Repaint wake-ups (egui, recognition results, timers) go through an eventfd.
    let wake = Wake::new().map_err(|e| format!("eventfd: {e}"))?;
    let wake_cb = wake.clone();
    app.ctx.set_request_repaint_callback(move |info| {
        *app.repaint_sched.lock().unwrap() = Some(Instant::now() + info.delay);
        wake_cb.notify();
    });

    loop {
        let _ = queue.flush();
        let _ = queue.dispatch_pending(&mut app);

        app.drain_results();
        if app.dirty {
            app.dirty = false;
            app.run_frame()?;
            let _ = queue.flush();
        }
        if app.quit {
            return Ok(());
        }

        let timeout = app.poll_timeout_ms();
        if timeout == 0 {
            continue;
        }

        // Arm the socket read *before* polling so no event can slip through.
        let read_guard = queue.prepare_read();
        let mut fds = [
            libc::pollfd {
                fd: wake.fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: match &read_guard {
                    Some(g) => g.connection_fd().as_raw_fd(),
                    None => -1,
                }, // must be armed before polling
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let r = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as u32, timeout) };
        if r < 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(libc::EINTR) {
                drop(read_guard);
                continue;
            }
            return Err(format!("poll: {err}"));
        }
        if fds[0].revents != 0 {
            wake.drain();
        }
        if let Some(g) = read_guard {
            if fds[1].revents != 0 {
                g.read().map_err(|e| format!("wayland read: {e}"))?;
            }
        }
    }
}

