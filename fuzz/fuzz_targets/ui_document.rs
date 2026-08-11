//! The UI document reader, against bytes nobody wrote on purpose.
//!
//! The reader takes a blob from disk and hands back either a document
//! or a refusal. Both are answers; a panic, a hang, or a read past the
//! end are not, and this target exists to look for those.
//!
//! One step further than the other targets: when the bytes DO read as
//! a document, the tree is instantiated too. Validation claims the
//! forward-parent proof makes instantiation infallible — the assert
//! inside `tree` is that claim, and an input that breaks it is
//! exactly the finding this harness is for.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(document) = renew_ui::Document::read(data) {
        let _ = document.tree();
    }
});
