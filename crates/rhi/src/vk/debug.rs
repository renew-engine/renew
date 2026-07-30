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
    if let Ok(mut retained) = counters.first_messages.lock()
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
