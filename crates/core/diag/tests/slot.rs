//! Install-then-emit: records reach the sink with the right level, target,
//! and formatted message. Own process: the slot is written exactly once.

use std::sync::Mutex;

use renew_diag::{Level, Record, Sink};

struct Capture {
    seen: Mutex<Vec<(Level, String, String)>>,
}

impl Sink for Capture {
    fn write(&self, record: &Record<'_>) {
        // Allocation is fine here: the zero-allocation contract binds the
        // crate's emit path, not what a sink chooses to do. Fallible lock
        // on purpose: trait impls are not #[test] fns, so test lint
        // relaxations do not reach them.
        if let Ok(mut seen) = self.seen.lock() {
            seen.push((
                record.level(),
                record.target().to_string(),
                format!("{}", record.message()),
            ));
        }
    }
}

#[test]
fn every_macro_reaches_the_installed_sink_intact() {
    let sink: &'static Capture = Box::leak(Box::new(Capture {
        seen: Mutex::new(Vec::new()),
    }));
    renew_diag::install(sink);

    // Every level macro, both arms where it matters, plus the common
    // back end called directly — no exported macro goes unexpanded.
    renew_diag::error!(target: "explicit", "boom: {}", "now");
    renew_diag::warn!("watch {}", "out");
    renew_diag::info!("hello {}", 42);
    renew_diag::debug!(target: "inner", "state: {:?}", (1, 2));
    renew_diag::trace!("fine detail");
    renew_diag::log!(Level::Warn, "direct back end");

    let seen = sink.seen.lock().expect("capture lock");
    let expected: Vec<(Level, String, String)> = vec![
        (Level::Error, "explicit".into(), "boom: now".into()),
        // Default target is the caller's module path — for an integration
        // test binary that is the test crate's own name.
        (Level::Warn, "slot".into(), "watch out".into()),
        (Level::Info, "slot".into(), "hello 42".into()),
        (Level::Debug, "inner".into(), "state: (1, 2)".into()),
        (Level::Trace, "slot".into(), "fine detail".into()),
        (Level::Warn, "slot".into(), "direct back end".into()),
    ];
    assert_eq!(*seen, expected);
}
