//! Opens one real window, receives the ready callback, and exits on the
//! first loop iteration — the smallest end-to-end proof the seam works.
//!
//! Own harness (`harness = false`): the OS event loop must run on the
//! main thread, which the default test harness cannot provide. Where no
//! display server exists (headless CI), loop creation fails recoverably
//! and this test SKIPS with a printed reason — honest about what a
//! windowing layer can prove headless. Real windowed CI coverage
//! arrives with the rendering test infrastructure.

use std::process::ExitCode;

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};

struct SmokeApp {
    ready_fired: bool,
    updates: u32,
    redraws_seen: u32,
    physical_size: (u32, u32),
}

impl WindowApp for SmokeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        self.ready_fired = true;
        self.physical_size = window.physical_size();
        assert!(window.scale_factor() > 0.0);
    }

    fn event(&mut self, event: WindowEvent) {
        if event == WindowEvent::RedrawRequested {
            self.redraws_seen += 1;
        }
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.updates += 1;
        if self.updates == 1 {
            // Exercise the redraw-request path; delivery timing is the
            // OS's business, so it is reported, not hard-asserted.
            control.request_redraw();
        }
        if self.updates >= 3 {
            control.exit();
        }
    }
}

fn main() -> ExitCode {
    let config = WindowConfig {
        title: "renew-smoke".to_string(),
        logical_width: 320.0,
        logical_height: 200.0,
        resizable: false,
    };
    let mut app = SmokeApp {
        ready_fired: false,
        updates: 0,
        redraws_seen: 0,
        physical_size: (0, 0),
    };
    match run_window_app(&config, &mut app) {
        Ok(()) => {
            assert!(app.ready_fired, "ready must fire before the loop turns");
            assert!(app.updates >= 3, "update must run each iteration");
            assert!(
                app.physical_size.0 > 0 && app.physical_size.1 > 0,
                "the window must have a real size at ready"
            );
            println!(
                "window smoke: ok (size {:?}, redraws delivered: {})",
                app.physical_size, app.redraws_seen
            );
            ExitCode::SUCCESS
        }
        Err(WindowError::LoopUnavailable { message }) => {
            // Headless environment: skipping is the honest outcome.
            println!("window smoke: SKIPPED (loop unavailable: {message})");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("window smoke: FAILED: {error}");
            ExitCode::FAILURE
        }
    }
}
