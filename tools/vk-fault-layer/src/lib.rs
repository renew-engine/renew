//! A test-only Vulkan layer: transparent passthrough to the next layer
//! or driver, with named calls failed on cue, so driver-failure paths
//! are deterministically testable.
//!
//! Loader contract implemented here, against the SDK's `vk_layer.h`:
//! `vkNegotiateLoaderLayerInterfaceVersion` hands the loader our
//! GetInstanceProcAddr/GetDeviceProcAddr; `vkCreateInstance` and
//! `vkCreateDevice` walk the loader's layer chain (`sType` 47/48), take
//! the next link's proc-addr entry points, advance the chain, and call
//! down; every other call resolves through the stored next entry
//! points, so the layer adds nothing but a table lookup.
//!
//! Fault protocol: `RENEW_FAULT=<vkCallName>=<result>[@<ordinal>]`
//! arms exactly one fault — the ordinal-th occurrence (1-based,
//! default 1) of the named call returns the given result instead of
//! calling down. The variable is re-read at every `vkCreateInstance`,
//! so one test process can run many scenarios back to back; unset or
//! malformed re-arms to nothing, silently — this layer lives in a
//! stranger's process and never panics over an env var.
//!
//! Everything here is FFI: `unsafe` throughout, every site commented.

use std::collections::HashMap;
use std::ffi::{CStr, c_char, c_void};
use std::sync::{LazyLock, Mutex};

use ash::vk;
use ash::vk::Handle as _;

/// `VkNegotiateLayerInterface` from `vk_layer.h` (not part of the API
/// headers ash generates).
#[repr(C)]
pub struct NegotiateLayerInterface {
    s_type: i32,
    p_next: *mut c_void,
    loader_layer_interface_version: u32,
    pfn_get_instance_proc_addr: Option<vk::PFN_vkGetInstanceProcAddr>,
    pfn_get_device_proc_addr: Option<vk::PFN_vkGetDeviceProcAddr>,
    pfn_get_physical_device_proc_addr: Option<unsafe extern "system" fn()>,
}

/// `VkLayerInstanceLink` from `vk_layer.h`.
#[repr(C)]
struct LayerInstanceLink {
    p_next: *mut LayerInstanceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_physical_device_proc_addr: Option<unsafe extern "system" fn()>,
}

/// `VkLayerInstanceCreateInfo` from `vk_layer.h`. The union collapses
/// to the one member this layer reads (`pLayerInfo`); the loader only
/// hands us the `LINK_INFO` function where that member is active.
#[repr(C)]
struct LayerInstanceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: i32,
    p_layer_info: *mut LayerInstanceLink,
}

/// `VkLayerDeviceLink` from `vk_layer.h`.
#[repr(C)]
struct LayerDeviceLink {
    p_next: *mut LayerDeviceLink,
    pfn_next_get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pfn_next_get_device_proc_addr: vk::PFN_vkGetDeviceProcAddr,
}

/// `VkLayerDeviceCreateInfo` from `vk_layer.h`, collapsed as above.
#[repr(C)]
struct LayerDeviceCreateInfo {
    s_type: vk::StructureType,
    p_next: *const c_void,
    function: i32,
    p_layer_info: *mut LayerDeviceLink,
}

const LOADER_INSTANCE_CREATE_INFO: i32 = 47;
const LOADER_DEVICE_CREATE_INFO: i32 = 48;
const LAYER_LINK_INFO: i32 = 0;
const NEGOTIATE_INTERFACE_STRUCT: i32 = 1;

/// One armed fault: fail the `ordinal`-th occurrence of `call` with
/// `result`. `seen` is the live occurrence counter.
struct Fault {
    call: String,
    result: vk::Result,
    ordinal: u32,
    seen: u32,
}

fn fault_slot() -> &'static Mutex<Option<Fault>> {
    static SLOT: LazyLock<Mutex<Option<Fault>>> = LazyLock::new(|| Mutex::new(None));
    &SLOT
}

/// Map a `RENEW_FAULT` result name to its `vk::Result`; unnamed values
/// pass through as raw `i32`. `None` for garbage.
fn parse_result(name: &str) -> Option<vk::Result> {
    Some(match name {
        "SUCCESS" => vk::Result::SUCCESS,
        "TIMEOUT" => vk::Result::TIMEOUT,
        "NOT_READY" => vk::Result::NOT_READY,
        "ERROR_OUT_OF_HOST_MEMORY" => vk::Result::ERROR_OUT_OF_HOST_MEMORY,
        "ERROR_OUT_OF_DEVICE_MEMORY" => vk::Result::ERROR_OUT_OF_DEVICE_MEMORY,
        "ERROR_DEVICE_LOST" => vk::Result::ERROR_DEVICE_LOST,
        "ERROR_INITIALIZATION_FAILED" => vk::Result::ERROR_INITIALIZATION_FAILED,
        "ERROR_INCOMPATIBLE_DRIVER" => vk::Result::ERROR_INCOMPATIBLE_DRIVER,
        "ERROR_SURFACE_LOST_KHR" => vk::Result::ERROR_SURFACE_LOST_KHR,
        "ERROR_OUT_OF_DATE_KHR" => vk::Result::ERROR_OUT_OF_DATE_KHR,
        "ERROR_INVALID_SHADER_NV" => vk::Result::ERROR_INVALID_SHADER_NV,
        "ERROR_UNKNOWN" => vk::Result::ERROR_UNKNOWN,
        raw => vk::Result::from_raw(raw.parse::<i32>().ok()?),
    })
}

/// Parse `<vkCallName>=<result>[@<ordinal>]`; anything malformed is
/// `None`.
fn parse_fault(spec: &str) -> Option<Fault> {
    let (call, rest) = spec.split_once('=')?;
    let (result, ordinal) = match rest.split_once('@') {
        Some((result, ordinal)) => (result, ordinal.parse::<u32>().ok()?),
        None => (rest, 1),
    };
    if call.is_empty() || ordinal == 0 {
        return None;
    }
    Some(Fault {
        call: call.to_owned(),
        result: parse_result(result)?,
        ordinal,
        seen: 0,
    })
}

/// Re-arm the fault from `RENEW_FAULT`, counter reset; unset or
/// malformed clears it. Reading the environment is all this layer ever
/// does with it — nothing here calls `set_var`.
fn rearm_fault() {
    let armed = std::env::var("RENEW_FAULT")
        .ok()
        .and_then(|spec| parse_fault(&spec));
    if let Ok(mut slot) = fault_slot().lock() {
        *slot = armed;
    }
}

/// Count a call against the armed fault: `Some(result)` exactly on the
/// ordinal-th occurrence of the named call, `None` otherwise.
fn should_fail(call: &str) -> Option<vk::Result> {
    let mut slot = fault_slot().lock().ok()?;
    let fault = slot.as_mut()?;
    if fault.call != call {
        return None;
    }
    fault.seen += 1;
    (fault.seen == fault.ordinal).then_some(fault.result)
}

/// Per-instance state: the next layer's instance entry point plus the
/// pointers this layer resolves through it.
struct InstanceState {
    next_gipa: vk::PFN_vkGetInstanceProcAddr,
    destroy_instance: vk::PFN_vkDestroyInstance,
    enumerate_physical_devices: Option<vk::PFN_vkEnumeratePhysicalDevices>,
}

/// The next layer's entry points for every intercepted device-level
/// call, resolved eagerly at device creation. `None` means the next
/// layer does not have the function (absent extension) — the wrapper
/// for such a name is never advertised.
struct DeviceNext {
    create_image: Option<vk::PFN_vkCreateImage>,
    create_buffer: Option<vk::PFN_vkCreateBuffer>,
    create_image_view: Option<vk::PFN_vkCreateImageView>,
    create_shader_module: Option<vk::PFN_vkCreateShaderModule>,
    create_pipeline_layout: Option<vk::PFN_vkCreatePipelineLayout>,
    create_graphics_pipelines: Option<vk::PFN_vkCreateGraphicsPipelines>,
    create_command_pool: Option<vk::PFN_vkCreateCommandPool>,
    create_fence: Option<vk::PFN_vkCreateFence>,
    create_semaphore: Option<vk::PFN_vkCreateSemaphore>,
    create_swapchain_khr: Option<vk::PFN_vkCreateSwapchainKHR>,
    allocate_memory: Option<vk::PFN_vkAllocateMemory>,
    allocate_command_buffers: Option<vk::PFN_vkAllocateCommandBuffers>,
    bind_image_memory: Option<vk::PFN_vkBindImageMemory>,
    bind_buffer_memory: Option<vk::PFN_vkBindBufferMemory>,
    map_memory: Option<vk::PFN_vkMapMemory>,
    begin_command_buffer: Option<vk::PFN_vkBeginCommandBuffer>,
    end_command_buffer: Option<vk::PFN_vkEndCommandBuffer>,
    reset_command_buffer: Option<vk::PFN_vkResetCommandBuffer>,
    queue_submit2: Option<vk::PFN_vkQueueSubmit2>,
    wait_for_fences: Option<vk::PFN_vkWaitForFences>,
    reset_fences: Option<vk::PFN_vkResetFences>,
    device_wait_idle: Option<vk::PFN_vkDeviceWaitIdle>,
    acquire_next_image_khr: Option<vk::PFN_vkAcquireNextImageKHR>,
    queue_present_khr: Option<vk::PFN_vkQueuePresentKHR>,
}

/// Per-device state: the next layer's device entry point plus the
/// pointers this layer resolves through it.
struct DeviceState {
    next_gdpa: vk::PFN_vkGetDeviceProcAddr,
    destroy_device: vk::PFN_vkDestroyDevice,
    next: DeviceNext,
}

/// Dispatchable Vulkan handles begin with a loader-owned dispatch key;
/// keying on it follows every reference layer.
///
/// # Safety
///
/// `handle` must be a live dispatchable Vulkan handle.
unsafe fn dispatch_key(handle: *const c_void) -> usize {
    // SAFETY: per the contract, the first pointer-sized word of a
    // dispatchable handle is the loader's dispatch key.
    unsafe { *handle.cast::<usize>() }
}

fn instances() -> &'static Mutex<HashMap<usize, InstanceState>> {
    static MAP: LazyLock<Mutex<HashMap<usize, InstanceState>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &MAP
}

fn devices() -> &'static Mutex<HashMap<usize, DeviceState>> {
    static MAP: LazyLock<Mutex<HashMap<usize, DeviceState>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    &MAP
}

/// Erase a concrete `extern "system"` fn pointer to the dispatch
/// contract's currency, `PFN_vkVoidFunction`'s payload.
///
/// # Safety
///
/// `F` must be an `unsafe extern "system" fn` pointer type (they all
/// share one layout).
unsafe fn erase<F: Copy>(f: F) -> unsafe extern "system" fn() {
    // SAFETY: per the contract `F` is a fn pointer, the same size and
    // layout as the erased type.
    unsafe { std::mem::transmute_copy::<F, unsafe extern "system" fn()>(&f) }
}

/// Resolve `name` through the next layer's GIPA and reinterpret the
/// erased pointer as the concrete PFN type `F`.
///
/// # Safety
///
/// `F` must be exactly the PFN type the Vulkan spec assigns to `name`,
/// and `instance` must be live.
unsafe fn resolve_instance_pfn<F: Copy>(
    gipa: vk::PFN_vkGetInstanceProcAddr,
    instance: vk::Instance,
    name: &CStr,
) -> Option<F> {
    // SAFETY: resolving on the live instance through the next layer's
    // entry point, per the caller's contract.
    let erased = unsafe { gipa(instance, name.as_ptr()) }?;
    // SAFETY: per the caller's contract the erased pointer resolved
    // for `name` has exactly the signature `F`.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn(), F>(&erased) })
}

/// Resolve `name` through the next layer's GDPA and reinterpret the
/// erased pointer as the concrete PFN type `F`.
///
/// # Safety
///
/// `F` must be exactly the PFN type the Vulkan spec assigns to `name`,
/// and `device` must be live.
unsafe fn resolve_device_pfn<F: Copy>(
    gdpa: vk::PFN_vkGetDeviceProcAddr,
    device: vk::Device,
    name: &CStr,
) -> Option<F> {
    // SAFETY: resolving on the live device through the next layer's
    // entry point, per the caller's contract.
    let erased = unsafe { gdpa(device, name.as_ptr()) }?;
    // SAFETY: per the caller's contract the erased pointer resolved
    // for `name` has exactly the signature `F`.
    Some(unsafe { std::mem::transmute_copy::<unsafe extern "system" fn(), F>(&erased) })
}

/// Eagerly resolve every intercepted device-level entry point through
/// the next layer's GDPA.
///
/// # Safety
///
/// `device` must be the live device just created through the chain.
unsafe fn resolve_device_next(gdpa: vk::PFN_vkGetDeviceProcAddr, device: vk::Device) -> DeviceNext {
    // SAFETY: each field's PFN type pins the exact signature for the
    // name resolved into it; the device is live per the contract.
    unsafe {
        DeviceNext {
            create_image: resolve_device_pfn(gdpa, device, c"vkCreateImage"),
            create_buffer: resolve_device_pfn(gdpa, device, c"vkCreateBuffer"),
            create_image_view: resolve_device_pfn(gdpa, device, c"vkCreateImageView"),
            create_shader_module: resolve_device_pfn(gdpa, device, c"vkCreateShaderModule"),
            create_pipeline_layout: resolve_device_pfn(gdpa, device, c"vkCreatePipelineLayout"),
            create_graphics_pipelines: resolve_device_pfn(
                gdpa,
                device,
                c"vkCreateGraphicsPipelines",
            ),
            create_command_pool: resolve_device_pfn(gdpa, device, c"vkCreateCommandPool"),
            create_fence: resolve_device_pfn(gdpa, device, c"vkCreateFence"),
            create_semaphore: resolve_device_pfn(gdpa, device, c"vkCreateSemaphore"),
            create_swapchain_khr: resolve_device_pfn(gdpa, device, c"vkCreateSwapchainKHR"),
            allocate_memory: resolve_device_pfn(gdpa, device, c"vkAllocateMemory"),
            allocate_command_buffers: resolve_device_pfn(gdpa, device, c"vkAllocateCommandBuffers"),
            bind_image_memory: resolve_device_pfn(gdpa, device, c"vkBindImageMemory"),
            bind_buffer_memory: resolve_device_pfn(gdpa, device, c"vkBindBufferMemory"),
            map_memory: resolve_device_pfn(gdpa, device, c"vkMapMemory"),
            begin_command_buffer: resolve_device_pfn(gdpa, device, c"vkBeginCommandBuffer"),
            end_command_buffer: resolve_device_pfn(gdpa, device, c"vkEndCommandBuffer"),
            reset_command_buffer: resolve_device_pfn(gdpa, device, c"vkResetCommandBuffer"),
            queue_submit2: resolve_device_pfn(gdpa, device, c"vkQueueSubmit2"),
            wait_for_fences: resolve_device_pfn(gdpa, device, c"vkWaitForFences"),
            reset_fences: resolve_device_pfn(gdpa, device, c"vkResetFences"),
            device_wait_idle: resolve_device_pfn(gdpa, device, c"vkDeviceWaitIdle"),
            acquire_next_image_khr: resolve_device_pfn(gdpa, device, c"vkAcquireNextImageKHR"),
            queue_present_khr: resolve_device_pfn(gdpa, device, c"vkQueuePresentKHR"),
        }
    }
}

/// Copy the next-layer pfn selected by `pick` for the device owning
/// `handle`. Queues and command buffers share their device's dispatch
/// key, so one lookup serves all three handle kinds.
///
/// # Safety
///
/// `handle` must be the raw value of a live dispatchable handle (or
/// zero, which resolves to `None`).
unsafe fn device_next<F: Copy>(
    handle: u64,
    pick: impl FnOnce(&DeviceNext) -> Option<F>,
) -> Option<F> {
    if handle == 0 {
        return None;
    }
    let map = devices().lock().ok()?;
    // SAFETY: per the caller's contract, `handle` is live.
    let state = map.get(&unsafe { dispatch_key(handle as *const c_void) })?;
    pick(&state.next)
}

/// The erased wrapper for an intercepted device-level name, `None` for
/// names this layer leaves alone. Shared by both proc-addr entry
/// points: the loader resolves device functions through GDPA, but some
/// paths resolve them through the instance chain, and both must hand
/// out the same interception.
fn device_level_wrapper(name: &[u8]) -> vk::PFN_vkVoidFunction {
    // SAFETY: every arm pairs a wrapper with the PFN type of exactly
    // the call it implements (signature-checked at the coercion), then
    // erases it to the dispatch contract's currency.
    unsafe {
        match name {
            b"vkCreateImage" => Some(erase::<vk::PFN_vkCreateImage>(fault_create_image)),
            b"vkCreateBuffer" => Some(erase::<vk::PFN_vkCreateBuffer>(fault_create_buffer)),
            b"vkCreateImageView" => {
                Some(erase::<vk::PFN_vkCreateImageView>(fault_create_image_view))
            }
            b"vkCreateShaderModule" => Some(erase::<vk::PFN_vkCreateShaderModule>(
                fault_create_shader_module,
            )),
            b"vkCreatePipelineLayout" => Some(erase::<vk::PFN_vkCreatePipelineLayout>(
                fault_create_pipeline_layout,
            )),
            b"vkCreateGraphicsPipelines" => Some(erase::<vk::PFN_vkCreateGraphicsPipelines>(
                fault_create_graphics_pipelines,
            )),
            b"vkCreateCommandPool" => Some(erase::<vk::PFN_vkCreateCommandPool>(
                fault_create_command_pool,
            )),
            b"vkCreateFence" => Some(erase::<vk::PFN_vkCreateFence>(fault_create_fence)),
            b"vkCreateSemaphore" => {
                Some(erase::<vk::PFN_vkCreateSemaphore>(fault_create_semaphore))
            }
            b"vkCreateSwapchainKHR" => Some(erase::<vk::PFN_vkCreateSwapchainKHR>(
                fault_create_swapchain_khr,
            )),
            b"vkAllocateMemory" => Some(erase::<vk::PFN_vkAllocateMemory>(fault_allocate_memory)),
            b"vkAllocateCommandBuffers" => Some(erase::<vk::PFN_vkAllocateCommandBuffers>(
                fault_allocate_command_buffers,
            )),
            b"vkBindImageMemory" => {
                Some(erase::<vk::PFN_vkBindImageMemory>(fault_bind_image_memory))
            }
            b"vkBindBufferMemory" => Some(erase::<vk::PFN_vkBindBufferMemory>(
                fault_bind_buffer_memory,
            )),
            b"vkMapMemory" => Some(erase::<vk::PFN_vkMapMemory>(fault_map_memory)),
            b"vkBeginCommandBuffer" => Some(erase::<vk::PFN_vkBeginCommandBuffer>(
                fault_begin_command_buffer,
            )),
            b"vkEndCommandBuffer" => Some(erase::<vk::PFN_vkEndCommandBuffer>(
                fault_end_command_buffer,
            )),
            b"vkResetCommandBuffer" => Some(erase::<vk::PFN_vkResetCommandBuffer>(
                fault_reset_command_buffer,
            )),
            b"vkQueueSubmit2" => Some(erase::<vk::PFN_vkQueueSubmit2>(fault_queue_submit2)),
            b"vkWaitForFences" => Some(erase::<vk::PFN_vkWaitForFences>(fault_wait_for_fences)),
            b"vkResetFences" => Some(erase::<vk::PFN_vkResetFences>(fault_reset_fences)),
            b"vkDeviceWaitIdle" => Some(erase::<vk::PFN_vkDeviceWaitIdle>(fault_device_wait_idle)),
            b"vkAcquireNextImageKHR" => Some(erase::<vk::PFN_vkAcquireNextImageKHR>(
                fault_acquire_next_image_khr,
            )),
            b"vkQueuePresentKHR" => {
                Some(erase::<vk::PFN_vkQueuePresentKHR>(fault_queue_present_khr))
            }
            _ => None,
        }
    }
}

/// Advertise a swapchain-family wrapper only when the next layer
/// actually has the entry point — the extension may be absent, and a
/// wrapper over nothing must not exist either.
fn advertise_khr(
    device: vk::Device,
    has_next: impl FnOnce(&DeviceNext) -> bool,
    wrapper: unsafe extern "system" fn(),
) -> vk::PFN_vkVoidFunction {
    let map = devices().lock().ok()?;
    // SAFETY: a non-null device reaching a layer is live.
    let state = map.get(&unsafe { dispatch_key(device.as_raw() as *const c_void) })?;
    if has_next(&state.next) {
        Some(wrapper)
    } else {
        None
    }
}

/// The negotiation entry point the loader looks up by name.
///
/// # Safety
///
/// Called by the Vulkan loader with a valid, writable struct.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn vkNegotiateLoaderLayerInterfaceVersion(
    p_version_struct: *mut NegotiateLayerInterface,
) -> vk::Result {
    if p_version_struct.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    // SAFETY: the loader hands a valid struct per the layer contract.
    let version = unsafe { &mut *p_version_struct };
    if version.s_type != NEGOTIATE_INTERFACE_STRUCT {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    if version.loader_layer_interface_version < 2 {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    version.loader_layer_interface_version = 2;
    version.pfn_get_instance_proc_addr = Some(fault_get_instance_proc_addr);
    version.pfn_get_device_proc_addr = Some(fault_get_device_proc_addr);
    version.pfn_get_physical_device_proc_addr = None;
    vk::Result::SUCCESS
}

/// The layer's vkGetInstanceProcAddr.
///
/// # Safety
///
/// Called by the loader/next layers per the Vulkan dispatch contract.
unsafe extern "system" fn fault_get_instance_proc_addr(
    instance: vk::Instance,
    p_name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    if p_name.is_null() {
        return None;
    }
    // SAFETY: p_name is a valid NUL-terminated string per the contract.
    let name = unsafe { CStr::from_ptr(p_name) };
    match name.to_bytes() {
        b"vkGetInstanceProcAddr" => {
            // SAFETY: transmuting a concrete `extern "system"` fn to the
            // erased PFN_vkVoidFunction, the dispatch contract's currency.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkGetInstanceProcAddr, unsafe extern "system" fn()>(
                    fault_get_instance_proc_addr,
                )
            })
        }
        b"vkCreateInstance" => {
            // SAFETY: as above.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkCreateInstance, unsafe extern "system" fn()>(
                    fault_create_instance,
                )
            })
        }
        b"vkDestroyInstance" => {
            // SAFETY: as above.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkDestroyInstance, unsafe extern "system" fn()>(
                    fault_destroy_instance,
                )
            })
        }
        b"vkEnumeratePhysicalDevices" => {
            // SAFETY: as above.
            Some(unsafe {
                erase::<vk::PFN_vkEnumeratePhysicalDevices>(fault_enumerate_physical_devices)
            })
        }
        b"vkCreateDevice" => {
            // SAFETY: as above.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkCreateDevice, unsafe extern "system" fn()>(
                    fault_create_device,
                )
            })
        }
        b"vkGetDeviceProcAddr" => {
            // SAFETY: as above.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkGetDeviceProcAddr, unsafe extern "system" fn()>(
                    fault_get_device_proc_addr,
                )
            })
        }
        other => {
            if instance == vk::Instance::null() {
                return None;
            }
            // Some paths resolve device-level functions through the
            // instance chain; hand out the same wrappers they would
            // get from GDPA so no route bypasses the fault check.
            if let Some(wrapper) = device_level_wrapper(other) {
                return Some(wrapper);
            }
            let map = instances().lock().ok()?;
            // SAFETY: a non-null instance reaching a layer is live.
            let state = map.get(&unsafe { dispatch_key(instance.as_raw() as *const c_void) })?;
            // SAFETY: chaining to the next layer's entry point with the
            // caller's own arguments.
            unsafe { (state.next_gipa)(instance, p_name) }
        }
    }
}

/// The layer's vkGetDeviceProcAddr.
///
/// # Safety
///
/// Called by the loader/next layers per the Vulkan dispatch contract.
unsafe extern "system" fn fault_get_device_proc_addr(
    device: vk::Device,
    p_name: *const c_char,
) -> vk::PFN_vkVoidFunction {
    if p_name.is_null() || device == vk::Device::null() {
        return None;
    }
    // SAFETY: p_name is a valid NUL-terminated string per the contract.
    let name = unsafe { CStr::from_ptr(p_name) };
    match name.to_bytes() {
        b"vkGetDeviceProcAddr" => {
            // SAFETY: as in the instance path.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkGetDeviceProcAddr, unsafe extern "system" fn()>(
                    fault_get_device_proc_addr,
                )
            })
        }
        b"vkDestroyDevice" => {
            // SAFETY: as above.
            Some(unsafe {
                std::mem::transmute::<vk::PFN_vkDestroyDevice, unsafe extern "system" fn()>(
                    fault_destroy_device,
                )
            })
        }
        // The swapchain family is advertised only when the next layer
        // has it: a wrapper must not conjure an absent extension.
        b"vkCreateSwapchainKHR" => advertise_khr(
            device,
            |next| next.create_swapchain_khr.is_some(),
            // SAFETY: erasing the wrapper for exactly this call.
            unsafe { erase::<vk::PFN_vkCreateSwapchainKHR>(fault_create_swapchain_khr) },
        ),
        b"vkAcquireNextImageKHR" => advertise_khr(
            device,
            |next| next.acquire_next_image_khr.is_some(),
            // SAFETY: as above.
            unsafe { erase::<vk::PFN_vkAcquireNextImageKHR>(fault_acquire_next_image_khr) },
        ),
        b"vkQueuePresentKHR" => advertise_khr(
            device,
            |next| next.queue_present_khr.is_some(),
            // SAFETY: as above.
            unsafe { erase::<vk::PFN_vkQueuePresentKHR>(fault_queue_present_khr) },
        ),
        other => {
            if let Some(wrapper) = device_level_wrapper(other) {
                return Some(wrapper);
            }
            let map = devices().lock().ok()?;
            // SAFETY: a non-null device reaching a layer is live.
            let state = map.get(&unsafe { dispatch_key(device.as_raw() as *const c_void) })?;
            // SAFETY: chaining to the next layer's entry point.
            unsafe { (state.next_gdpa)(device, p_name) }
        }
    }
}

/// The layer's vkCreateInstance: advance the loader chain, call down,
/// record the next entry point for the new instance.
///
/// # Safety
///
/// Called by the loader with valid create-info/allocator/out pointers.
unsafe extern "system" fn fault_create_instance(
    p_create_info: *const vk::InstanceCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_instance: *mut vk::Instance,
) -> vk::Result {
    // Every instance creation re-arms the fault from the environment,
    // so one test process can run many scenarios back to back.
    rearm_fault();
    if p_create_info.is_null() || p_instance.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    // Walk pNext for the loader's LINK_INFO entry.
    // SAFETY: the chain pointers come from the loader and are valid for
    // the duration of the call.
    let mut chain = unsafe { (*p_create_info).p_next }.cast::<LayerInstanceCreateInfo>();
    let link_info = loop {
        if chain.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        // SAFETY: as above; every chain node starts with sType/pNext.
        let node = unsafe { &*chain };
        if node.s_type.as_raw() == LOADER_INSTANCE_CREATE_INFO && node.function == LAYER_LINK_INFO {
            break chain.cast_mut();
        }
        chain = node.p_next.cast::<LayerInstanceCreateInfo>();
    };
    // SAFETY: LINK_INFO's active union member is pLayerInfo; the link
    // list has at least our own entry.
    let link = unsafe { (*link_info).p_layer_info };
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    // SAFETY: as above.
    let next_gipa = unsafe { (*link).pfn_next_get_instance_proc_addr };
    // Advance the chain for the next layer down (the documented
    // in-place mutation every layer performs).
    // SAFETY: the loader owns this mutable structure for exactly this
    // purpose.
    unsafe { (*link_info).p_layer_info = (*link).p_next };

    // The fault check sits after the chain advance so a failed
    // creation leaves the loader's chain state consistent either way.
    if let Some(result) = should_fail("vkCreateInstance") {
        return result;
    }

    // SAFETY: pre-instance resolution of vkCreateInstance through the
    // next link, per the layer contract.
    let next_create = unsafe { next_gipa(vk::Instance::null(), c"vkCreateInstance".as_ptr()) };
    let Some(next_create) = next_create else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the erased pointer resolved for "vkCreateInstance" has
    // exactly this signature.
    let next_create = unsafe {
        std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkCreateInstance>(next_create)
    };
    // SAFETY: calling down with the caller's own (valid) arguments.
    let result = unsafe { next_create(p_create_info, p_allocator, p_instance) };
    if result != vk::Result::SUCCESS {
        return result;
    }
    // SAFETY: on success the out-pointer holds the new live instance.
    let instance = unsafe { *p_instance };
    // SAFETY: resolving vkDestroyInstance on the live instance.
    let destroy = unsafe { next_gipa(instance, c"vkDestroyInstance".as_ptr()) };
    let Some(destroy) = destroy else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the erased pointer resolved for "vkDestroyInstance" has
    // exactly this signature.
    let destroy = unsafe {
        std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkDestroyInstance>(destroy)
    };
    if let Ok(mut map) = instances().lock() {
        map.insert(
            // SAFETY: the new instance is a live dispatchable handle.
            unsafe { dispatch_key(instance.as_raw() as *const c_void) },
            InstanceState {
                next_gipa,
                destroy_instance: destroy,
                // SAFETY: resolving on the live instance; the field
                // type pins the exact PFN signature for the name.
                enumerate_physical_devices: unsafe {
                    resolve_instance_pfn(next_gipa, instance, c"vkEnumeratePhysicalDevices")
                },
            },
        );
    }
    vk::Result::SUCCESS
}

/// # Safety
///
/// Called by the loader with a live instance (or null, a no-op).
unsafe extern "system" fn fault_destroy_instance(
    instance: vk::Instance,
    p_allocator: *const vk::AllocationCallbacks<'_>,
) {
    if instance == vk::Instance::null() {
        return;
    }
    // SAFETY: live dispatchable handle per the contract.
    let key = unsafe { dispatch_key(instance.as_raw() as *const c_void) };
    let state = instances().lock().ok().and_then(|mut map| map.remove(&key));
    if let Some(state) = state {
        // SAFETY: chaining the destruction down with the caller's
        // arguments; the entry point was resolved on this instance.
        unsafe { (state.destroy_instance)(instance, p_allocator) };
    }
}

/// The layer's vkCreateDevice: advance the loader chain, call down,
/// record the next device entry points.
///
/// # Safety
///
/// Called by the loader with valid arguments per the contract.
#[expect(
    clippy::similar_names,
    reason = "the layer contract's own vocabulary: instance vs device proc-addr"
)]
unsafe extern "system" fn fault_create_device(
    physical_device: vk::PhysicalDevice,
    p_create_info: *const vk::DeviceCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_device: *mut vk::Device,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateDevice") {
        return result;
    }
    if p_create_info.is_null() || p_device.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    // SAFETY: chain pointers from the loader, valid for the call.
    let mut chain = unsafe { (*p_create_info).p_next }.cast::<LayerDeviceCreateInfo>();
    let link_info = loop {
        if chain.is_null() {
            return vk::Result::ERROR_INITIALIZATION_FAILED;
        }
        // SAFETY: as above.
        let node = unsafe { &*chain };
        if node.s_type.as_raw() == LOADER_DEVICE_CREATE_INFO && node.function == LAYER_LINK_INFO {
            break chain.cast_mut();
        }
        chain = node.p_next.cast::<LayerDeviceCreateInfo>();
    };
    // SAFETY: LINK_INFO's active member is pLayerInfo.
    let link = unsafe { (*link_info).p_layer_info };
    if link.is_null() {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    }
    // SAFETY: as the link reads above: the loader-owned link node is
    // valid for the duration of the call.
    let (next_gipa, next_gdpa) = unsafe {
        (
            (*link).pfn_next_get_instance_proc_addr,
            (*link).pfn_next_get_device_proc_addr,
        )
    };
    // SAFETY: advancing the loader-owned chain, as documented.
    unsafe { (*link_info).p_layer_info = (*link).p_next };

    // SAFETY: vkCreateDevice is resolved through the next GIPA with a
    // null instance per the layer chain contract.
    let next_create = unsafe { next_gipa(vk::Instance::null(), c"vkCreateDevice".as_ptr()) };
    let Some(next_create) = next_create else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the erased pointer resolved for "vkCreateDevice" has
    // exactly this signature.
    let next_create = unsafe {
        std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkCreateDevice>(next_create)
    };
    // SAFETY: calling down with the caller's own arguments.
    let result = unsafe { next_create(physical_device, p_create_info, p_allocator, p_device) };
    if result != vk::Result::SUCCESS {
        return result;
    }
    // SAFETY: on success the out-pointer holds the new live device.
    let device = unsafe { *p_device };
    // SAFETY: resolving vkDestroyDevice on the live device through the
    // next GDPA.
    let destroy = unsafe { next_gdpa(device, c"vkDestroyDevice".as_ptr()) };
    let Some(destroy) = destroy else {
        return vk::Result::ERROR_INITIALIZATION_FAILED;
    };
    // SAFETY: the erased pointer resolved for "vkDestroyDevice" has
    // exactly this signature.
    let destroy = unsafe {
        std::mem::transmute::<unsafe extern "system" fn(), vk::PFN_vkDestroyDevice>(destroy)
    };
    if let Ok(mut map) = devices().lock() {
        map.insert(
            // SAFETY: the new device is a live dispatchable handle.
            unsafe { dispatch_key(device.as_raw() as *const c_void) },
            DeviceState {
                next_gdpa,
                destroy_device: destroy,
                // SAFETY: the device is live; every name is paired
                // with its exact PFN type by the field it fills.
                next: unsafe { resolve_device_next(next_gdpa, device) },
            },
        );
    }
    vk::Result::SUCCESS
}

/// # Safety
///
/// Called by the loader with a live device (or null, a no-op).
unsafe extern "system" fn fault_destroy_device(
    device: vk::Device,
    p_allocator: *const vk::AllocationCallbacks<'_>,
) {
    if device == vk::Device::null() {
        return;
    }
    // SAFETY: live dispatchable handle per the contract.
    let key = unsafe { dispatch_key(device.as_raw() as *const c_void) };
    let state = devices().lock().ok().and_then(|mut map| map.remove(&key));
    if let Some(state) = state {
        // SAFETY: chaining the destruction down; resolved on this
        // device.
        unsafe { (state.destroy_device)(device, p_allocator) };
    }
}

// --- Intercepted calls. Every wrapper has the same shape: fail on
// cue, otherwise chain through the state resolved at creation; a
// wrapper whose state is gone answers ERROR_UNKNOWN — a layer never
// unwinds into its host.

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_enumerate_physical_devices(
    instance: vk::Instance,
    p_physical_device_count: *mut u32,
    p_physical_devices: *mut vk::PhysicalDevice,
) -> vk::Result {
    if let Some(result) = should_fail("vkEnumeratePhysicalDevices") {
        return result;
    }
    if instance == vk::Instance::null() {
        return vk::Result::ERROR_UNKNOWN;
    }
    let next = instances().lock().ok().and_then(|map| {
        // SAFETY: a non-null instance reaching a layer wrapper is live.
        map.get(&unsafe { dispatch_key(instance.as_raw() as *const c_void) })
            .and_then(|state| state.enumerate_physical_devices)
    });
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(instance, p_physical_device_count, p_physical_devices) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_image(
    device: vk::Device,
    p_create_info: *const vk::ImageCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_image: *mut vk::Image,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateImage") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_image) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_image) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_buffer(
    device: vk::Device,
    p_create_info: *const vk::BufferCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_buffer: *mut vk::Buffer,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateBuffer") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_buffer) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_buffer) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_image_view(
    device: vk::Device,
    p_create_info: *const vk::ImageViewCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_view: *mut vk::ImageView,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateImageView") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_image_view) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_view) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_shader_module(
    device: vk::Device,
    p_create_info: *const vk::ShaderModuleCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_shader_module: *mut vk::ShaderModule,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateShaderModule") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_shader_module) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_shader_module) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_pipeline_layout(
    device: vk::Device,
    p_create_info: *const vk::PipelineLayoutCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_pipeline_layout: *mut vk::PipelineLayout,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreatePipelineLayout") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_pipeline_layout) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_pipeline_layout) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_graphics_pipelines(
    device: vk::Device,
    pipeline_cache: vk::PipelineCache,
    create_info_count: u32,
    p_create_infos: *const vk::GraphicsPipelineCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_pipelines: *mut vk::Pipeline,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateGraphicsPipelines") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_graphics_pipelines) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe {
        next(
            device,
            pipeline_cache,
            create_info_count,
            p_create_infos,
            p_allocator,
            p_pipelines,
        )
    }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_command_pool(
    device: vk::Device,
    p_create_info: *const vk::CommandPoolCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_command_pool: *mut vk::CommandPool,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateCommandPool") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_command_pool) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_command_pool) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_fence(
    device: vk::Device,
    p_create_info: *const vk::FenceCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_fence: *mut vk::Fence,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateFence") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_fence) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_fence) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_semaphore(
    device: vk::Device,
    p_create_info: *const vk::SemaphoreCreateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_semaphore: *mut vk::Semaphore,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateSemaphore") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_semaphore) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_semaphore) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_create_swapchain_khr(
    device: vk::Device,
    p_create_info: *const vk::SwapchainCreateInfoKHR<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_swapchain: *mut vk::SwapchainKHR,
) -> vk::Result {
    if let Some(result) = should_fail("vkCreateSwapchainKHR") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.create_swapchain_khr) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_create_info, p_allocator, p_swapchain) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_allocate_memory(
    device: vk::Device,
    p_allocate_info: *const vk::MemoryAllocateInfo<'_>,
    p_allocator: *const vk::AllocationCallbacks<'_>,
    p_memory: *mut vk::DeviceMemory,
) -> vk::Result {
    if let Some(result) = should_fail("vkAllocateMemory") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.allocate_memory) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_allocate_info, p_allocator, p_memory) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_allocate_command_buffers(
    device: vk::Device,
    p_allocate_info: *const vk::CommandBufferAllocateInfo<'_>,
    p_command_buffers: *mut vk::CommandBuffer,
) -> vk::Result {
    if let Some(result) = should_fail("vkAllocateCommandBuffers") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.allocate_command_buffers) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, p_allocate_info, p_command_buffers) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_bind_image_memory(
    device: vk::Device,
    image: vk::Image,
    memory: vk::DeviceMemory,
    memory_offset: vk::DeviceSize,
) -> vk::Result {
    if let Some(result) = should_fail("vkBindImageMemory") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.bind_image_memory) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, image, memory, memory_offset) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_bind_buffer_memory(
    device: vk::Device,
    buffer: vk::Buffer,
    memory: vk::DeviceMemory,
    memory_offset: vk::DeviceSize,
) -> vk::Result {
    if let Some(result) = should_fail("vkBindBufferMemory") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.bind_buffer_memory) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, buffer, memory, memory_offset) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_map_memory(
    device: vk::Device,
    memory: vk::DeviceMemory,
    offset: vk::DeviceSize,
    size: vk::DeviceSize,
    flags: vk::MemoryMapFlags,
    pp_data: *mut *mut c_void,
) -> vk::Result {
    if let Some(result) = should_fail("vkMapMemory") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.map_memory) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, memory, offset, size, flags, pp_data) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_begin_command_buffer(
    command_buffer: vk::CommandBuffer,
    p_begin_info: *const vk::CommandBufferBeginInfo<'_>,
) -> vk::Result {
    if let Some(result) = should_fail("vkBeginCommandBuffer") {
        return result;
    }
    // SAFETY: command buffers carry their device's dispatch key; a
    // non-null handle reaching a layer wrapper is live.
    let next = unsafe { device_next(command_buffer.as_raw(), |next| next.begin_command_buffer) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(command_buffer, p_begin_info) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_end_command_buffer(
    command_buffer: vk::CommandBuffer,
) -> vk::Result {
    if let Some(result) = should_fail("vkEndCommandBuffer") {
        return result;
    }
    // SAFETY: command buffers carry their device's dispatch key; a
    // non-null handle reaching a layer wrapper is live.
    let next = unsafe { device_next(command_buffer.as_raw(), |next| next.end_command_buffer) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(command_buffer) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_reset_command_buffer(
    command_buffer: vk::CommandBuffer,
    flags: vk::CommandBufferResetFlags,
) -> vk::Result {
    if let Some(result) = should_fail("vkResetCommandBuffer") {
        return result;
    }
    // SAFETY: command buffers carry their device's dispatch key; a
    // non-null handle reaching a layer wrapper is live.
    let next = unsafe { device_next(command_buffer.as_raw(), |next| next.reset_command_buffer) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(command_buffer, flags) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_queue_submit2(
    queue: vk::Queue,
    submit_count: u32,
    p_submits: *const vk::SubmitInfo2<'_>,
    fence: vk::Fence,
) -> vk::Result {
    if let Some(result) = should_fail("vkQueueSubmit2") {
        return result;
    }
    // SAFETY: queues carry their device's dispatch key; a non-null
    // handle reaching a layer wrapper is live.
    let next = unsafe { device_next(queue.as_raw(), |next| next.queue_submit2) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(queue, submit_count, p_submits, fence) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_wait_for_fences(
    device: vk::Device,
    fence_count: u32,
    p_fences: *const vk::Fence,
    wait_all: vk::Bool32,
    timeout: u64,
) -> vk::Result {
    if let Some(result) = should_fail("vkWaitForFences") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.wait_for_fences) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, fence_count, p_fences, wait_all, timeout) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_reset_fences(
    device: vk::Device,
    fence_count: u32,
    p_fences: *const vk::Fence,
) -> vk::Result {
    if let Some(result) = should_fail("vkResetFences") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.reset_fences) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, fence_count, p_fences) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_device_wait_idle(device: vk::Device) -> vk::Result {
    if let Some(result) = should_fail("vkDeviceWaitIdle") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.device_wait_idle) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_acquire_next_image_khr(
    device: vk::Device,
    swapchain: vk::SwapchainKHR,
    timeout: u64,
    semaphore: vk::Semaphore,
    fence: vk::Fence,
    p_image_index: *mut u32,
) -> vk::Result {
    if let Some(result) = should_fail("vkAcquireNextImageKHR") {
        return result;
    }
    // SAFETY: a non-null device reaching a layer wrapper is live.
    let next = unsafe { device_next(device.as_raw(), |next| next.acquire_next_image_khr) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(device, swapchain, timeout, semaphore, fence, p_image_index) }
}

/// # Safety
///
/// Called through the dispatch chain with valid arguments per the
/// Vulkan contract.
unsafe extern "system" fn fault_queue_present_khr(
    queue: vk::Queue,
    p_present_info: *const vk::PresentInfoKHR<'_>,
) -> vk::Result {
    if let Some(result) = should_fail("vkQueuePresentKHR") {
        return result;
    }
    // SAFETY: queues carry their device's dispatch key; a non-null
    // handle reaching a layer wrapper is live.
    let next = unsafe { device_next(queue.as_raw(), |next| next.queue_present_khr) };
    let Some(next) = next else {
        return vk::Result::ERROR_UNKNOWN;
    };
    // SAFETY: chaining to the next layer with the caller's own
    // arguments.
    unsafe { next(queue, p_present_info) }
}
