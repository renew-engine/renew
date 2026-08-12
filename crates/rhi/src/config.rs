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

    /// An opaque colour written the way it was authored: three
    /// sRGB-encoded bytes, decoded to the linear values this type holds.
    ///
    /// **Prefer this to dividing by 255.** A hex literal picked in an
    /// image editor is sRGB-encoded, so `k / 255.0` does not produce the
    /// light that colour stands for — it produces roughly its square
    /// root, and everything computed from it is wrong by that much.
    ///
    /// Alpha is not taken here because alpha is not encoded: coverage is
    /// already linear, and there is no encoded alpha byte to decode.
    /// Build a translucent colour with [`Color::new`], decoding each
    /// colour channel through [`crate::srgb::decode`] and leaving alpha
    /// as the fraction it already is.
    #[must_use]
    pub fn srgb8(rgb: [u8; 3]) -> Self {
        Self {
            r: crate::srgb::decode(rgb[0]),
            g: crate::srgb::decode(rgb[1]),
            b: crate::srgb::decode(rgb[2]),
            a: 1.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_default_device_is_named_and_unvalidated() {
        let desc = DeviceDesc::default();
        assert_eq!(desc.app_name, "renew");
        // Validation costs frame time, so it is opt-in: a caller that
        // says nothing gets the shipping configuration, not the test one.
        assert_eq!(desc.validation, Validation::Off);
    }

    #[test]
    fn color_channels_keep_their_rgba_order() {
        // Struct equality, not four float comparisons: the derived impl
        // is what every caller's assertion goes through anyway.
        assert_eq!(
            Color::new(0.25, 0.5, 0.75, 1.0),
            Color {
                r: 0.25,
                g: 0.5,
                b: 0.75,
                a: 1.0,
            }
        );
    }
}
