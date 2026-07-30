//! A test-only Vulkan layer: transparent passthrough to the next layer
//! or driver, with named calls failed on cue (the fault protocol lands
//! in the next milestone; this milestone is the loader contract plus
//! provable transparency).
//!
//! Loader contract implemented here, against the SDK's `vk_layer.h`:
//! `vkNegotiateLoaderLayerInterfaceVersion` hands the loader our
//! GetInstanceProcAddr/GetDeviceProcAddr; vkCreateInstance and
//! vkCreateDevice walk the loader's layer chain (`sType` 47/48), take
//! the next link's proc-addr entry points, advance the chain, and call
//! down; every other call resolves through the stored next entry
//! points, so the layer adds nothing but a table lookup.
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

/// Per-instance state: the next layer's instance entry point plus the
/// pointers this layer resolves through it.
struct InstanceState {
    next_gipa: vk::PFN_vkGetInstanceProcAddr,
    destroy_instance: vk::PFN_vkDestroyInstance,
}

/// Per-device state: the next layer's device entry point plus the
/// pointers this layer resolves through it.
struct DeviceState {
    next_gdpa: vk::PFN_vkGetDeviceProcAddr,
    destroy_device: vk::PFN_vkDestroyDevice,
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
        _ => {
            if instance == vk::Instance::null() {
                return None;
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
        _ => {
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
/// record the next device entry point.
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
