//! The iOS doorway: the OS enters through `main`, and the loop it hands
//! off to never gives control back.
//!
//! **This is the presentation path, which is the point of it.** The
//! headless run draws into an image and never creates a surface, so a
//! lane built on it proves the renderer works and says nothing about
//! whether this platform can present. Here a window exists, a swapchain
//! is built on it, and every frame is acquired and presented — the code
//! that was compiled for this target from the first mobile lane onward
//! and exercised by nothing until now.
//!
//! A phone has no terminal anyone watches, so the sample's report goes
//! to a log inside the app's sandbox, read back from the host. The
//! picture itself is read a different way: the simulator's screen can be
//! captured, which is the only way to see that something reached it.

use std::path::PathBuf;

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowEvent, WindowRef, run_window_app,
};

use crate::cli::Options;
use crate::windowed::TriangleApp;

/// Where an iOS app writes its report.
///
/// The sandbox root arrives in `HOME`, and the whole container is read
/// back from the host with `get_app_container`. A launch without `HOME`
/// falls back to the temporary directory rather than going mute.
fn log_path() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || std::env::temp_dir().join("hello_triangle.log"),
        |home| {
            PathBuf::from(home)
                .join("Documents")
                .join("hello_triangle.log")
        },
    )
}

/// The application's entry, called by `main` on this platform.
///
/// **Does not return.** The loop enters `UIApplicationMain`, which owns
/// the process from that point on, so the sample's own verdict — the
/// line the Android doorway writes when the loop ends — has no
/// counterpart here. What a reader gets instead is the log up to the
/// moment the system stopped the app, and the screen itself.
pub fn ios_main() -> ! {
    let log = log_path();
    let _ = renew_platform::diag::log_to_file(Some(log.clone()), Some("hello_triangle: ios start"));

    let options = Options {
        // The system ends the session, not a frame budget: an app that
        // quit itself after a few hundred frames would be gone before
        // anything could look at it.
        frames: u64::MAX,
        ..Options::default()
    };
    let mut app = Logged {
        triangle: TriangleApp::new(&options),
        log,
    };
    let config = WindowConfig {
        title: "renew — hello triangle".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
    };

    if let Err(error) = run_window_app(&config, &mut app) {
        app.note(&format!("loop never started: {error}"));
    }

    // Reached when the loop refused to start, and in principle on a
    // clean return, which this platform documents as impossible. Either
    // way there is no OS to hand back to.
    std::process::exit(1)
}

/// The sample's app, with the desktop's console replaced by the log.
struct Logged {
    triangle: TriangleApp,
    log: PathBuf,
}

impl Logged {
    /// One line into the log, best effort: a line that cannot be written
    /// has nowhere to report that.
    fn note(&self, line: &str) {
        let _ = renew_platform::fs::append(&self.log, format!("{line}\n").as_bytes());
    }
}

impl WindowApp for Logged {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.note(&format!("ready: {width}x{height}"));
        self.triangle.ready(window);
    }

    fn event(&mut self, event: WindowEvent) {
        self.triangle.event(event);
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.triangle.update(control);
    }

    fn surface_lost(&mut self) {
        self.note("surface lost: epoch closed, awaiting the next ready");
        self.triangle.surface_lost();
    }

    fn suspended(&mut self) {
        self.note("suspended: the app is in the background");
    }

    fn resumed(&mut self) {
        self.note("resumed: the app is in the foreground again");
    }
}
