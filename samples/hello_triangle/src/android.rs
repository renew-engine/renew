//! The Android doorway: the OS enters through `android_main`, exactly
//! as the desktop enters through `main`.
//!
//! What the triangle proves here is not the triangle. It is that the
//! renderer survives a platform that takes windows away: this sample
//! owns a window target, so backgrounding drops the whole chain and the
//! next foreground rebuilds it against a new window and the device that
//! outlived both. The log in the activity's internal storage is where
//! that story is readable, because a phone shows no console.

use std::path::PathBuf;

use renew_platform::window::android::{AndroidApp, run_window_app_android};
use renew_platform::window::{LoopControl, WindowApp, WindowConfig, WindowEvent, WindowRef};

use crate::cli::Options;
use crate::windowed::TriangleApp;

/// The activity's entry, called by the OS glue once the process is up.
///
/// The unmangled name is the contract: the activity loads the sample's
/// shared library and looks for exactly this symbol. The expect sits on
/// the item because that is the narrowest scope the language offers —
/// it covers the body too, which carries no unsafe of its own.
#[expect(
    unsafe_code,
    reason = "the OS glue finds the entry by its unmangled name"
)]
#[unsafe(no_mangle)]
extern "Rust" fn android_main(activity: AndroidApp) {
    let log = activity
        .internal_data_path()
        .map(|dir| dir.join("hello_triangle.log"));
    let _ = renew_platform::diag::log_to_file(log.clone(), Some("hello_triangle: android start"));

    let options = Options {
        // A phone session ends when the OS ends it. The desktop default
        // bounds an unattended run; a launcher icon that quit itself
        // after a few hundred frames would read as a crash.
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
        ..WindowConfig::default()
    };
    let outcome = run_window_app_android(activity, &config, &mut app);
    // Through the sample's own verdict rather than around it: `finish`
    // is where a refused adapter, a failed bring-up and a wedge become
    // a sentence, and it is also the documented teardown order. A
    // doorway that logged "loop ended" would throw away the one line
    // worth pulling off a device. It consumes the app, so the log path
    // is taken first — the sink outlives the sample by design.
    let Logged { triangle, log } = app;
    let note = match triangle.finish(outcome) {
        Ok(report) => format!("ended: {}", report.digest_line()),
        Err(error) => format!("ended badly: {error}"),
    };
    if let Some(path) = &log {
        let _ = renew_platform::fs::append(
            path,
            format!(
                "{note}
"
            )
            .as_bytes(),
        );
    }
}

/// The sample's app with the desktop's console replaced by the log.
struct Logged {
    triangle: TriangleApp,
    log: Option<PathBuf>,
}

impl Logged {
    /// One line into the log, best effort: a line that cannot be
    /// written has nowhere to report that.
    fn note(&self, line: &str) {
        if let Some(path) = &self.log {
            let _ = renew_platform::fs::append(path, format!("{line}\n").as_bytes());
        }
    }
}

impl WindowApp for Logged {
    fn ready(&mut self, window: &WindowRef<'_>) {
        let (width, height) = window.physical_size();
        self.note(&format!(
            "ready: {width}x{height}, bringing the renderer up"
        ));
        self.triangle.ready(window);
    }

    fn event(&mut self, event: WindowEvent) {
        // Redraws arrive at display rate; logging each would fill the
        // storage this sample writes to. Everything else is rare and
        // worth a line.
        if event != WindowEvent::RedrawRequested {
            self.note(&format!("{event:?}"));
        }
        self.triangle.event(event);
    }

    fn update(&mut self, control: &mut LoopControl) {
        self.triangle.update(control);
    }

    fn surface_lost(&mut self) {
        self.note("surface lost: dropping the target, keeping the device");
        self.triangle.surface_lost();
    }
}
