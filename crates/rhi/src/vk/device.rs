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

/// The device-lost poison: once set, it never clears, and every later
/// operation on the device fails fast. Extracted as its own type so the
/// poison protocol has direct unit coverage — a real device loss is not
/// reliably inducible on any test adapter.
#[derive(Debug, Default)]
pub(crate) struct PoisonFlag(Cell<bool>);

impl PoisonFlag {
    pub(crate) fn poisoned(&self) -> bool {
        self.0.get()
    }

    /// Record a raw result; poisons on device loss and reports whether
    /// this result was one.
    pub(crate) fn note(&self, result: vk::Result) -> bool {
        if result == vk::Result::ERROR_DEVICE_LOST {
            self.0.set(true);
            true
        } else {
            false
        }
    }
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
    /// Owns the loaded Vulkan library: must outlive every dispatch,
    /// including `vkDestroyInstance` in Drop. Only the presentation
    /// path reads it after bring-up, hence the headless allow.
    #[cfg_attr(not(feature = "present"), allow(dead_code))]
    pub(crate) entry: ash::Entry,
    pub(crate) adapter: AdapterInfo,
    /// The depth attachment format chosen at bring-up, `None` when the
    /// adapter offers no format in the chain. Consulted by pipeline and
    /// target creation; queried once because format support is a static
    /// property of the physical device.
    pub(crate) depth_format: Option<vk::Format>,
    /// The one descriptor-set layout every sampled binding shares: a
    /// single combined image sampler at binding zero, fragment stage.
    /// Owned here because layout identity is what makes any written
    /// set compatible with any pipeline that declares the slot — one
    /// object, created at bring-up, destroyed at teardown before the
    /// device.
    pub(crate) sampled_set_layout: vk::DescriptorSetLayout,
    /// `maxImageDimension2D`, read once at bring-up.
    ///
    /// Kept for the same reason `depth_format` is: it is a static
    /// property of the physical device, so querying it per resource
    /// would ask the driver a question whose answer cannot change.
    /// Creation paths compare against it and refuse by name, rather
    /// than handing an over-limit extent to `vkCreateImage` where it
    /// becomes a usage violation -- which reddens every lane that
    /// asserts zero validation errors, and reports the caller's mistake
    /// as the engine's.
    pub(crate) max_image_dimension_2d: u32,
    pub(crate) lost: PoisonFlag,
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
        if self.lost.poisoned() {
            Err(DeviceError::DeviceLost)
        } else {
            Ok(())
        }
    }

    /// Record a raw result, poisoning the device on loss.
    pub(crate) fn note_result(&self, result: vk::Result) {
        self.lost.note(result);
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
            self.device
                .destroy_descriptor_set_layout(self.sampled_set_layout, Some(&self.alloc_cbs()));
            self.device.destroy_device(Some(&self.alloc_cbs()));
            if let Some((utils, messenger)) = self.debug.take() {
                utils.destroy_debug_utils_messenger(messenger, Some(&self.alloc_cbs()));
            }
            self.instance.destroy_instance(Some(&self.alloc_cbs()));
        }
        // Destruction-time validation messages (the layer's leak check
        // at vkDestroyInstance) arrive through the create-info-chained
        // callback and land in these counters, which are still alive
        // here. No caller exists any more, so the finding is surfaced
        // the only way left: the diag error channel.
        let errors = self
            .validation
            .errors
            .load(core::sync::atomic::Ordering::Relaxed);
        if errors > 0 {
            renew_diag::error!(
                target: "renew-rhi",
                "validation reported {errors} error(s) by instance destruction"
            );
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
        let entry = unsafe { ash::Entry::load() }.map_err(loader_unavailable)?;

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

        // Interior NULs cannot cross the C boundary; strip them rather
        // than silently reporting an empty name.
        let app_name = std::ffi::CString::new(desc.app_name.replace('\0', "")).unwrap_or_default();
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
        if validation_on {
            // Synchronization validation rides along whenever the layer
            // is active at all: hazard coverage must not depend on
            // which policy found the layer, or the presentation path
            // only ever sees it on machines that demanded it.
            create_info = create_info.push_next(&mut validation_features);
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
            // A loader with no driver behind it (bare CI runners): the
            // same "no usable GPU runtime" seam as a missing loader.
            vk::Result::ERROR_INCOMPATIBLE_DRIVER => DeviceError::LoaderUnavailable {
                message: "the loader found no compatible Vulkan driver".to_string(),
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

        // SAFETY: category 2: instance and selected physical device
        // live; the query has no failure mode.
        let depth_format = select_depth_format(|format| unsafe {
            instance.get_physical_device_format_properties(physical, format)
        });

        // SAFETY: category 2: instance and physical device live; the
        // query has no failure mode and fills a caller-owned struct.
        let limits = unsafe { instance.get_physical_device_properties(physical) }.limits;
        let max_image_dimension_2d = limits.max_image_dimension2_d;

        // SAFETY: category 2: instance and physical device live.
        let device_extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
            .map_err(|code| {
                teardown_early(&instance, debug.as_ref(), &ledger);
                creation("vkEnumerateDeviceExtensionProperties", code)
            })?;
        let device_extension_names: Vec<*const core::ffi::c_char> =
            wanted_device_extensions(&device_extensions)
                .iter()
                .map(|name| name.as_ptr())
                .collect();

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

        // The crate's one sampled-binding set layout, created with the
        // device because it lives and dies with it.
        let bindings = [vk::DescriptorSetLayoutBinding::default()
            .binding(0)
            .descriptor_type(vk::DescriptorType::COMBINED_IMAGE_SAMPLER)
            .descriptor_count(1)
            .stage_flags(vk::ShaderStageFlags::FRAGMENT)];
        let layout_info = vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings);
        // SAFETY: category 2: device live; the binding array is a local
        // outliving the call.
        let sampled_set_layout = unsafe {
            device.create_descriptor_set_layout(
                &layout_info,
                Some(&crate::vk::alloc::callbacks(&ledger)),
            )
        }
        .map_err(|code| {
            // SAFETY: device live, created just above with these same
            // callbacks; nothing else references it yet.
            unsafe { device.destroy_device(Some(&crate::vk::alloc::callbacks(&ledger))) };
            teardown_early(&instance, debug.as_ref(), &ledger);
            match code {
                vk::Result::ERROR_OUT_OF_HOST_MEMORY => DeviceError::OutOfHostMemory {
                    call: "vkCreateDescriptorSetLayout",
                },
                other => creation("vkCreateDescriptorSetLayout", other),
            }
        })?;

        renew_diag::info!(
            target: "renew-rhi",
            "device up: {} ({:?}), depth format {}",
            adapter.name,
            adapter.kind,
            depth_format.map_or("unsupported", depth_name)
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
                depth_format,
                sampled_set_layout,
                max_image_dimension_2d,
                lost: PoisonFlag::default(),
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

    /// The depth attachment format chosen at bring-up, as a diagnostic
    /// name — `None` when no format in the chain offers optimal-tiling
    /// depth-stencil attachment use on this adapter. Depth-free
    /// rendering is unaffected by a `None`.
    #[must_use]
    pub fn depth_format_name(&self) -> Option<&'static str> {
        self.shared.depth_format.map(depth_name)
    }

    /// Whether the validation layer is actually active on this device
    /// — the strict test lanes assert this so their validation oracle
    /// can never go silently vacuous.
    #[must_use]
    pub fn validation_active(&self) -> bool {
        self.shared.debug.is_some()
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

/// The no-Vulkan-runtime seam: the one bring-up failure that is about
/// the machine rather than the request, so the loader's own words are
/// carried through verbatim instead of replaced by ours.
///
/// Takes any message source rather than `ash::LoadingError`, which
/// cannot be constructed outside ash — this way the mapping is provable
/// even though the loading failure itself is reachable only on a
/// machine with no Vulkan runtime installed.
fn loader_unavailable(error: impl core::fmt::Display) -> DeviceError {
    DeviceError::LoaderUnavailable {
        message: error.to_string(),
    }
}

/// The device extensions v0 asks the driver to enable, from what the
/// adapter reports.
///
/// Pure so the portability arm is provable: it exists for the
/// `MoltenVK` contract — the portability subset must be enabled
/// wherever it is exposed — and no Windows or Linux adapter exposes it.
fn wanted_device_extensions(
    available: &[vk::ExtensionProperties],
) -> Vec<&'static core::ffi::CStr> {
    let offers = |name: &core::ffi::CStr| {
        available
            .iter()
            .any(|ext| ext.extension_name_as_c_str().is_ok_and(|n| n == name))
    };
    let mut wanted = Vec::new();
    if offers(ash::khr::portability_subset::NAME) {
        wanted.push(ash::khr::portability_subset::NAME);
    }
    // The swapchain extension powers the present path when this build
    // carries it.
    #[cfg(feature = "present")]
    if offers(ash::khr::swapchain::NAME) {
        wanted.push(ash::khr::swapchain::NAME);
    }
    wanted
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

/// The v0 adapter requirement, as a pure predicate over what the driver
/// reported: a graphics queue family, plus the two Vulkan 1.3 features
/// every recorded frame uses. `None` means "not usable here".
///
/// Pure on purpose. A test machine offers the adapters it happens to
/// have — never one missing dynamic rendering, never a compute-only
/// one — so the rejection arms are provable only away from the driver.
fn graphics_family(
    families: &[vk::QueueFamilyProperties],
    features: &vk::PhysicalDeviceVulkan13Features<'_>,
) -> Option<u32> {
    if features.dynamic_rendering != vk::TRUE || features.synchronization2 != vk::TRUE {
        return None;
    }
    let index = families
        .iter()
        .position(|family| family.queue_flags.contains(vk::QueueFlags::GRAPHICS))?;
    u32::try_from(index).ok()
}

/// The depth formats v0 will accept, best first: `D32_SFLOAT` (the
/// full-float range), then `D24_UNORM_S8_UINT` (the near-universal
/// fallback where D32 attachment support is missing).
const DEPTH_FORMAT_CHAIN: [vk::Format; 2] = [vk::Format::D32_SFLOAT, vk::Format::D24_UNORM_S8_UINT];

/// The chain's diagnostic names; `vk::Format`'s own Debug form covers
/// every format and is not a stable contract, so the two names the
/// chain can produce are spelled out.
fn depth_name(format: vk::Format) -> &'static str {
    match format {
        vk::Format::D32_SFLOAT => "D32_SFLOAT",
        vk::Format::D24_UNORM_S8_UINT => "D24_UNORM_S8_UINT",
        _ => "(outside the chain)",
    }
}

/// Walk [`DEPTH_FORMAT_CHAIN`] and keep the first format whose
/// *optimal-tiling* features include depth-stencil attachment use — the
/// tiling depth images are created with, so linear-only support must
/// not qualify.
///
/// Pure over a properties source for the same reason as
/// [`graphics_family`]: a test adapter supports what it supports —
/// virtually always the whole chain — so the fallback and both-refused
/// arms are provable only away from the driver.
fn select_depth_format(
    properties_of: impl Fn(vk::Format) -> vk::FormatProperties,
) -> Option<vk::Format> {
    DEPTH_FORMAT_CHAIN.into_iter().find(|format| {
        properties_of(*format)
            .optimal_tiling_features
            .contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
    })
}

/// Classify one adapter and produce its selection key: rank by kind
/// first, then by lowest device ID, so selection is deterministic on
/// any machine. Pure for the same reason as [`graphics_family`] — the
/// kind table has five arms and a test machine exhibits one.
fn describe_adapter(properties: &vk::PhysicalDeviceProperties) -> (AdapterInfo, (u8, u32)) {
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
    (info, (rank, properties.device_id))
}

fn select_adapter(instance: &ash::Instance) -> Result<Selected, DeviceError> {
    // SAFETY: category 2: instance live.
    let physical_devices = unsafe { instance.enumerate_physical_devices() }
        .map_err(|code| creation("vkEnumeratePhysicalDevices", code))?;
    // `min_by_key` keeps the first of equal keys. Two identical cards in
    // one machine report the same kind and device ID, so ties DO happen
    // there and fall to enumeration order, which the spec does not
    // promise to keep stable — picking between twins needs a key this
    // API does not expose (PCI location), so v0 states the bound rather
    // than pretending to a determinism it cannot deliver.
    physical_devices
        .into_iter()
        .filter_map(|physical| {
            // SAFETY: category 2: instance and each enumerated device live.
            let properties = unsafe { instance.get_physical_device_properties(physical) };
            // SAFETY: category 2: instance and enumerated device live.
            let families =
                unsafe { instance.get_physical_device_queue_family_properties(physical) };
            let mut vulkan13 = vk::PhysicalDeviceVulkan13Features::default();
            let mut features2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut vulkan13);
            // SAFETY: category 2: as above; the chained struct outlives
            // the call.
            unsafe { instance.get_physical_device_features2(physical, &mut features2) };
            let queue_family = graphics_family(&families, &vulkan13)?;
            let (info, key) = describe_adapter(&properties);
            Some(((physical, info, queue_family), key))
        })
        .min_by_key(|(_, key)| *key)
        .map(|(selected, _)| selected)
        .ok_or(DeviceError::NoSuitableAdapter {
            requirement: "a graphics queue plus dynamic rendering and synchronization2",
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    // The poison protocol, unit-covered: real device loss is not
    // reliably inducible on any test adapter, so the flag's laws are
    // proven here and the call sites route through it exclusively.
    #[test]
    fn poison_sets_only_on_device_loss_and_sticks() {
        let flag = PoisonFlag::default();
        assert!(!flag.poisoned(), "fresh flag is clean");
        assert!(!flag.note(vk::Result::ERROR_OUT_OF_HOST_MEMORY));
        assert!(!flag.poisoned(), "non-loss errors never poison");
        assert!(!flag.note(vk::Result::SUCCESS));
        assert!(flag.note(vk::Result::ERROR_DEVICE_LOST));
        assert!(flag.poisoned(), "device loss poisons");
        assert!(!flag.note(vk::Result::SUCCESS));
        assert!(flag.poisoned(), "poison is sticky");
    }

    // The depth chain, unit-covered: a test adapter virtually always
    // supports the whole chain, so the fallback and both-refused arms
    // would otherwise go unproven.
    #[test]
    fn the_depth_chain_prefers_d32_falls_back_to_d24_and_can_refuse_both() {
        let attachment_for = |supported: &'static [vk::Format]| {
            move |format: vk::Format| {
                let features = if supported.contains(&format) {
                    vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT
                } else {
                    vk::FormatFeatureFlags::empty()
                };
                vk::FormatProperties {
                    optimal_tiling_features: features,
                    ..Default::default()
                }
            }
        };
        assert_eq!(
            select_depth_format(attachment_for(&DEPTH_FORMAT_CHAIN)),
            Some(vk::Format::D32_SFLOAT),
            "both supported: the chain's head wins"
        );
        assert_eq!(
            select_depth_format(attachment_for(&[vk::Format::D24_UNORM_S8_UINT])),
            Some(vk::Format::D24_UNORM_S8_UINT),
            "D32 refused: the fallback is chosen"
        );
        assert_eq!(
            select_depth_format(attachment_for(&[])),
            None,
            "both refused: no format, not a panic"
        );
    }

    #[test]
    fn linear_only_support_never_qualifies_a_depth_format() {
        let linear_only = |_: vk::Format| vk::FormatProperties {
            linear_tiling_features: vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT,
            ..Default::default()
        };
        assert_eq!(
            select_depth_format(linear_only),
            None,
            "depth images are optimal-tiling; linear support is not support"
        );
    }

    #[test]
    fn the_chain_formats_have_stable_diagnostic_names() {
        assert_eq!(depth_name(vk::Format::D32_SFLOAT), "D32_SFLOAT");
        assert_eq!(
            depth_name(vk::Format::D24_UNORM_S8_UINT),
            "D24_UNORM_S8_UINT"
        );
        assert_eq!(
            depth_name(vk::Format::R8G8B8A8_UNORM),
            "(outside the chain)"
        );
    }

    fn family(flags: vk::QueueFlags) -> vk::QueueFamilyProperties {
        vk::QueueFamilyProperties {
            queue_flags: flags,
            queue_count: 1,
            ..Default::default()
        }
    }

    fn properties(
        device_type: vk::PhysicalDeviceType,
        device_id: u32,
        name: &[u8],
    ) -> vk::PhysicalDeviceProperties {
        let mut properties = vk::PhysicalDeviceProperties {
            device_type,
            device_id,
            vendor_id: 0x1002,
            driver_version: 42,
            ..Default::default()
        };
        // `c_char` is signed on some targets and unsigned on others;
        // `try_from` is the one conversion that compiles on both.
        for (cell, byte) in properties.device_name.iter_mut().zip(name) {
            *cell = core::ffi::c_char::try_from(*byte).unwrap_or(0);
        }
        properties
    }

    // The requirement gate, unit-covered: the machine running this test
    // supplies whichever adapters it has, so every rejection arm would
    // otherwise be untested until it silently stopped working.
    #[test]
    fn the_adapter_gate_rejects_what_v0_cannot_render_with() {
        let full = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let families = [
            family(vk::QueueFlags::COMPUTE),
            family(vk::QueueFlags::GRAPHICS | vk::QueueFlags::TRANSFER),
        ];
        assert_eq!(
            graphics_family(&families, &full),
            Some(1),
            "the first graphics-capable family is the chosen one"
        );
        assert_eq!(
            graphics_family(&families[..1], &full),
            None,
            "a compute-only adapter cannot render"
        );
        assert_eq!(graphics_family(&[], &full), None, "no queues, no adapter");
        assert_eq!(
            graphics_family(
                &families,
                &vk::PhysicalDeviceVulkan13Features::default().synchronization2(true)
            ),
            None,
            "dynamic rendering is not optional: every frame uses it"
        );
        assert_eq!(
            graphics_family(
                &families,
                &vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true)
            ),
            None,
            "synchronization2 is not optional: every barrier uses it"
        );
    }

    #[test]
    fn every_adapter_kind_maps_to_its_own_rank() {
        let table = [
            (
                vk::PhysicalDeviceType::DISCRETE_GPU,
                AdapterKind::DiscreteGpu,
                0u8,
            ),
            (
                vk::PhysicalDeviceType::INTEGRATED_GPU,
                AdapterKind::IntegratedGpu,
                1,
            ),
            (
                vk::PhysicalDeviceType::VIRTUAL_GPU,
                AdapterKind::VirtualGpu,
                2,
            ),
            (
                vk::PhysicalDeviceType::CPU,
                AdapterKind::SoftwareRasterizer,
                3,
            ),
            (vk::PhysicalDeviceType::OTHER, AdapterKind::Other, 4),
        ];
        let mut keys = Vec::with_capacity(table.len());
        for (device_type, kind, rank) in table {
            let (info, key) = describe_adapter(&properties(device_type, 9, b"renew adapter"));
            assert_eq!(info.kind, kind, "{device_type:?} classified wrongly");
            assert_eq!(key, (rank, 9), "{device_type:?} ranked wrongly");
            keys.push(key);
        }
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(
            keys, sorted,
            "the table must already be in preference order"
        );
        // Same kind, different adapters: the lower device ID wins, which
        // is what makes selection reproducible across runs.
        let (_, low) = describe_adapter(&properties(vk::PhysicalDeviceType::DISCRETE_GPU, 1, b"a"));
        let (_, high) =
            describe_adapter(&properties(vk::PhysicalDeviceType::DISCRETE_GPU, 2, b"b"));
        assert!(low < high);
    }

    #[test]
    fn adapter_identity_survives_the_description() {
        let (info, _) = describe_adapter(&properties(
            vk::PhysicalDeviceType::DISCRETE_GPU,
            7,
            b"renew reference adapter",
        ));
        assert_eq!(info.name, "renew reference adapter");
        assert_eq!(info.device_id, 7);
        assert_eq!(info.vendor_id, 0x1002);
        assert_eq!(info.driver_version, 42);
    }

    fn extension(name: &core::ffi::CStr) -> vk::ExtensionProperties {
        let mut properties = vk::ExtensionProperties::default();
        for (cell, byte) in properties
            .extension_name
            .iter_mut()
            .zip(name.to_bytes_with_nul())
        {
            *cell = core::ffi::c_char::try_from(*byte).unwrap_or(0);
        }
        properties
    }

    // The MoltenVK rule, unit-covered: an adapter that exposes the
    // portability subset requires it enabled, and no adapter this suite
    // can run against exposes it.
    #[test]
    fn the_portability_subset_is_requested_wherever_it_is_offered() {
        assert_eq!(
            wanted_device_extensions(&[extension(ash::khr::portability_subset::NAME)]),
            [ash::khr::portability_subset::NAME]
        );
        assert!(
            wanted_device_extensions(&[extension(c"VK_KHR_maintenance5")]).is_empty(),
            "nothing v0 needs on offer, nothing requested"
        );
        assert!(wanted_device_extensions(&[]).is_empty());
    }

    #[cfg(feature = "present")]
    #[test]
    fn the_swapchain_extension_is_requested_when_the_adapter_has_it() {
        assert_eq!(
            wanted_device_extensions(&[
                extension(c"VK_KHR_maintenance5"),
                extension(ash::khr::swapchain::NAME),
            ]),
            [ash::khr::swapchain::NAME]
        );
    }

    // The load itself needs a machine with no Vulkan runtime; what the
    // failure turns into does not, and that is the part with a rule:
    // the loader's own explanation reaches the caller intact, because
    // nothing this crate could write in its place would be as useful.
    #[test]
    fn a_loader_failure_carries_the_loaders_own_words() {
        let words = "Cannot load `vkGetInstanceProcAddr` symbol from library";
        // `matches!` rather than a `let … else`: a fallback arm is dead
        // on every passing run, and this form pins the variant too.
        let mapped = loader_unavailable(words);
        assert!(
            matches!(&mapped, DeviceError::LoaderUnavailable { message } if message == words),
            "expected the loader's own words back, got {mapped:?}"
        );
    }

    #[test]
    fn a_name_with_no_terminator_becomes_a_placeholder() {
        let mut unnamed = properties(vk::PhysicalDeviceType::CPU, 0, b"");
        // No NUL anywhere in the array: the driver's name is unreadable
        // as a C string, and a report still needs something to print.
        unnamed.device_name.fill(core::ffi::c_char::MAX);
        assert_eq!(describe_adapter(&unnamed).0.name, "(unnamed adapter)");
    }
}
