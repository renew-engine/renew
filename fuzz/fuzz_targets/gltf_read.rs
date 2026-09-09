//! A whole glTF asset, against bytes nobody wrote on purpose.
//!
//! **The one target in this tranche whose input needs no encoding
//! invented for it.** A glTF asset is a file -- a container, or the
//! document on its own -- so the generator writes both shapes, this
//! reads them, and the merge-time replay gate reads the same ones.
//! The accessor target had to carry six parameters in a head because an
//! accessor is not a file — this is what it looks like when the layer
//! under test takes bytes.
//!
//! # What is deliberately not seeded
//!
//! **A node hierarchy containing a cycle.** The reader refuses one, and
//! the refusal is checked by construction — every node is entered at
//! most once. But if that guard were ever removed, a seed carrying a
//! cycle would make this target *hang* rather than fail, and a hang is
//! the one outcome a harness cannot report: the run would stop making
//! progress with nothing to show for it, here and in the merge gate
//! alike.
//!
//! That is not a guess. Probing the mutation locally did exactly that —
//! one test failed, the next stopped, and a timeout ended the run. So
//! the cycle is pinned by a deterministic test beside the crate, where a
//! wedged run is a failed test instead of a silent stall.
//!
//! # What is asserted
//!
//! Everything a caller relies on and no refusal reveals: geometry that
//! comes back is whole triangles, holds coordinates a bounding box can
//! bound, and has optional arrays whose lengths match the geometry they
//! describe. A reader that returned a mesh with four positions, or with
//! one normal for two triangles, would have answered rather than
//! crashed — and every layer above it would then be reading past the
//! end of something.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::{glb, gltf};

fuzz_target!(|data: &[u8]| {
    // **The tables beyond geometry are read from the same bytes, and
    // separately.**
    // `read` builds geometry and never looks at a material, so a document
    // that reaches this target exercises that table only if something
    // asks for it. Asking here costs one parse and covers a layer the
    // geometry path cannot reach at all -- including for the inputs that
    // are refused below, which is where a table this reader must not
    // trust is likeliest to be.
    tables_answer(data);

    // **The owning entry point, from the raw bytes, doing its own
    // dispatch.** Everything above holds a parse and borrows out of it;
    // `tables` is what a caller uses when it cannot, and it is the only
    // path that *copies* an image. That copy is what a document can
    // amplify -- nothing says two images may not name one buffer view,
    // so a thousand entries can point at one megabyte and the bytes come
    // back a thousand times over. The fuzzer's own memory limit is what
    // catches that, and it can only catch it if something calls this.
    owned_tables_answer(data);

    let Ok(mesh) = gltf::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert_eq!(
        mesh.positions.len() % 3,
        0,
        "geometry that read is whole triangles"
    );
    assert!(!mesh.is_empty(), "an empty read is refused, not returned");
    assert_eq!(mesh.positions.len(), mesh.triangles() * 3);

    // **The optional arrays describe the geometry beside them or are
    // absent.** A mesh carrying one normal for two triangles is the
    // shape every layer above would read past the end of.
    assert!(
        mesh.face_normals.is_empty() || mesh.face_normals.len() == mesh.triangles(),
        "a face normal per triangle or none: {} for {}",
        mesh.face_normals.len(),
        mesh.triangles()
    );
    assert!(
        mesh.corner_normals.is_empty() || mesh.corner_normals.len() == mesh.positions.len(),
        "a normal per corner or none: {} for {}",
        mesh.corner_normals.len(),
        mesh.positions.len()
    );
    assert!(
        mesh.corner_texcoords.is_empty() || mesh.corner_texcoords.len() == mesh.positions.len(),
        "a coordinate per corner or none: {} for {}",
        mesh.corner_texcoords.len(),
        mesh.positions.len()
    );

    // **Every array, not just the positions.** A node transform moves
    // normals through a different matrix from the one it moves positions
    // through, so a transform large enough to overflow can produce a
    // value in one array and not the other.
    for value in mesh
        .positions
        .iter()
        .chain(&mesh.face_normals)
        .chain(&mesh.corner_normals)
        .flatten()
    {
        assert!(
            value.is_finite(),
            "a coordinate nothing downstream can bound reached a caller"
        );
    }
    for value in mesh.corner_texcoords.iter().flatten() {
        assert!(
            value.is_finite(),
            "a texture coordinate nothing downstream can bound reached a caller"
        );
    }

    // Reading twice answers the same, which is what makes a recorded
    // corpus mean anything: a reader whose answer depended on anything
    // but its input could not be reasoned about from bytes at all.
    let again = gltf::read(data).expect("what read once reads again");
    assert_eq!(again, mesh, "the same bytes read to the same geometry");
});

/// Read the material and image tables, and hold each to what the format
/// states.
///
/// **It costs about 44% more per input**, measured over the committed
/// seeds: a third `Source` is built where geometry already builds two,
/// which decodes every buffer's payload again. Paid deliberately -- the
/// tables it reaches are unreachable from the geometry path at any
/// price, and a table nothing attacks is a table nothing has checked.
///
/// Separate from the geometry above because the two share only their
/// bytes: a document may carry materials and no geometry, or the reverse,
/// and a target that only asked for one would leave the other's
/// arithmetic unattacked.
///
/// **Both shapes, not just the loose document.** An earlier version
/// parsed the raw bytes, so every container went straight past it -- and
/// a container is what most of this corpus is, and what most real assets
/// are. The dispatch here is the reader's own.
fn tables_answer(data: &[u8]) {
    let container = glb::looks_like(data).then(|| glb::read(data)).transpose();
    let Ok(container) = container else {
        // The container layer's own refusals are the geometry half's
        // business; a malformed wrapper has no document to ask about.
        return;
    };
    let (document, chunk) = container.map_or((data, None), |read| (read.json, read.binary));

    let Ok(json) = gltf::parse(document) else {
        return;
    };
    let root = json.root();

    let Ok(materials) = gltf::materials(root) else {
        return;
    };

    for material in &materials {
        // **Every factor the format bounds, still inside its bound.** A
        // reader that let one through would be handing a renderer a
        // multiplier the document was refused for stating.
        for component in material
            .base_color
            .iter()
            .chain(&material.emissive)
            .chain(core::slice::from_ref(&material.metallic))
            .chain(core::slice::from_ref(&material.roughness))
        {
            assert!(
                (0.0..=1.0).contains(component),
                "a factor outside the range the format states reached a caller: {component}"
            );
        }

        // A cutoff is bounded below; the format states no upper bound,
        // so none is asserted. Nothing asserts it is finite either --
        // the number layer refuses one that is not, so an assertion here
        // would be defensive code that reads like safety and checks
        // nothing.
        if let renew_mesh::pbr::Alpha::Mask { cutoff } = material.alpha {
            assert!(
                cutoff >= 0.0,
                "a cutoff below the stated minimum reached a caller: {cutoff}"
            );
        }

        if let Some(occlusion) = material.occlusion_map {
            assert!(
                (0.0..=1.0).contains(&occlusion.strength),
                "an occlusion strength outside its stated range"
            );
        }

        // **Every index is inside the table it names.** This reader does
        // not resolve a texture, but it bounds one, and an index past the
        // table reaching a caller is the fault nothing downstream can
        // catch.
        let textures = root.get("textures").map_or(0, renew_json::Value::len);
        for map in [material.base_color_map, material.metallic_roughness_map]
            .into_iter()
            .flatten()
            .chain(material.normal_map.map(|normal| normal.map))
            .chain(material.occlusion_map.map(|occlusion| occlusion.map))
            .chain(material.emissive_map)
        {
            assert!(
                (map.texture as usize) < textures,
                "a texture index past the table reached a caller: {} of {textures}",
                map.texture
            );
        }
    }

    // **The pairing, over every mesh the document has.** It takes an
    // index the caller chooses and reads a table the document controls,
    // which is exactly the shape worth attacking, and nothing reached it
    // before.
    let meshes = root.get("meshes").map_or(0, renew_json::Value::len);
    for mesh in 0..meshes {
        let Ok(pairing) = gltf::primitive_materials(root, mesh) else {
            continue;
        };
        for named in pairing.into_iter().flatten() {
            assert!(
                (named as usize) < materials.len(),
                "a material index past the table reached a caller: {named} of {}",
                materials.len()
            );
        }
    }

    // **The image table, from the same document.** It reaches for
    // buffer bytes the way an accessor does and decodes payloads the way
    // a buffer does, so it is where those two layers meet under a table
    // that has no geometry in it at all.
    if let Ok(source) = gltf::Source::of(root, chunk)
        && let Ok(images) = gltf::images(root, &source)
    {
        // **Both halves, because a container has two.** An image named by
        // a view is served out of the binary chunk, which is a separate
        // slice from the JSON this document arrived as -- so bounding the
        // bytes by the JSON alone would fire on a well-formed GLB whose
        // chunk is larger than its document, and a crash that is the
        // harness's own arithmetic is the expensive kind of crash.
        let carried = document.len() + chunk.map_or(0, <[u8]>::len);
        for image in &images {
            assert!(
                image.bytes.len() <= carried,
                "an image larger than the bytes it came from: {} of {carried}",
                image.bytes.len(),
            );
        }

        let again = gltf::images(root, &source).expect("what read once reads again");
        assert_eq!(again, images, "the same bytes read to the same images");
    }

    // Reading twice answers the same, as everywhere else here.
    let again = gltf::materials(root).expect("what read once reads again");
    assert_eq!(again, materials, "the same bytes read to the same materials");
}

/// Read the owning tables, and hold them to the borrowing ones.
///
/// Separate from the block above because it starts from bytes rather
/// than from a parse: it repeats the container dispatch on purpose, so
/// that the dispatch itself is attacked and not merely the readers
/// underneath it.
fn owned_tables_answer(data: &[u8]) {
    let Ok(counted) = gltf::tables(data, gltf::ImageBytes::Counted) else {
        // A refusal is an answer, as everywhere else in this target.
        return;
    };

    // **Counting states the length without holding the bytes**, which is
    // the whole point of the distinction: a caller reporting what a
    // model carries pays nothing for images it will never write.
    for image in &counted.images {
        assert!(
            image.bytes.is_none(),
            "an image was counted and its bytes were kept anyway"
        );
    }

    let Ok(kept) = gltf::tables(data, gltf::ImageBytes::Kept) else {
        // Keeping can refuse where counting does not: the copy answers to
        // a ceiling and the count does not need to.
        return;
    };

    assert_eq!(
        kept.materials, counted.materials,
        "the same bytes read to the same materials"
    );
    assert_eq!(
        kept.textures, counted.textures,
        "and to the same texture table"
    );
    assert_eq!(
        kept.images.len(),
        counted.images.len(),
        "and to the same number of images"
    );
    for (kept, counted) in kept.images.iter().zip(&counted.images) {
        assert_eq!(kept.name, counted.name);
        assert_eq!(kept.media_type, counted.media_type);
        // **The length is the same whether or not the bytes were kept**,
        // which is what lets a report be built from the cheap half.
        assert_eq!(
            kept.len, counted.len,
            "one image measured two different lengths"
        );
        assert_eq!(
            kept.bytes.as_ref().map(Vec::len),
            Some(kept.len),
            "an image kept a different number of bytes than it measured"
        );
    }
}
