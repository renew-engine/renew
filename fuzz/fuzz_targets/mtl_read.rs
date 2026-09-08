//! The MTL reader, against bytes nobody wrote on purpose.
//!
//! A material library is the smallest untrusted surface in this crate
//! and the one whose failure is least visible. Nothing here indexes
//! anything, nothing multiplies, and no number sizes an allocation — so
//! the ways this reader can go wrong are not the ways the geometry
//! readers can. They are quieter: **a value attached to the wrong
//! material, or a number that is not a number reaching a renderer as
//! one.**
//!
//! So what this target asserts is the shape of what comes back rather
//! than its size. Every material a library yields has finite factors, and
//! a library that reads at all has a material in it. The rest of the
//! interesting inputs are structural, and the seeds start the fuzzer
//! near them: a property before the first `newmtl`, a `map_*` line whose
//! options run to the end, a `newmtl` with no name to be referred to by.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::mtl;

fuzz_target!(|data: &[u8]| {
    let Ok(library) = mtl::read(data) else {
        // A refusal is an answer. Which refusal is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert!(
        !library.is_empty(),
        "a library that read declares a material"
    );

    for material in &library {
        for colour in [
            material.ambient,
            material.diffuse,
            material.specular,
            material.emissive,
        ]
        .into_iter()
        .flatten()
        {
            for value in colour {
                assert!(
                    value.is_finite(),
                    "a colour component nothing downstream can bound reached a caller"
                );
            }
        }
        for value in [material.shininess, material.opacity].into_iter().flatten() {
            assert!(
                value.is_finite(),
                "a factor nothing downstream can bound reached a caller"
            );
        }
        // Every name is a run of bytes copied out of the input, so a
        // library cannot ask for more memory than it spends. The same
        // claim the OBJ target makes about `mtllib` names, checked here
        // for the ones a material carries.
        let named: usize = material.name.len()
            + material
                .maps
                .iter()
                .map(|map| map.name.len())
                .sum::<usize>();
        assert!(
            named <= data.len(),
            "{named} bytes of names came out of {} bytes of file",
            data.len()
        );
    }
});
