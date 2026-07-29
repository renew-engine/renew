//! Installing a second sink is a contract violation, fatal in dev builds.
//! Own process: it must own the slot's one write and its one violation.

use renew_diag::{Record, Sink};

struct Quiet;

impl Sink for Quiet {
    fn write(&self, _record: &Record<'_>) {}
}

// The violation is a debug assertion, so this test is meaningful only in
// profiles that keep debug assertions on (every canonical command does).
#[cfg(debug_assertions)]
#[test]
#[should_panic(expected = "installed twice")]
fn second_install_is_a_contract_violation() {
    static FIRST: Quiet = Quiet;
    static SECOND: Quiet = Quiet;
    renew_diag::install(&FIRST);
    renew_diag::install(&SECOND);
}
