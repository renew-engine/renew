//! The GPU half: a billboard pipeline over the rendering crate, drawing
//! what the pool packed.
//!
//! Behind the `render` feature so the pure half stays device-free. The
//! split point is the packed bytes: the pool writes them, this module
//! rides them to a draw, and nothing else crosses.

use renew_rhi::{
    Blend, Device, Extent, Item, PipelineDesc, PipelineError, RenderPipeline, TargetError,
    TargetFormat, builtin,
};

use crate::INSTANCE_STRIDE;

/// How the billboard combines with what the target holds.
///
/// A crate-local pair rather than the rendering crate's whole blend
/// enum, because these two are the modes that make sense for particles
/// and an opaque particle is a quad pretending: additive for light on
/// light — order-independent, so unsorted batches stay byte-stable —
/// and premultiplied alpha for smoke-like media, accepted unsorted in
/// v0 with the artifact documented where the choice is made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParticleBlend {
    /// `src + dst * (1 - src.a)`: media that occlude.
    Alpha,
    /// `src + dst`: light that accumulates. The recommended mode where
    /// sorting has not been paid for.
    Additive,
}

impl ParticleBlend {
    fn to_rhi(self) -> Blend {
        match self {
            Self::Alpha => Blend::PremultipliedAlpha,
            Self::Additive => Blend::Additive,
        }
    }
}

/// The camera a billboard needs: the matrix, and the basis that makes
/// every quad face it. Ninety-six bytes of push data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraPush {
    bytes: [u8; 96],
}

impl CameraPush {
    /// Pack a view-projection's columns and the camera's right and up.
    ///
    /// Right and up are the eye's own axes — the same vectors a view
    /// matrix is built from — and they need not be renormalized here:
    /// the shader scales them by each particle's size, so a non-unit
    /// basis draws scaled quads, visibly, not unsafely.
    #[must_use]
    pub fn from_parts(columns: [[f32; 4]; 4], right: [f32; 3], up: [f32; 3]) -> Self {
        let mut bytes = [0u8; 96];
        let mut at = 0;
        for column in columns {
            for value in column {
                bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
                at += 4;
            }
        }
        for vector in [right, up] {
            for value in vector {
                bytes[at..at + 4].copy_from_slice(&value.to_ne_bytes());
                at += 4;
            }
            // The block's vec4 padding: written as zero so the pushed
            // bytes are a pure function of the arguments.
            bytes[at..at + 4].copy_from_slice(&0.0f32.to_ne_bytes());
            at += 4;
        }
        Self { bytes }
    }

    /// The packed bytes, exactly the pipeline's declared push range.
    #[must_use]
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
}

/// What can go wrong building the renderer. Creation only: the draw
/// itself cannot fail, and the render belongs to the target.
#[derive(Debug)]
#[non_exhaustive]
pub enum ParticleRenderError {
    /// Building the pipeline — or the sampler it binds, which the
    /// rendering crate reports in the same vocabulary — failed.
    Pipeline(PipelineError),
    /// The atlas texture could not be created.
    Texture(TargetError),
    /// The per-frame instance buffer could not be allocated.
    Buffer(TargetError),
}

impl core::fmt::Display for ParticleRenderError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Pipeline(error) => {
                write!(f, "building the particle pipeline or its sampler: {error}")
            }
            Self::Texture(error) => write!(f, "creating the particle atlas: {error}"),
            Self::Buffer(error) => write!(f, "allocating the particle instance buffer: {error}"),
        }
    }
}

impl std::error::Error for ParticleRenderError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Pipeline(error) => Some(error),
            Self::Texture(error) | Self::Buffer(error) => Some(error),
        }
    }
}

/// Draws a pool's packed instances as camera-facing quads.
///
/// Owns the pipeline, the atlas it samples, and one per-frame buffer
/// sized for the pool's capacity — one buffer, one item per renderer
/// per frame, which is the rendering crate's contract for per-frame
/// bytes. Depth is test-without-write: particles respect the world's
/// surfaces and leave no footprint for each other, which is what makes
/// unsorted additive batches read correctly.
pub struct ParticleRenderer {
    pipeline: RenderPipeline,
    binding: renew_rhi::Binding,
    buffer: renew_rhi::Buffer,
}

impl ParticleRenderer {
    /// Build the pipeline, upload the atlas, and allocate the buffer.
    ///
    /// `atlas_pixels` is **premultiplied** RGBA8, row-major,
    /// `atlas_extent.width * atlas_extent.height * 4` bytes — the same
    /// caller obligation every blending path carries. Bytes that are
    /// not premultiplied composite visibly wrong under
    /// [`ParticleBlend::Alpha`], not unsafely.
    ///
    /// # Errors
    ///
    /// Each creation failure carries its own variant, so a reader is
    /// sent to the thing that refused rather than to a scene nobody
    /// offered.
    pub fn new(
        device: &Device,
        format: TargetFormat,
        atlas_extent: Extent,
        atlas_pixels: &[u8],
        blend: ParticleBlend,
        capacity: u32,
    ) -> Result<Self, ParticleRenderError> {
        let texture = device
            .create_texture(&renew_rhi::TextureDesc::new(atlas_extent, atlas_pixels))
            .map_err(ParticleRenderError::Texture)?;
        let sampler = device
            .create_sampler(&renew_rhi::SamplerDesc::atlas())
            .map_err(ParticleRenderError::Pipeline)?;
        let binding = device
            .create_binding(&renew_rhi::BindingDesc::new(
                renew_rhi::BindingSource::Texture(&texture),
                &sampler,
            ))
            .map_err(ParticleRenderError::Pipeline)?;
        let pipeline = device
            .create_pipeline(
                &PipelineDesc::new(builtin::PARTICLE, format)
                    .instance_input(builtin::PARTICLE_INSTANCE_LAYOUT)
                    .push_constant_size(96)
                    .blend(blend.to_rhi())
                    .sampled_bindings(1)
                    .depth_state(renew_rhi::DepthState::test_only()),
            )
            .map_err(ParticleRenderError::Pipeline)?;
        let buffer = device
            .create_buffer(
                capacity as usize * INSTANCE_STRIDE,
                renew_rhi::BufferUsage::PerFrame,
            )
            .map_err(ParticleRenderError::Buffer)?;
        Ok(Self {
            pipeline,
            binding,
            buffer,
        })
    }

    /// The draw for `live` instances packed in `instances`, seen
    /// through `camera`.
    ///
    /// `instances` is what [`crate::ParticleSystem::write_instances`]
    /// packed — the caller owns the scratch buffer, allocated once
    /// beside the pool. Place the item after the opaque world; place
    /// additive renderers last. A zero-live item is a legal no-op draw,
    /// so an empty pool needs no special case.
    ///
    /// # Panics
    ///
    /// `instances` shorter than `live` records is a contract violation,
    /// asserted by name: the length bounds the copy into the per-frame
    /// buffer, and truncating instead would be a quiet wrong draw.
    #[must_use]
    pub fn item<'a>(&'a self, instances: &'a [u8], live: u32, camera: &'a CameraPush) -> Item<'a> {
        let bytes = live as usize * INSTANCE_STRIDE;
        assert!(
            instances.len() >= bytes,
            "the scratch buffer holds {} bytes and {live} live particles need {bytes}",
            instances.len()
        );
        Item::new(&self.pipeline)
            .frame_data(renew_rhi::FrameData::new(
                &self.buffer,
                &instances[..bytes],
                live,
            ))
            .push_data(camera.bytes())
            .bindings(&[&self.binding])
    }
}

impl core::fmt::Debug for ParticleRenderer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ParticleRenderer").finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The push block's byte layout, deviceless: columns at 0..64,
    /// right at 64..76 with a zeroed pad, up at 80..92 with a zeroed
    /// pad — the exact offsets the shader's std430 block reads. The
    /// golden proves the whole path where a device exists; this pins
    /// the host half everywhere.
    #[test]
    fn the_camera_push_packs_the_documented_offsets() {
        let columns = [
            [1.0, 2.0, 3.0, 4.0],
            [5.0, 6.0, 7.0, 8.0],
            [9.0, 10.0, 11.0, 12.0],
            [13.0, 14.0, 15.0, 16.0],
        ];
        let push = CameraPush::from_parts(columns, [17.0, 18.0, 19.0], [20.0, 21.0, 22.0]);
        let bytes = push.bytes();
        assert_eq!(bytes.len(), 96, "the declared push range");
        let float_at = |index: usize| {
            f32::from_ne_bytes([
                bytes[index * 4],
                bytes[index * 4 + 1],
                bytes[index * 4 + 2],
                bytes[index * 4 + 3],
            ])
        };
        for (index, expected) in (1u8..=16).enumerate() {
            assert_eq!(
                float_at(index).to_bits(),
                f32::from(expected).to_bits(),
                "matrix float {index}"
            );
        }
        for (slot, expected) in [(16, 17.0f32), (17, 18.0), (18, 19.0), (19, 0.0)] {
            assert_eq!(
                float_at(slot).to_bits(),
                expected.to_bits(),
                "right slot {slot}"
            );
        }
        for (slot, expected) in [(20, 20.0f32), (21, 21.0), (22, 22.0), (23, 0.0)] {
            assert_eq!(
                float_at(slot).to_bits(),
                expected.to_bits(),
                "up slot {slot}"
            );
        }
    }

    /// Every error variant says something a reader can act on, and
    /// hands back what it wraps — deviceless, like the sibling
    /// renderer's own error tests, because the mappings are reachable
    /// on every machine even where the calls beneath them are not.
    #[test]
    fn every_error_variant_displays_and_chains() {
        use std::error::Error as _;
        let pipeline = ParticleRenderError::Pipeline(PipelineError::InvalidSpirv {
            stage: "vertex",
            reason: "bad magic",
        });
        let texture = ParticleRenderError::Texture(TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory(atlas)",
        });
        let buffer = ParticleRenderError::Buffer(TargetError::OutOfDeviceMemory {
            call: "vkAllocateMemory(instances)",
        });
        for (error, needle) in [
            (&pipeline, "particle pipeline"),
            (&texture, "particle atlas"),
            (&buffer, "instance buffer"),
        ] {
            let shown = error.to_string();
            assert!(shown.contains(needle), "`{shown}` missing `{needle}`");
            assert!(
                error.source().is_some(),
                "{needle}: the wrapping variant must hand back its cause"
            );
        }
        assert!(
            pipeline
                .source()
                .is_some_and(|cause| cause.to_string().contains("bad magic")),
            "the pipeline refusal must keep the rendering crate's words"
        );
    }

    /// The blend mapping, both arms: the crate-local pair names the
    /// rendering crate's modes it means.
    #[test]
    fn the_blend_mapping_names_what_it_means() {
        assert_eq!(ParticleBlend::Alpha.to_rhi(), Blend::PremultipliedAlpha);
        assert_eq!(ParticleBlend::Additive.to_rhi(), Blend::Additive);
    }
}
