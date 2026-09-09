//! The iOS doorway: the OS enters through `main`, as the desktop does,
//! and then never comes back.
//!
//! Unlike Android there is no separate entry symbol to export — an iOS
//! application's executable starts at `main` like any other program, and
//! the difference is what happens next: the event loop enters
//! `UIApplicationMain`, which owns the process from that point on. So
//! this doorway is a function `main` hands off to and does not expect a
//! return from.
//!
//! A phone has no terminal anyone watches, so the sample's visible half
//! moves, exactly as it does on Android: everything the desktop prints,
//! this doorway appends to a log inside the app's sandbox, read back
//! from the host with `xcrun simctl get_app_container`. The log is the
//! sample's report on this platform — ready, every event as its one-line
//! description, and each surface epoch as it opens or closes.

use std::path::PathBuf;

use renew_platform::window::{
    LoopControl, WindowApp, WindowConfig, WindowEvent, WindowRef, run_window_app,
};

use crate::app::{EchoApp, describe};
use crate::cli::Options;

/// Where an iOS app writes its report.
///
/// The sandbox root arrives in `HOME`, and the whole container is read
/// back from the host with `get_app_container`; `Documents` is simply
/// where a document goes. A launch without `HOME`
/// falls back to the temporary directory rather than going mute — the
/// same rule the diagnostics sink follows.
///
/// Returns a path rather than an `Option`, because it always has one:
/// the Android doorway's equivalent is optional for a real reason (the
/// activity may genuinely have no data path), and copying its shape
/// here would be a signature that cannot fail claiming it can.
fn log_path() -> PathBuf {
    std::env::var_os("HOME").map_or_else(
        || std::env::temp_dir().join("input_echo.log"),
        |home| PathBuf::from(home).join("Documents").join("input_echo.log"),
    )
}

/// The application's entry, called by `main` on this platform.
///
/// **Does not return.** `run_window_app` enters `UIApplicationMain`, and
/// the process ends when the system ends it. The outcome line every
/// other doorway writes at the end has no counterpart here, and saying
/// so is the point: a reader looking for it should know it is absent by
/// design rather than missing by accident.
pub fn ios_main() -> ! {
    let log = log_path();

    // The panic hook and the diagnostics channel both go to the same
    // file, before anything can fail.
    let _ = renew_platform::diag::log_to_file(Some(log.clone()), Some("input_echo: ios start"));

    let options = Options {
        // A phone session ends when the OS ends it, not on a tick
        // budget: an app that quit itself after ten seconds would read
        // as a crash.
        frames: u64::MAX,
        ..Options::default()
    };
    let mut app = Filed {
        echo: EchoApp::new(&options),
        log,
    };
    let config = WindowConfig {
        title: "renew — input echo".to_string(),
        logical_width: 640.0,
        logical_height: 360.0,
        resizable: true,
        ..WindowConfig::default()
    };

    // A failure here is a loop that never started, which is the only
    // outcome this call can report: everything after a successful start
    // belongs to `UIApplicationMain`.
    if let Err(error) = run_window_app(&config, &mut app) {
        app.note(&format!("loop never started: {error}"));
    }

    // Reached when the loop refused to start, and in principle if it
    // ever returned cleanly — which `run_window_app` documents as
    // impossible on this platform, the loop having entered
    // `UIApplicationMain`. Either way the process has nothing left to do
    // and no OS to hand back to, so it exits rather than falling out of
    // a function the platform expects never to return from.
    std::process::exit(1)
}

/// The library's app, with the desktop's console replaced by the log.
///
/// A doorway-local wrapper rather than a library change: what varies per
/// platform is where the words go, never what the sample does. The
/// Android doorway carries the same shape for the same reason.
struct Filed {
    echo: EchoApp,
    log: PathBuf,
}

impl Filed {
    /// One line into the log, best effort: a line that cannot be written
    /// has nowhere to report that, and must not become the fault it was
    /// describing.
    fn note(&self, line: &str) {
        let _ = renew_platform::fs::append(&self.log, format!("{line}\n").as_bytes());
    }
}

impl WindowApp for Filed {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.note(&format!(
            "ready: {width}x{height} at scale {}",
            window.scale_factor()
        ));
        self.echo.ready(window);
    }

    /// **The line this platform exists to produce.**
    ///
    /// Android revokes the window when an app is backgrounded and this
    /// fires there on every cycle. iOS is believed not to — the surface
    /// survives a resign-active — so on this platform the expected count
    /// is zero, and any line here is the finding.
    fn surface_lost(&mut self) {
        self.note("surface lost: epoch closed, awaiting the next ready");
        self.echo.surface_lost();
    }

    /// **The two lines that make this platform observable.**
    ///
    /// iOS suspends an application without taking its window, so
    /// `surface_lost` never fires here and an absence of it proves
    /// nothing on its own: a run that was never backgrounded looks
    /// exactly the same. These say that the suspend happened, which is
    /// what turns "no surface was lost" from an absence into a finding.
    fn suspended(&mut self) {
        self.note("suspended: the app is in the background");
    }

    fn resumed(&mut self) {
        self.note("resumed: the app is in the foreground again");
    }

    fn event(&mut self, event: WindowEvent) {
        self.note(&describe(event));
        self.echo.event(event);
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.echo.update(control);
    }
}
