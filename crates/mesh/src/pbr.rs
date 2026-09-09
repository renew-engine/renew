//! The material vocabulary glTF uses, which is not the one a material
//! library uses.
//!
//! **There are two material models in this crate and they are not two
//! spellings of one thing.** A material library carries the Phong
//! vocabulary — ambient, diffuse, specular, a specular exponent — because
//! that is what the format stores, and [`crate::mtl::Material`] holds it.
//! A glTF material is metallic-roughness: a base colour, how metallic the
//! surface is, how rough, and a set of maps that modulate those.
//!
//! **Parts of the two do line up.** An emissive colour is an emissive
//! colour in both, and a diffuse colour and a base colour are the same
//! quantity under two names. It is the specular half that has no
//! non-heuristic mapping — and a conversion carrying only the members
//! that do correspond would quietly drop the rest, which is worse than
//! not converting.
//!
//! # Why they are not merged, and never converted
//!
//! Every mapping between the two in circulation is a heuristic. There is
//! no specular exponent that *is* a roughness; there is a formula
//! somebody found acceptable for their renderer. A reader that applied
//! one would hand back a material no file contained, which is the thing
//! this crate refuses to do everywhere else:
//!
//! > **Empty rather than computed.** Deriving a normal here would put a
//! > value in the array that the file did not contain, and a caller
//! > cannot then tell what the exporter said from what this crate
//! > guessed.
//!
//! So: two types, each named for the vocabulary it holds, and a caller
//! that wants one model out of both converts where the conversion can be
//! seen and argued with.
//!
//! # Every member has a default, and they are the format's
//!
//! A material object has **no required members at all** — `{}` is a legal
//! material and means every default. The defaults below are the schema's,
//! not this crate's convenience, which is why [`Material::default`] is
//! the answer to an empty object rather than a separate notion of
//! "unset".
//!
//! # Ranges are refused rather than clamped, and that differs from the
//! material library on purpose
//!
//! [`crate::mtl::Material::shininess`] is deliberately not clamped: the
//! format's usual range is a **convention**, files exceed it, and a
//! reader that clamped would silently change a material rather than
//! report one.
//!
//! glTF's ranges are not a convention. The schema states `minimum` and
//! `maximum` on the factors, so a value outside them is a document that
//! does not conform, and the reader says so and names the member —
//! including the alpha cutoff, which is bounded below whatever the mode
//! does with it. The two readers differ because the two formats differ,
//! not because they disagree about clamping.
//!
//! **The check happens at the document's own width.** A number a hair
//! past the bound narrows to exactly the bound, so a reader that
//! converted first and checked afterwards would clamp while saying it
//! refuses.

/// Which texture a map names, and which coordinate set it reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRef {
    /// The texture's index in the document's own table.
    ///
    /// **Required**, unlike almost everything else here: a map that names
    /// no texture is not a map.
    ///
    /// A `u32` because that is what the format's own id type is, and
    /// because five of these live in every material — widening them
    /// to a pointer would make the size of a material depend on the
    /// target, which is a dependency this crate keeps out of its counts.
    ///
    /// **Bounded, not resolved.** This crate does not read the `textures`
    /// table, so the number is checked against that table's length and no
    /// further: it is known to name a row that exists, and nothing here
    /// can say what is in it.
    pub texture: u32,
    /// Which `TEXCOORD_n` attribute to read it with. Zero by default.
    pub uv_set: u32,
}

/// A normal map, and how far it leans.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalTexture {
    /// The texture and its coordinate set.
    pub map: TextureRef,
    /// The scale applied to the sampled normal's x and y. One by default,
    /// and **unbounded**: the schema states no range for it.
    pub scale: f32,
}

/// An occlusion map, and how strongly it applies.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct OcclusionTexture {
    /// The texture and its coordinate set.
    pub map: TextureRef,
    /// How much of the sampled occlusion to apply, in `0..=1`. One by
    /// default.
    pub strength: f32,
}

/// How a material's alpha is meant to be read.
///
/// **Three answers, and the cutoff belongs to exactly one of them.** A
/// material that is not masked has no cutoff. The format lets a document
/// state the number under any mode it declares and requires two of the
/// three to ignore it — and forbids it outright when no mode is named
/// at all. Putting it on the variant that uses it is what stops a caller
/// reading a threshold that means nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum Alpha {
    /// The default: the alpha channel is ignored and the surface is
    /// fully opaque.
    #[default]
    Opaque,
    /// Alpha is a threshold, and this is where it sits.
    Mask {
        /// At or above this, the surface is opaque; below it, invisible.
        /// Half by default, and bounded below by zero with **no upper
        /// bound** — the schema states only a minimum.
        cutoff: f32,
    },
    /// Alpha composites.
    Blend,
}

/// One material, in glTF's own vocabulary.
///
/// Every field carries the format's default, so a document that says
/// nothing about a material gets [`Material::default`] rather than a
/// refusal — an empty object is legal and means exactly this.
#[derive(Clone, Debug, PartialEq)]
pub struct Material {
    /// What the document called it, if it called it anything.
    ///
    /// **Decoration, not identity.** A material library's names are load
    /// bearing, because that is how a geometry file refers to one; a glTF
    /// document refers to a material by its index. This is carried so a
    /// tool can show a person something better than a number.
    pub name: Option<String>,

    /// Linear multipliers for the base colour, each in `0..=1`.
    /// `[1.0; 4]` by default.
    pub base_color: [f32; 4],
    /// How metallic the surface is, in `0..=1`. One by default.
    pub metallic: f32,
    /// How rough the surface is, in `0..=1`. One by default.
    pub roughness: f32,
    /// Linear multipliers for the emitted colour, each in `0..=1`. Black
    /// by default, which is the format's way of saying "emits nothing".
    pub emissive: [f32; 3],

    /// How to read the alpha channel, and the threshold when that is
    /// what it is.
    pub alpha: Alpha,
    /// Whether the back face is drawn. False by default.
    pub double_sided: bool,

    /// The base colour map.
    pub base_color_map: Option<TextureRef>,
    /// The map carrying metallic in blue and roughness in green.
    ///
    /// **One texture, two channels**, which is the format's arrangement
    /// and not something this crate chose; a reader that split it into
    /// two would be describing a document that does not exist.
    pub metallic_roughness_map: Option<TextureRef>,
    /// The normal map, and its scale.
    pub normal_map: Option<NormalTexture>,
    /// The occlusion map, and its strength.
    pub occlusion_map: Option<OcclusionTexture>,
    /// The emissive map.
    pub emissive_map: Option<TextureRef>,
}

impl Default for Material {
    /// The format's defaults, which are what an empty material means.
    fn default() -> Self {
        Self {
            name: None,
            base_color: [1.0; 4],
            metallic: 1.0,
            roughness: 1.0,
            emissive: [0.0; 3],
            alpha: Alpha::Opaque,
            double_sided: false,
            base_color_map: None,
            metallic_roughness_map: None,
            normal_map: None,
            occlusion_map: None,
            emissive_map: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{Alpha, Material};

    /// **The defaults are the format's, and an empty material is them.**
    ///
    /// Pinned here rather than left to the reader that builds them,
    /// because a default that drifts is a material this crate invented
    /// and nothing would say so.
    #[expect(
        clippy::float_cmp,
        reason = "these are the format's stated defaults, written as literals in both places; a tolerance would let a default drift and still pass, which is the one thing this test exists to prevent"
    )]
    #[test]
    fn the_defaults_are_the_ones_the_format_states() {
        let material = Material::default();
        assert_eq!(material.base_color, [1.0, 1.0, 1.0, 1.0]);
        assert_eq!(material.metallic, 1.0);
        assert_eq!(material.roughness, 1.0);
        assert_eq!(material.emissive, [0.0, 0.0, 0.0]);
        assert_eq!(material.alpha, Alpha::Opaque);
        assert!(!material.double_sided);
        assert!(material.name.is_none());
        assert!(material.base_color_map.is_none());
        assert!(material.metallic_roughness_map.is_none());
        assert!(material.normal_map.is_none());
        assert!(material.occlusion_map.is_none());
        assert!(material.emissive_map.is_none());
    }

    /// A cutoff exists only on the variant that uses one.
    #[test]
    fn only_a_masked_material_carries_a_cutoff() {
        assert_eq!(Alpha::default(), Alpha::Opaque);
        assert_ne!(Alpha::Mask { cutoff: 0.5 }, Alpha::Blend);
        assert_ne!(Alpha::Mask { cutoff: 0.5 }, Alpha::Mask { cutoff: 0.25 });
    }
}
