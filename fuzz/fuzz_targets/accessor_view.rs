//! An accessor's arithmetic, against parameters nobody chose on purpose.
//!
//! **The first target here whose input is not a file format.** Every
//! other one takes bytes that are a PNG, a WAV, a PLY, a document; an
//! accessor is a byte region *plus six parameters*, and a fuzzer that
//! could only vary the region would never reach one refusal about the
//! parameters. So a fixed head carries them — the encoding is in
//! `crates/mesh/tests/shared/accessor_seed.rs`, included below rather
//! than copied, because the corpus generator and the merge-time replay
//! gate read the same bytes and three copies of one encoding is a
//! defect waiting to happen.
//!
//! **What is being looked for is an offset, not a panic.** The claims
//! below are all about staying inside: the last byte an accessor reads
//! must be inside the region it was given, and every element it says it
//! has must answer. A reader that computed offsets from a stride it
//! never compared against the count would satisfy a bounds check on the
//! region and still read past the elements — which is exactly the fault
//! this layer exists to refuse, and it would not show up as a crash.

#![no_main]

use libfuzzer_sys::fuzz_target;
use renew_mesh::accessor::Component;

#[path = "../../crates/mesh/tests/shared/accessor_seed.rs"]
mod seed;

fuzz_target!(|data: &[u8]| {
    let Some(parsed) = seed::decode(data) else {
        return;
    };
    let Ok(accessor) = parsed.accessor() else {
        // A component code outside the table is an answer.
        return;
    };
    // A view that does not fit its buffer is an answer too, and it is
    // the layer in front: nothing below can tell whether the region it
    // was handed was really there.
    let Ok(region) = parsed.bytes() else {
        return;
    };
    assert!(
        region.len() <= parsed.region.len(),
        "a resolved region cannot be larger than the buffer it came from"
    );

    if parsed.as_indices {
        let Ok(indices) = accessor.indices(region) else {
            return;
        };
        assert_eq!(indices.len(), accessor.count, "a view holds what it accepted");
        assert!(!indices.is_empty(), "an empty accessor is refused, not borrowed");
        for element in 0..indices.len() {
            assert!(
                indices.at(element).is_some(),
                "index {element} of {} is inside this view",
                indices.len()
            );
        }
        assert!(
            indices.at(indices.len()).is_none(),
            "one past the count is outside the view"
        );
        return;
    }

    let Ok(view) = accessor.view(region) else {
        // So is every other refusal. Which one is the suite's business
        // beside the crate; that the call returned at all is this
        // target's.
        return;
    };

    assert_eq!(view.len(), accessor.count, "a view holds what it accepted");
    assert!(!view.is_empty(), "an empty accessor is refused, not borrowed");

    let components = accessor.shape.components();
    // **Every element answers, and one past the end does not.** The
    // second half is the one that matters: a view whose `len` outran
    // what it validated would still answer for the elements inside it.
    for element in 0..view.len() {
        for component in 0..components {
            assert!(
                view.float(element, component).is_some(),
                "element {element} component {component} of {} is inside this view",
                view.len()
            );
        }
        assert!(
            view.float(element, components).is_none(),
            "one component past the shape is outside it"
        );
    }
    assert!(
        view.float(view.len(), 0).is_none(),
        "one element past the count is outside the view"
    );

    // The last byte the accessor addresses is inside the region it was
    // handed. Recomputed here from the accessor's own numbers rather
    // than trusted from the reader, which is the point of asserting it.
    let span = (accessor.count - 1) * accessor.stride();
    let last = accessor.byte_offset + span + accessor.element_size();
    assert!(
        last <= region.len(),
        "this view reaches byte {last} of a {}-byte region",
        region.len()
    );

    // A normalised integer is a fraction of its own range, and the
    // signed types are clamped rather than allowed one step past.
    if accessor.normalized {
        let floor = if matches!(accessor.component, Component::I8 | Component::I16) {
            -1.0
        } else {
            0.0
        };
        for element in 0..view.len() {
            for component in 0..components {
                let Some(value) = view.float(element, component) else {
                    continue;
                };
                assert!(
                    (floor..=1.0).contains(&value),
                    "a normalised component came back as {value}, outside {floor} to 1"
                );
            }
        }
    }

    // Reading twice answers the same, which is what makes a corpus mean
    // anything: a reader whose answer depended on anything but its input
    // could not be reasoned about from recorded bytes at all.
    let again = accessor.view(region).expect("what read once reads again");
    assert_eq!(again, view, "the same claim over the same bytes");
});
