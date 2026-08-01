//! The trace codec, against text nobody wrote on purpose.
//!
//! `parse` reads a recorded input trace — a file the engine did not write
//! and must not trust. It answers with a trace or a refusal naming a
//! line; anything else is a defect.
//!
//! Non-UTF-8 input is skipped rather than lossily converted. The reader's
//! contract starts at `&str`, so feeding it replacement characters would
//! fuzz the conversion instead of the parser, and every crash found that
//! way would be unreachable from a real file.

#![no_main]

use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Ok(text) = core::str::from_utf8(data) {
        let _ = renew_trace::parse(text);
    }
});
