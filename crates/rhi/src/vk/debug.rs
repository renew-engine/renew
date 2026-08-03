//! The validation-layer message funnel: messages become diag records
//! plus per-device counters, so tests can assert "zero validation
//! errors" mechanically.

use core::ffi::c_void;
use core::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use ash::vk;

/// How many first-messages are retained verbatim for reports.
const RETAINED_MESSAGES: usize = 8;

/// Per-device validation tallies. The callback can fire on driver
/// threads: counters are relaxed atomics, message capture is behind a
/// mutex (never taken on the render path — only when validation
/// speaks, which is itself a defect signal).
#[derive(Debug, Default)]
pub struct ValidationCounters {
    pub errors: AtomicU64,
    pub warnings: AtomicU64,
    pub first_messages: Mutex<Vec<String>>,
}

extern "system" fn messenger_cb(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    callback_data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    user_data: *mut c_void,
) -> vk::Bool32 {
    if user_data.is_null() || callback_data.is_null() {
        return vk::FALSE;
    }
    // SAFETY: `user_data` is the `&ValidationCounters` installed at
    // messenger creation; the counters live in the device spine, which
    // destroys the messenger (through these very callbacks) before the
    // counters drop. `callback_data` is valid for the duration of the
    // callback per the Vulkan contract.
    let (counters, message) = unsafe {
        let counters = &*user_data.cast::<ValidationCounters>();
        let data = &*callback_data;
        let message = if data.p_message.is_null() {
            String::from("(no message)")
        } else {
            core::ffi::CStr::from_ptr(data.p_message)
                .to_string_lossy()
                .into_owned()
        };
        (counters, message)
    };
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR) {
        counters.errors.fetch_add(1, Ordering::Relaxed);
        renew_diag::error!(target: "renew-rhi", "validation: {message}");
    } else if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING) {
        counters.warnings.fetch_add(1, Ordering::Relaxed);
        renew_diag::warn!(target: "renew-rhi", "validation: {message}");
    }
    // Retain errors only. Keeping the first N of any severity let
    // loader chatter fill every slot — first infos, then, once infos
    // were excluded, the loader's own layer-management WARNINGS — so a
    // report could say "3 errors" while showing eight lines of noise: a
    // diagnostic that hides the diagnosis. The counters still count
    // warnings; the retained text answers one question, "what made this
    // red", and only errors make it red. Found the first time a test
    // drove a failure corner past a noisy loader.
    if severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR)
        && let Ok(mut retained) = counters.first_messages.lock()
        && retained.len() < RETAINED_MESSAGES
    {
        retained.push(message);
    }
    vk::FALSE
}

/// The messenger create-info wired to a counters block.
pub fn messenger_info(counters: &ValidationCounters) -> vk::DebugUtilsMessengerCreateInfoEXT<'_> {
    vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR
                | vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
        )
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(messenger_cb))
        .user_data(core::ptr::from_ref(counters).cast_mut().cast())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn call(
        counters: &ValidationCounters,
        severity: vk::DebugUtilsMessageSeverityFlagsEXT,
        message: &core::ffi::CStr,
    ) -> vk::Bool32 {
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default().message(message);
        messenger_cb(
            severity,
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
            &raw const data,
            core::ptr::from_ref(counters).cast_mut().cast(),
        )
    }

    #[test]
    fn errors_and_warnings_are_tallied_and_retained() {
        let counters = ValidationCounters::default();
        let verdict = call(
            &counters,
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            c"bad barrier",
        );
        assert_eq!(verdict, vk::FALSE, "the callback never aborts the call");
        call(
            &counters,
            vk::DebugUtilsMessageSeverityFlagsEXT::WARNING,
            c"suspicious usage",
        );
        assert_eq!(counters.errors.load(Ordering::Relaxed), 1);
        assert_eq!(counters.warnings.load(Ordering::Relaxed), 1);
        // Warnings are counted and NOT retained: the retained text
        // answers "what made this red", and only errors make it red.
        // The loader's layer-management warnings taught that lesson by
        // flooding every slot in the first report anyone actually
        // needed.
        let retained = counters.first_messages.lock().expect("retained lock");
        assert_eq!(retained.as_slice(), ["bad barrier"]);
    }

    #[test]
    fn retention_caps_while_counters_keep_counting() {
        let counters = ValidationCounters::default();
        for _ in 0..(RETAINED_MESSAGES + 4) {
            call(
                &counters,
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                c"repeated",
            );
        }
        assert_eq!(
            counters.errors.load(Ordering::Relaxed),
            (RETAINED_MESSAGES + 4) as u64
        );
        let retained = counters.first_messages.lock().expect("retained lock");
        assert_eq!(retained.len(), RETAINED_MESSAGES, "retention is capped");
    }

    #[test]
    fn null_pointers_are_tolerated_without_counting() {
        let counters = ValidationCounters::default();
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default();
        // Null user data: nothing to count into.
        assert_eq!(
            messenger_cb(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
                &raw const data,
                core::ptr::null_mut(),
            ),
            vk::FALSE
        );
        // Null callback data: nothing to read.
        assert_eq!(
            messenger_cb(
                vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
                vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
                core::ptr::null(),
                core::ptr::from_ref(&counters).cast_mut().cast(),
            ),
            vk::FALSE
        );
        assert_eq!(counters.errors.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn a_message_less_record_still_counts_with_a_placeholder() {
        let counters = ValidationCounters::default();
        let data = vk::DebugUtilsMessengerCallbackDataEXT::default();
        messenger_cb(
            vk::DebugUtilsMessageSeverityFlagsEXT::ERROR,
            vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION,
            &raw const data,
            core::ptr::from_ref(&counters).cast_mut().cast(),
        );
        assert_eq!(counters.errors.load(Ordering::Relaxed), 1);
        let retained = counters.first_messages.lock().expect("retained lock");
        assert_eq!(retained.as_slice(), ["(no message)"]);
    }
}
