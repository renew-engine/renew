//! The front page's example, held against the crate.
//!
//! A README is not doctested — no crate here includes one as module
//! documentation — so the example on the front page is the one piece of
//! code in the crate that nothing compiles. This test is that example,
//! transcribed, including the claim its comment makes about which tick
//! each frame belongs to. If the API moves, this fails; if the example
//! moves, this must move with it.

use renew_net::{MAX_DATAGRAM_BYTES, PeerId, wire};

#[test]
fn the_front_page_example_compiles_and_says_what_it_claims()
-> Result<(), Box<dyn core::error::Error>> {
    let sender = PeerId::new(0).ok_or("seat zero is always in range")?;
    let header = wire::Header {
        kind: wire::Kind::Inputs,
        sender,
        session: 0x51e3,
    };

    let mut out = [0u8; MAX_DATAGRAM_BYTES];
    let len = wire::write_inputs(&mut out, header, 4_000, 1, 3, &[0b0001, 0b0001, 0b0101])?;

    let wire::Body::Inputs(body) = wire::read(&out[..len])?.body else {
        return Err("an Inputs decoded as something else".into());
    };

    let seen: Vec<(u64, u8)> = body
        .iter()
        .filter_map(|(tick, frame)| frame.first().map(|byte| (tick, *byte)))
        .collect();
    assert_eq!(
        seen,
        vec![(4_000, 0b0001), (4_001, 0b0001), (4_002, 0b0101)],
        "the three pairs the README prints in its comment"
    );
    Ok(())
}
