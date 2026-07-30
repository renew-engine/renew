//! Device bring-up and the shared spine every GPU object hangs off.
//!
//! Ownership model: `Device` is a handle to an `Rc<DeviceShared>`; the
//! `Rc` makes the whole RHI structurally `!Send + !Sync` (the v0
//! single-thread contract, in the type system). Every resource wrapper
//! holds its own `Rc`, so the device outlives every resource without
//! runtime tracking; `DeviceShared::drop` destroys in exact reverse
//! creation order after a best-effort wait-idle.

use core::cell::Cell;
use std::rc::Rc;

use ash::vk;

use crate::config::{AdapterInfo, AdapterKind, DeviceDesc, Validation};
use crate::error::DeviceError;
use crate::vk::alloc::AllocLedger;
use crate::vk::debug::ValidationCounters;

const VALIDATION_LAYER: &core::ffi::CStr = c"VK_LAYER_KHRONOS_validation";
/// Fence watchdog: a hang becomes a diagnosable failure, not a wedged
/// process.
pub(crate) const FENCE_TIMEOUT_NS: u64 = 5_000_000_000;

/// Driver host-allocation tallies, read via
/// [`Device::host_allocation_stats`]. Diagnostics only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostAllocationStats {
    pub allocations: u64,
    pub deallocations: u64,
    pub reallocations: u64,
    pub bytes_in_use: usize,
    pub peak_bytes: usize,
}

/// Validation-layer activity, read via [`Device::validation_report`].
#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub errors: u64,
    pub warnings: u64,
    /// The first few messages, verbatim.
    pub first_messages: Vec<String>,
}

/// Everything the GPU context owns, destroyed in reverse creation
/// order. Fields ordered so anything still alive at field-drop time is
/// safe to drop in declaration order (the handles are destroyed
/// explicitly in `Drop::drop` first).
pub(crate) struct DeviceShared {
    pub(crate) device: ash::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) queue_family: u32,
    pub(crate) physical: vk::PhysicalDevice,
    debug: Option<(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    pub(crate) instance: ash::Instance,
    pub(crate) entry: ash::Entry,
    pub(crate) adapter: AdapterInfo,
    pub(crate) lost: Cell<bool>,
    /// Boxed for address stability: the driver holds this pointer for
    /// the instance's whole life.
    validation: Box<ValidationCounters>,
    /// Same address-stability requirement.
    ledger: Box<AllocLedger>,
}

impl DeviceShared {
    /// The allocation callbacks pointing at this device's ledger.
    pub(crate) fn alloc_cbs(&self) -> vk::AllocationCallbacks<'_> {
        crate::vk::alloc::callbacks(&self.ledger)
    }

    /// Fail fast once the device is lost.
    pub(crate) fn check_lost(&self) -> Result<(), DeviceError> {
        if self.lost.get() {
            Err(DeviceError::DeviceLost)
        } else {
            Ok(())
        }
    }

    /// Record a raw result, poisoning the device on loss.
    pub(crate) fn note_result(&self, result: vk::Result) {
        if result == vk::Result::ERROR_DEVICE_LOST {
            self.lost.set(true);
        }
    }
}

impl Drop for DeviceShared {
    fn drop(&mut self) {
        // Best-effort quiesce; failure is logged, never a panic (D5).
        // SAFETY: the device handle is live (nothing destroys it before
        // this Drop) and externally synchronized (crate-wide !Send).
        let idle = unsafe { self.device.device_wait_idle() };
        if let Err(code) = idle {
            renew_diag::error!(target: "renew-rhi", "wait-idle at teardown failed: {code:?}");
        }
        // SAFETY: reverse creation order; each handle was created by
        // the paired create call with these same allocation callbacks;
        // no resource object can outlive this struct (each holds an Rc
        // to it).
        unsafe {
            self.device.destroy_device(Some(&self.alloc_cbs()));
            if let Some((utils, messenger)) = self.debug.take() {
                utils.destroy_debug_utils_messenger(messenger, Some(&self.alloc_cbs()));
            }
            self.instance.destroy_instance(Some(&self.alloc_cbs()));
        }
    }
}

/// The GPU context. `Clone` hands out another handle to the same
/// context; the context is destroyed when the last handle (and last
/// resource created from it) drops.
#[derive(Clone)]
pub struct Device {
    pub(crate) shared: Rc<DeviceShared>,
}

impl Device {
    /// Bring up the GPU context.
    ///
    /// # Errors
    ///
    /// [`DeviceError::LoaderUnavailable`] when no Vulkan runtime can be
    /// loaded (the graceful-skip seam); [`DeviceError::ValidationUnavailable`]
    /// when [`Validation::Required`] finds no layer; adapter and
    /// creation failures otherwise.
    #[expect(
        clippy::too_many_lines,
        reason = "one linear bring-up ladder; splitting it hides the creation order the teardown must mirror"
    )]
    pub fn new(desc: &DeviceDesc) -> Result<Self, DeviceError> {
        // SAFETY: category 1 (the one loader-entry site). Loading the
        // Vulkan runtime library; no aliasing or lifetime obligations —
        // the Entry owns the loaded library for the spine's lifetime.
        let entry =
            unsafe { ash::Entry::load() }.map_err(|error| DeviceError::LoaderUnavailable {
                message: error.to_string(),
            })?;

        // Ledger + counters first: their addresses go into driver
        // structures and must outlive everything created below.
        let ledger = Box::new(AllocLedger::default());
        let validation_counters = Box::new(ValidationCounters::default());

        // SAFETY: category 2 (ash dispatch): entry is live; no external
        // synchronization requirements on enumeration.
        let available_layers = unsafe { entry.enumerate_instance_layer_properties() }
            .map_err(|code| creation("vkEnumerateInstanceLayerProperties", code))?;
        let layer_available = available_layers.iter().any(|layer| {
            layer
                .layer_name_as_c_str()
                .is_ok_and(|name| name == VALIDATION_LAYER)
        });
        let validation_on = match desc.validation {
            Validation::Off => false,
            Validation::IfAvailable => layer_available,
            Validation::Required => {
                if !layer_available {
                    return Err(DeviceError::ValidationUnavailable);
                }
                true
            }
        };

        // SAFETY: category 2: as above.
        let available_extensions =
            unsafe { entry.enumerate_instance_extension_properties(None) }
                .map_err(|code| creation("vkEnumerateInstanceExtensionProperties", code))?;
        let has_extension = |name: &core::ffi::CStr| {
            available_extensions
                .iter()
                .any(|ext| ext.extension_name_as_c_str().is_ok_and(|n| n == name))
        };

        let mut extensions: Vec<*const core::ffi::c_char> = Vec::new();
        if validation_on {
            extensions.push(ash::ext::debug_utils::NAME.as_ptr());
        }
        let mut flags = vk::InstanceCreateFlags::empty();
        if has_extension(ash::khr::portability_enumeration::NAME) {
            // The loader only lists portability (MoltenVK) adapters when
            // asked; asking whenever possible keeps macOS working with
            // zero platform-specific code here.
            extensions.push(ash::khr::portability_enumeration::NAME.as_ptr());
            flags |= vk::InstanceCreateFlags::ENUMERATE_PORTABILITY_KHR;
        }
        #[cfg(feature = "present")]
        {
            // Surface extensions are instance-level and must be enabled
            // before any window exists; enable whichever this platform
            // offers.
            for name in [
                ash::khr::surface::NAME,
                ash::khr::win32_surface::NAME,
                ash::khr::xlib_surface::NAME,
                ash::khr::xcb_surface::NAME,
                ash::khr::wayland_surface::NAME,
                ash::ext::metal_surface::NAME,
            ] {
                if has_extension(name) {
                    extensions.push(name.as_ptr());
                }
            }
        }

        let app_name = std::ffi::CString::new(desc.app_name).unwrap_or_default();
        let app_info = vk::ApplicationInfo::default()
            .application_name(&app_name)
            .api_version(vk::API_VERSION_1_3);
        let layers: Vec<*const core::ffi::c_char> = if validation_on {
            vec![VALIDATION_LAYER.as_ptr()]
        } else {
            Vec::new()
        };
        let enabled_features = [vk::ValidationFeatureEnableEXT::SYNCHRONIZATION_VALIDATION];
        let mut validation_features =
            vk::ValidationFeaturesEXT::default().enabled_validation_features(&enabled_features);
        let mut messenger_info = crate::vk::debug::messenger_info(&validation_counters);
        let mut create_info = vk::InstanceCreateInfo::default()
            .application_info(&app_info)
            .enabled_layer_names(&layers)
            .enabled_extension_names(&extensions)
            .flags(flags);
        if validation_on && desc.validation == Validation::Required {
            create_info = create_info.push_next(&mut validation_features);
        }
        if validation_on {
            // Chaining the messenger info covers create/destroy-time
            // messages too.
            create_info = create_info.push_next(&mut messenger_info);
        }

        // SAFETY: category 2: entry live; create infos and every
        // pointer they reference (names, layer/extension arrays,
        // callback user data) outlive this call; the allocation
        // callbacks' ledger outlives the instance (owned by the spine).
        let instance = unsafe {
            entry.create_instance(&create_info, Some(&crate::vk::alloc::callbacks(&ledger)))
        }
        .map_err(|code| match code {
            vk::Result::ERROR_OUT_OF_HOST_MEMORY => DeviceError::OutOfHostMemory {
                call: "vkCreateInstance",
            },
            other => creation("vkCreateInstance", other),
        })?;

        let debug = if validation_on {
            let utils = ash::ext::debug_utils::Instance::new(&entry, &instance);
            let info = crate::vk::debug::messenger_info(&validation_counters);
            // SAFETY: category 2: instance live; counters outlive the
            // messenger (spine-owned, destroyed before them).
            match unsafe {
                utils.create_debug_utils_messenger(
                    &info,
                    Some(&crate::vk::alloc::callbacks(&ledger)),
                )
            } {
                Ok(messenger) => Some((utils, messenger)),
                Err(code) => {
                    // SAFETY: instance was just created and is unshared.
                    unsafe {
                        instance.destroy_instance(Some(&crate::vk::alloc::callbacks(&ledger)));
                    }
                    return Err(creation("vkCreateDebugUtilsMessengerEXT", code));
                }
            }
        } else {
            None
        };

        // Adapter selection. Deterministic: rank by kind, then lowest
        // deviceID.
        let selected = select_adapter(&instance)
            .inspect_err(|_| teardown_early(&instance, debug.as_ref(), &ledger))?;
        let (physical, adapter, queue_family) = selected;

        // Device extensions: portability subset must be enabled when
        // present (MoltenVK contract); the swapchain extension powers
        // the present path when this build carries it.
        // SAFETY: category 2: instance and physical device live.
        let device_extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|code| {
                teardown_early(&instance, debug.as_ref(), &ledger);
                creation("vkEnumerateDeviceExtensionProperties", code)
            })?;
        let device_has = |name: &core::ffi::CStr| {
            device_extensions
                .iter()
                .any(|ext| ext.extension_name_as_c_str().is_ok_and(|n| n == name))
        };
        let mut device_extension_names: Vec<*const core::ffi::c_char> = Vec::new();
        if device_has(ash::khr::portability_subset::NAME) {
            device_extension_names.push(ash::khr::portability_subset::NAME.as_ptr());
        }
        #[cfg(feature = "present")]
        if device_has(ash::khr::swapchain::NAME) {
            device_extension_names.push(ash::khr::swapchain::NAME.as_ptr());
        }

        let priorities = [1.0f32];
        let queue_info = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(queue_family)
            .queue_priorities(&priorities)];
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let device_info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queue_info)
            .enabled_extension_names(&device_extension_names)
            .push_next(&mut vulkan13);
        // SAFETY: category 2: instance/physical live; all referenced
        // arrays outlive the call; ledger outlives the device.
        let device = unsafe {
            instance.create_device(
                physical,
                &device_info,
                Some(&crate::vk::alloc::callbacks(&ledger)),
            )
        }
        .map_err(|code| {
            teardown_early(&instance, debug.as_ref(), &ledger);
            match code {
                vk::Result::ERROR_OUT_OF_HOST_MEMORY => DeviceError::OutOfHostMemory {
                    call: "vkCreateDevice",
                },
                other => creation("vkCreateDevice", other),
            }
        })?;
        // SAFETY: category 2: device live; the family/index pair was
        // declared in the create info above.
        let queue = unsafe { device.get_device_queue(queue_family, 0) };

        renew_diag::info!(
            target: "renew-rhi",
            "device up: {} ({:?})",
            adapter.name,
            adapter.kind
        );

        Ok(Self {
            shared: Rc::new(DeviceShared {
                device,
                queue,
                queue_family,
                physical,
                debug,
                instance,
                entry,
                adapter,
                lost: Cell::new(false),
                validation: validation_counters,
                ledger,
            }),
        })
    }

    /// The selected adapter.
    #[must_use]
    pub fn adapter(&self) -> &AdapterInfo {
        &self.shared.adapter
    }

    /// Validation-layer activity so far.
    #[must_use]
    pub fn validation_report(&self) -> ValidationReport {
        use core::sync::atomic::Ordering;
        let counters = &self.shared.validation;
        ValidationReport {
            errors: counters.errors.load(Ordering::Relaxed),
            warnings: counters.warnings.load(Ordering::Relaxed),
            first_messages: counters
                .first_messages
                .lock()
                .map(|retained| retained.clone())
                .unwrap_or_default(),
        }
    }

    /// The driver host-allocation ledger.
    #[must_use]
    pub fn host_allocation_stats(&self) -> HostAllocationStats {
        use core::sync::atomic::Ordering;
        let ledger = &self.shared.ledger;
        HostAllocationStats {
            allocations: ledger.allocations.load(Ordering::Relaxed),
            deallocations: ledger.deallocations.load(Ordering::Relaxed),
            reallocations: ledger.reallocations.load(Ordering::Relaxed),
            bytes_in_use: ledger.bytes_in_use.load(Ordering::Relaxed),
            peak_bytes: ledger.peak_bytes.load(Ordering::Relaxed),
        }
    }

    /// Block until the GPU is idle.
    ///
    /// # Errors
    ///
    /// [`DeviceError::DeviceLost`] when the device is or becomes lost.
    pub fn wait_idle(&self) -> Result<(), DeviceError> {
        self.shared.check_lost()?;
        // SAFETY: category 2: device live; externally synchronized by
        // the crate-wide single-thread contract.
        let result = unsafe { self.shared.device.device_wait_idle() };
        match result {
            Ok(()) => Ok(()),
            Err(code) => {
                self.shared.note_result(code);
                if code == vk::Result::ERROR_DEVICE_LOST {
                    Err(DeviceError::DeviceLost)
                } else {
                    Err(creation("vkDeviceWaitIdle", code))
                }
            }
        }
    }
}

fn creation(call: &'static str, code: vk::Result) -> DeviceError {
    DeviceError::Creation {
        call,
        code: code.as_raw(),
    }
}

/// Instance-level cleanup for the failure paths before the spine
/// exists.
fn teardown_early(
    instance: &ash::Instance,
    debug: Option<&(ash::ext::debug_utils::Instance, vk::DebugUtilsMessengerEXT)>,
    ledger: &AllocLedger,
) {
    // SAFETY: category 2: both handles were created above with these
    // callbacks and are unshared on this failure path.
    unsafe {
        if let Some((utils, messenger)) = debug {
            utils.destroy_debug_utils_messenger(
                *messenger,
                Some(&crate::vk::alloc::callbacks(ledger)),
            );
        }
        instance.destroy_instance(Some(&crate::vk::alloc::callbacks(ledger)));
    }
}

type Selected = (vk::PhysicalDevice, AdapterInfo, u32);

fn select_adapter(instance: &ash::Instance) -> Result<Selected, DeviceError> {
    // SAFETY: category 2: instance live.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|code| creation("vkEnumeratePhysicalDevices", code))?;
    let mut best: Option<(Selected, (u8, u32))> = None;
    for physical in physical_devices {
        // SAFETY: category 2: instance and each enumerated device live.
        let properties = unsafe { instance.get_physical_device_properties(physical) };
        let families = unsafe { instance.get_physical_device_queue_family_properties(physical) };
        let Some(queue_family) = families
            .iter()
            .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))
        else {
            continue;
        };
        let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default();
        let mut features2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut vulkan13);
        // SAFETY: category 2: as above; the chained struct outlives the
        // call.
        unsafe { instance.get_physical_device_features2(physical, &mut features2) };
        if vulkan13.dynamic_rendering != vk::TRUE {
            continue;
        }
        if vulkan13.synchronization2 != vk::TRUE {
            continue;
        }
        let kind = match properties.device_type {
            vk::PhysicalDeviceType::DISCRETE_GPU => AdapterKind::DiscreteGpu,
            vk::PhysicalDeviceType::INTEGRATED_GPU => AdapterKind::IntegratedGpu,
            vk::PhysicalDeviceType::VIRTUAL_GPU => AdapterKind::VirtualGpu,
            vk::PhysicalDeviceType::CPU => AdapterKind::SoftwareRasterizer,
            _ => AdapterKind::Other,
        };
        let rank = match kind {
            AdapterKind::DiscreteGpu => 0u8,
            AdapterKind::IntegratedGpu => 1,
            AdapterKind::VirtualGpu => 2,
            AdapterKind::SoftwareRasterizer => 3,
            AdapterKind::Other => 4,
        };
        let name = properties.device_name_as_c_str().map_or_else(
            |_| String::from("(unnamed adapter)"),
            |n| n.to_string_lossy().into_owned(),
        );
        let info = AdapterInfo {
            name,
            kind,
            driver_version: properties.driver_version,
            vendor_id: properties.vendor_id,
            device_id: properties.device_id,
        };
        let key = (rank, properties.device_id);
        let candidate = (
            (
                physical,
                info,
                u32::try_from(queue_family).unwrap_or(u32::MAX),
            ),
            key,
        );
        match &best {
            Some((_, best_key)) if *best_key <= key => {}
            _ => best = Some(candidate),
        }
    }
    best.map(|(selected, _)| selected)
        .ok_or(DeviceError::NoSuitableAdapter {
            requirement: "a graphics queue plus dynamic rendering and synchronization2",
        })
}
