//! Opens one real window, receives the ready callback, and exits on the
//! first loop iteration — the smallest end-to-end proof the seam works —
//! then proves a second loop in the same process is refused recoverably.
//!
//! Own harness (`harness = false`): the OS event loop must run on the
//! main thread, which the default test harness cannot provide. Where no
//! display server exists (headless CI), loop creation fails recoverably
//! and the windowed half of this test SKIPS with a printed reason —
//! honest about what a windowing layer can prove headless. The
//! second-loop half runs either way: the refusal is decided before any
//! display is touched. Known bound: an INCOHERENT display stack
//! (display present but X11 runtime libraries missing) panics below the
//! windowing seam's error path instead — a virtual display needs its
//! keyboard runtime installed too. Real windowed CI coverage arrives
//! with the rendering test infrastructure.

use std::process::ExitCode;

use raw_window_handle::{HasDisplayHandle, HasWindowHandle};
use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowError, WindowEvent, WindowRef, run_window_app,
};

struct SmokeApp {
    ready_fired: bool,
    updates: u32,
    redraws_seen: u32,
    physical_size: (u32, u32),
    losses: u32,
}

impl SmokeApp {
    fn new() -> Self {
        Self {
            ready_fired: false,
            updates: 0,
            redraws_seen: 0,
            physical_size: (0, 0),
            losses: 0,
        }
    }
}

impl WindowApp for SmokeApp {
    fn ready(&mut self, window: &WindowRef<'_>) {
        self.ready_fired = true;
        self.physical_size = window.physical_size();
        assert!(window.scale_factor() > 0.0);
        // The renderer's view of the window: an owned handle that keeps
        // the OS window alive and answers both handle traits, and whose
        // clones name the same window.
        let native = window.native();
        let shared = native.clone();
        assert!(
            native.display_handle().is_ok(),
            "the live window must yield a display handle"
        );
        let handle = native.window_handle().ok();
        let shared_handle = shared.window_handle().ok();
        assert!(
            handle.is_some(),
            "the live window must yield a window handle"
        );
        assert_eq!(
            handle, shared_handle,
            "clones of a native window must name the same OS window"
        );
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

    fn surface_lost(&mut self) {
        self.losses += 1;
    }
}

/// The windowed half: run one loop to completion where a display exists.
fn one_window_runs_and_exits(config: &WindowConfig) -> bool {
    let mut app = SmokeApp::new();
    match run_window_app(config, &mut app) {
        Ok(()) => {
            assert!(app.ready_fired, "ready must fire before the loop turns");
            assert!(app.updates >= 3, "update must run each iteration");
            assert!(
                app.physical_size.0 > 0 && app.physical_size.1 > 0,
                "the window must have a real size at ready"
            );
            // Desktop platforms grant exactly one surface epoch: a run
            // that ends by the app's own request must never have been
            // told its surface was lost. This is the claim the whole
            // epoch model rests on for desktop, pinned where a real
            // window exists to disprove it.
            assert_eq!(
                app.losses, 0,
                "a desktop run must close no surface epoch mid-flight"
            );
            println!(
                "window smoke: ok (size {:?}, redraws delivered: {})",
                app.physical_size, app.redraws_seen
            );
            true
        }
        Err(WindowError::LoopUnavailable { message }) => {
            // Headless environment: skipping is the honest outcome.
            println!("window smoke: SKIPPED (loop unavailable: {message})");
            true
        }
        Err(error) => {
            eprintln!("window smoke: FAILED: {error}");
            false
        }
    }
}

/// The event loop is a process-wide singleton. A second attempt — after
/// a loop has run to completion, or after a headless first attempt
/// failed — must come back as the recoverable `LoopUnavailable`, with no
/// callback delivered, never as a panic. This half is display-independent.
fn a_second_loop_is_refused_recoverably(config: &WindowConfig) -> bool {
    let mut app = SmokeApp::new();
    match run_window_app(config, &mut app) {
        Err(WindowError::LoopUnavailable { message }) => {
            assert!(
                !app.ready_fired && app.updates == 0,
                "a refused loop must deliver no callbacks"
            );
            println!("window smoke: second loop refused ({message})");
            true
        }
        Ok(()) => {
            eprintln!("window smoke: FAILED: a second event loop ran in the same process");
            false
        }
        Err(error) => {
            eprintln!("window smoke: FAILED: second attempt reported {error}");
            false
        }
    }
}

fn main() -> ExitCode {
    let config = WindowConfig {
        title: "renew-smoke".to_string(),
        logical_width: 320.0,
        logical_height: 200.0,
        resizable: false,
        ..WindowConfig::default()
    };
    if one_window_runs_and_exits(&config) && a_second_loop_is_refused_recoverably(&config) {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
