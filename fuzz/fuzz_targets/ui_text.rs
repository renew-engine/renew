//! The document grammar, against text nobody wrote on purpose.
//!
//! The compiler takes source text and hands back either a compiled
//! document or a diagnostic. Both are answers; a panic, a hang, or a
//! runaway recursion are not, and this target exists to look for
//! those.
//!
//! Two claims ride along. The compiler promises it only mints what
//! the runtime reader accepts — so an accepted compile is read back,
//! and a refusal there is a finding, not a corpus quirk. And the
//! reader's own promise holds transitively: the read document is
//! instantiated too.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    let Ok(text) = core::str::from_utf8(data) else {
        return;
    };
    if let Ok(compiled) = renew_cli::ui_compile::compile(text) {
        let document = renew_ui::Document::read(&compiled.bytes)
            .expect("the compiler must only mint what the reader accepts");
        let _ = document.tree();
    }
});
