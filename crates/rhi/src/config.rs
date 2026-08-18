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

/// How the presentation engine will rotate what is drawn.
///
/// **A declaration the renderer must answer, not decoration.** The
/// swapchain is built declaring the transform the surface reports, and
/// the presentation engine then treats the image as already rotated by
/// that much. A caller drawing unrotated content into a chain
/// declaring a quarter turn gets a sideways image at no GPU cost;
/// letting the compositor rotate instead costs real milliseconds on
/// the devices where this is not the identity. So the target reports
/// what it declared, and a renderer folds the counter-rotation into
/// its projection.
///
/// Every desktop surface reports [`SurfaceTransform::Identity`]
/// always. The other three exist because handheld panels have a
/// natural orientation that need not be the one being drawn to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SurfaceTransform {
    /// No rotation: what is drawn is what is shown.
    #[default]
    Identity,
    /// A quarter turn clockwise.
    Rotate90,
    /// A half turn.
    Rotate180,
    /// Three quarters clockwise.
    Rotate270,
}

impl SurfaceTransform {
    /// The turn as quarter-turns clockwise, which is the form a
    /// projection fold wants.
    #[must_use]
    pub const fn quarter_turns(self) -> u8 {
        match self {
            Self::Identity => 0,
            Self::Rotate90 => 1,
            Self::Rotate180 => 2,
            Self::Rotate270 => 3,
        }
    }

    /// Whether the declared transform swaps width and height — the
    /// question a caller sizing anything in surface space actually has.
    #[must_use]
    pub const fn swaps_extent(self) -> bool {
        matches!(self, Self::Rotate90 | Self::Rotate270)
    }
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
    /// The two questions a fold asks, answered for all four rotations.
    /// A table rather than four assertions: the quarter-turn count and
    /// the extent swap are the same fact seen twice, and a mapping that
    /// drifted between them would rotate content one way and size it
    /// the other.
    #[test]
    fn every_rotation_reports_its_turns_and_whether_it_swaps_the_extent() {
        for (transform, turns, swaps) in [
            (SurfaceTransform::Identity, 0, false),
            (SurfaceTransform::Rotate90, 1, true),
            (SurfaceTransform::Rotate180, 2, false),
            (SurfaceTransform::Rotate270, 3, true),
        ] {
            assert_eq!(transform.quarter_turns(), turns, "{transform:?}");
            assert_eq!(transform.swaps_extent(), swaps, "{transform:?}");
            // The swap is exactly the odd turns, stated as the relation
            // rather than as a second list anyone could edit alone.
            assert_eq!(
                transform.swaps_extent(),
                transform.quarter_turns() % 2 == 1,
                "{transform:?}"
            );
        }
    }

    /// The default is the identity, which is what every desktop surface
    /// reports and what a target says while it has no chain at all.
    #[test]
    fn the_default_transform_is_no_rotation() {
        assert_eq!(SurfaceTransform::default(), SurfaceTransform::Identity);
        assert_eq!(SurfaceTransform::default().quarter_turns(), 0);
    }

    use super::*;

    /// The authored-colour constructor decodes each channel and leaves
    /// alpha alone, because alpha was never encoded.
    #[test]
    fn an_authored_colour_is_decoded_channel_by_channel() {
        // The pause menu's panel colour, which is a real constant in this
        // tree rather than a number invented for a test.
        let colour = Color::srgb8([0x28, 0x2c, 0x34]);
        assert_eq!(colour.r.to_bits(), crate::srgb::decode(0x28).to_bits());
        assert_eq!(colour.g.to_bits(), crate::srgb::decode(0x2c).to_bits());
        assert_eq!(colour.b.to_bits(), crate::srgb::decode(0x34).to_bits());
        assert_eq!(
            colour.a.to_bits(),
            1.0f32.to_bits(),
            "opaque by construction"
        );
    }

    /// **The difference this constructor exists to make.** Dividing by
    /// 255 is what the tree did before, and for a mid-grey it is wrong by
    /// more than a factor of two — so a test that only checked "some
    /// number in range" would not have noticed the conversion at all.
    #[test]
    fn dividing_by_255_is_not_the_same_colour() {
        let decoded = Color::srgb8([128, 128, 128]);
        let naive = 128.0 / 255.0;
        assert!(
            decoded.r < naive * 0.55,
            "byte 128 stands for about 21.6% of the light, not {naive}"
        );
    }

    /// Black and white are the transfer function's fixed points, so they
    /// are the two colours that survive any spelling — worth pinning
    /// because they are also the two most likely to be clear values.
    #[test]
    fn the_endpoints_are_the_colours_they_look_like() {
        let black = Color::srgb8([0, 0, 0]);
        assert_eq!(black.r.to_bits(), 0.0f32.to_bits());
        let white = Color::srgb8([255, 255, 255]);
        assert_eq!(white.r.to_bits(), 1.0f32.to_bits());
        assert_eq!(white.a.to_bits(), 1.0f32.to_bits());
    }

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
