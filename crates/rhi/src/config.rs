//! Plain-data configuration and info types — no Vulkan vocabulary.

#![deny(unsafe_code)]

/// Device construction parameters.
#[derive(Debug, Clone)]
pub struct DeviceDesc {
    /// Reported to the driver as the application name.
    pub app_name: &'static str,
    pub validation: Validation,
}

impl Default for DeviceDesc {
    fn default() -> Self {
        Self {
            app_name: "renew",
            validation: Validation::Off,
        }
    }
}

/// Validation-layer policy for a device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Validation {
    /// No validation layer.
    Off,
    /// Enable the validation layer when installed; proceed without it
    /// otherwise.
    IfAvailable,
    /// Validation plus synchronization checking, or fail construction —
    /// the setting every test uses.
    Required,
}

/// What kind of adapter the device runs on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum AdapterKind {
    DiscreteGpu,
    IntegratedGpu,
    VirtualGpu,
    /// A CPU rasterizer — load-bearing for the golden tests, which gate
    /// only on a known software adapter.
    SoftwareRasterizer,
    Other,
}

/// The selected adapter, for logs and test gating.
#[derive(Debug, Clone)]
pub struct AdapterInfo {
    pub name: String,
    pub kind: AdapterKind,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
}

/// A render-target size in physical pixels. Zero-sized extents are
/// legal inputs to resize paths (a minimized window) and surface as
/// protocol outcomes, never errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Extent {
    pub width: u32,
    pub height: u32,
}

/// A clear color, linear RGBA in [0, 1].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    #[must_use]
    pub const fn new(r: f32, g: f32, b: f32, a: f32) -> Self {
        Self { r, g, b, a }
    }
}
