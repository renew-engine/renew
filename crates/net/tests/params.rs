//! What a session refuses before it exists, and what the handshake
//! fingerprint is a function of.

use core::num::NonZeroU64;

use renew_net::{
    INPUT_WINDOW, MAX_INPUT_BYTES, MAX_PEERS, MIN_PEERS, ParamsError, PeerId, SessionParams,
};

#[allow(
    clippy::expect_used,
    reason = "a fixture that cannot be built is a failure of the fixture, and its message is the report"
)]
fn seat(index: u8) -> PeerId {
    PeerId::new(index).expect("in range")
}

#[allow(
    clippy::expect_used,
    reason = "see `seat`: the fixture's own failure is the report"
)]
fn good() -> SessionParams {
    SessionParams {
        peer_count: 4,
        local: seat(2),
        input_bytes: 3,
        input_delay: 2,
        digest_period: 30,
        seed: 99,
        content: 0xc0_ffee,
        rules: 0xba5e,
        session: NonZeroU64::new(7).expect("not zero"),
    }
}

#[test]
fn a_sound_set_of_parameters_validates() {
    let valid = good().validate().expect("these are in range");
    assert_eq!(valid.peer_count(), 4);
    assert_eq!(valid.local(), seat(2));
    assert_eq!(valid.input_bytes(), 3);
    assert_eq!(valid.input_delay(), 2);
    assert_eq!(valid.digest_period(), 30);
    assert_eq!(valid.session().get(), 7);
    assert_eq!(valid.get().seed, 99);
    assert_eq!(valid.roster().count(), 4);
    assert_eq!(
        valid.remotes().count(),
        3,
        "the remotes are the roster less this machine"
    );
    assert!(!valid.remotes().contains(seat(2)));
}

#[test]
fn every_field_out_of_range_is_refused_by_name() {
    for count in [0u8, 1, MAX_PEERS + 1, u8::MAX] {
        let refusal = SessionParams {
            peer_count: count,
            local: seat(0),
            ..good()
        }
        .validate()
        .expect_err("outside the roster range");
        assert_eq!(
            refusal,
            ParamsError::PeerCount {
                saw: count,
                floor: MIN_PEERS,
                ceiling: MAX_PEERS
            }
        );
    }

    let refusal = SessionParams {
        peer_count: 2,
        local: seat(5),
        ..good()
    }
    .validate()
    .expect_err("a seat outside its own roster");
    assert_eq!(
        refusal,
        ParamsError::LocalNotInRoster {
            local: 5,
            peer_count: 2
        }
    );

    for width in [0u8, MAX_INPUT_BYTES + 1] {
        let refusal = SessionParams {
            input_bytes: width,
            ..good()
        }
        .validate()
        .expect_err("outside the width range");
        assert_eq!(
            refusal,
            ParamsError::InputBytes {
                saw: width,
                ceiling: MAX_INPUT_BYTES
            }
        );
    }

    let past = u8::try_from(INPUT_WINDOW).expect("the window fits a byte today");
    let refusal = SessionParams {
        input_delay: past,
        ..good()
    }
    .validate()
    .expect_err("a delay the window cannot buffer");
    assert_eq!(
        refusal,
        ParamsError::InputDelay {
            saw: past,
            window: INPUT_WINDOW
        }
    );

    let refusal = SessionParams {
        digest_period: 0,
        ..good()
    }
    .validate()
    .expect_err("every tick would owe a digest");
    assert_eq!(refusal, ParamsError::DigestPeriodZero);
}

#[test]
fn the_boundaries_themselves_are_legal() {
    // A refusal set that only pins its outside is one that could refuse
    // everything and still pass.
    let past = u8::try_from(INPUT_WINDOW).expect("fits");
    for params in [
        SessionParams {
            peer_count: MIN_PEERS,
            local: seat(1),
            ..good()
        },
        SessionParams {
            peer_count: MAX_PEERS,
            ..good()
        },
        SessionParams {
            input_bytes: 1,
            ..good()
        },
        SessionParams {
            input_bytes: MAX_INPUT_BYTES,
            ..good()
        },
        SessionParams {
            input_delay: 0,
            ..good()
        },
        SessionParams {
            input_delay: past - 1,
            ..good()
        },
        SessionParams {
            digest_period: 1,
            ..good()
        },
    ] {
        assert!(
            params.validate().is_ok(),
            "a boundary value was refused: {params:?}"
        );
    }
}

#[test]
fn every_refusal_names_the_numbers_it_saw() {
    let past = u8::try_from(INPUT_WINDOW).expect("fits");
    let cases = [
        SessionParams {
            peer_count: 1,
            local: seat(0),
            ..good()
        },
        SessionParams {
            peer_count: 2,
            local: seat(7),
            ..good()
        },
        SessionParams {
            input_bytes: 0,
            ..good()
        },
        SessionParams {
            input_delay: past,
            ..good()
        },
        SessionParams {
            digest_period: 0,
            ..good()
        },
    ];
    for params in cases {
        let refusal = params.validate().expect_err("built to be refused");
        let text = refusal.to_string();
        assert!(!text.is_empty(), "{refusal:?} printed nothing");
        if matches!(refusal, ParamsError::DigestPeriodZero) {
            assert!(text.contains("zero"), "expected the word: \"{text}\"");
        } else {
            assert!(
                text.chars().any(|character| character.is_ascii_digit()),
                "{refusal:?} printed no number: \"{text}\""
            );
        }
    }
}

// ---- the agreement fingerprint ----

#[test]
fn the_agreement_digest_covers_everything_two_peers_must_share() {
    let base = good().validate().expect("valid").agreement_digest();
    // Each of these is a genuine disagreement and must move the number.
    for changed in [
        SessionParams {
            seed: 100,
            ..good()
        },
        SessionParams {
            content: 0xc0_ffef,
            ..good()
        },
        SessionParams {
            rules: 0xba5f,
            ..good()
        },
        SessionParams {
            peer_count: 5,
            ..good()
        },
        SessionParams {
            input_bytes: 4,
            ..good()
        },
        SessionParams {
            input_delay: 3,
            ..good()
        },
        SessionParams {
            digest_period: 31,
            ..good()
        },
    ] {
        assert_ne!(
            changed.validate().expect("valid").agreement_digest(),
            base,
            "a parameter two peers must share did not reach the fingerprint: {changed:?}"
        );
    }
}

#[test]
fn the_agreement_digest_excludes_exactly_what_differs_per_peer() {
    let base = good().validate().expect("valid").agreement_digest();

    // The local seat differs per peer by definition. Folding it would make
    // every peer disagree with every other at the handshake.
    let elsewhere = SessionParams {
        local: seat(3),
        ..good()
    };
    assert_eq!(
        elsewhere.validate().expect("valid").agreement_digest(),
        base,
        "the local seat reached the fingerprint, so no two peers could ever agree"
    );

    // The session id cannot reach a confirmed frame — two peers holding
    // different ones drop each other's datagrams at the header — so it is
    // excluded too, and deliberately not derived from the parameters.
    let other_session = SessionParams {
        session: NonZeroU64::new(0xfeed).expect("not zero"),
        ..good()
    };
    assert_eq!(
        other_session.validate().expect("valid").agreement_digest(),
        base
    );
}
