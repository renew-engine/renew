//! The Android doorway: the OS enters through `android_main`, exactly
//! as the desktop enters through `main` — one doorway per way a
//! process starts, with every line of behaviour in the library between
//! them.
//!
//! A phone has no terminal anyone watches, so the sample's visible
//! half moves: everything the desktop prints, this doorway appends to
//! a log in the activity's internal storage, pulled with `adb`. The
//! log is the sample's report on this platform — ready, every event as
//! its one-line description, each surface epoch closing as the app is
//! backgrounded, and the loop's outcome at the end if one comes.

use std::path::PathBuf;

use renew_platform::window::android::{AndroidApp, run_window_app_android};
use renew_platform::window::{LoopControl, WindowApp, WindowConfig, WindowEvent, WindowRef};

use crate::app::{EchoApp, describe};
use crate::cli::Options;

/// The activity's entry, called by the OS glue once the process is up.
///
/// The unmangled name is the contract: the activity loads the sample's
/// shared library and looks for exactly this symbol.
// The one place in the sample the workspace's no-unsafe rule bends:
// an OS entry point is found by name, and a mangled name is a library
// the activity loads and cannot enter. The expect sits on the item
// because that is the narrowest scope the language offers — it covers
// the body too, so an unsafe block added there would ride under it
// silently; today the body carries none and the attribute alone
// fulfils the expectation.
#[expect(
    unsafe_code,
    reason = "the OS glue finds the entry by its unmangled name"
)]
#[unsafe(no_mangle)]
extern "Rust" fn android_main(activity: AndroidApp) {
    let log = activity
        .internal_data_path()
        .map(|dir| dir.join("input_echo.log"));

    // The panic hook and the diagnostics channel both go to the same
    // file. A refused sink leaves the run mute rather than dead — the
    // sink's own contract — and the doorway's appends below answer for
    // themselves per line.
    let _ = renew_platform::diag::log_to_file(log.clone(), Some("input_echo: android start"));

    let options = Options {
        // A phone session ends when the OS ends it, not on a tick
        // budget: the desktop default exists so a headless run halts,
        // and a launcher icon that quit itself after ten seconds would
        // read as a crash.
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
    let outcome = run_window_app_android(activity, &config, &mut app);
    app.note(&match outcome {
        Ok(()) => format!("loop ended: {}", app.echo.report().digest_line()),
        Err(error) => format!("loop failed: {error}"),
    });
}

/// The library's app, with the desktop's console replaced by the log.
///
/// A doorway-local wrapper rather than a library change: what varies
/// per platform is where the words go, never what the sample does.
struct Filed {
    echo: EchoApp,
    log: Option<PathBuf>,
}

impl Filed {
    /// One line into the log, best effort: a line that cannot be
    /// written has nowhere to report that, and must not become the
    /// fault it was describing.
    fn note(&self, line: &str) {
        if let Some(path) = &self.log {
            let _ = renew_platform::fs::append(path, format!("{line}\n").as_bytes());
        }
    }
}

impl WindowApp for Filed {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.note(&format!(
            "ready: {width}x{height} at scale {scale}",
            scale = window.scale_factor()
        ));
        self.echo.ready(window);
    }

    fn event(&mut self, event: WindowEvent) {
        self.note(&describe(event));
        self.echo.event(event);
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.echo.update(control);
    }

    fn surface_lost(&mut self) {
        // The epoch is closing under us; the library app holds no
        // window-derived values (its defaulted release is the honest
        // one), so the doorway's whole duty is to say it happened —
        // this line in a pulled log is the backgrounding evidence.
        self.note("surface lost: epoch closed, awaiting the next ready");
        self.echo.surface_lost();
    }

    /// Android reports both: it backgrounds the app *and* takes the
    /// window. Logging them separately is what lets the two platforms'
    /// logs be read side by side — the pair here, only this pair on
    /// iOS, where the window survives.
    fn suspended(&mut self) {
        self.note("suspended: the app is in the background");
    }

    fn resumed(&mut self) {
        self.note("resumed: the app is in the foreground again");
    }
}
