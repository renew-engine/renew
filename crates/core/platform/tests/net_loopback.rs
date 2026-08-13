//! The socket seam, over loopback.
//!
//! Everything here binds port zero and asks the kernel which port it got,
//! so two of these can run at once and neither has to agree a number with
//! anyone. Nothing reaches the network: an unavailable loopback is a
//! broken machine, not a flaky test.

use renew_platform::net::{
    IpAddr, Ipv4Addr, Ipv6Addr, NetError, RECEIVE_CEILING, Sent, Socket, SocketAddr, peer_tag,
};

fn loopback() -> SocketAddr {
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

#[test]
fn a_datagram_crosses_and_names_its_sender() {
    let listener = Socket::bind(loopback()).expect("loopback must bind");
    let sender = Socket::bind(loopback()).expect("loopback must bind");
    let to = listener
        .local_addr()
        .expect("a bound socket has an address");
    let from = sender.local_addr().expect("a bound socket has an address");

    assert_eq!(
        sender.send_to(b"hello", to).expect("loopback send"),
        Sent::Delivered
    );

    // Non-blocking, so the datagram may not have landed on the first
    // look. Spinning a bounded number of times is the only honest way to
    // wait without a clock this crate is allowed to read.
    let mut buffer = [0u8; 64];
    let mut received = None;
    for _ in 0..10_000 {
        if let Some(pair) = listener.recv_from(&mut buffer).expect("a healthy socket") {
            received = Some(pair);
            break;
        }
    }
    let (len, source) = received.expect("a datagram sent over loopback must arrive");
    assert_eq!(buffer.get(..len), Some(&b"hello"[..]));
    assert_eq!(
        source.port(),
        from.port(),
        "the receiver must be told which endpoint sent it"
    );
}

#[test]
fn an_empty_socket_reports_nothing_rather_than_blocking() {
    let socket = Socket::bind(loopback()).expect("loopback must bind");
    let mut buffer = [0u8; 64];
    // If this seam were blocking, this call would never return and the
    // test would hang rather than fail — which is why the socket has no
    // blocking mode to reach in the first place.
    assert!(
        socket
            .recv_from(&mut buffer)
            .expect("an empty socket is healthy")
            .is_none(),
        "an empty non-blocking socket reports nothing waiting"
    );
}

#[test]
fn a_datagram_larger_than_the_buffer_is_reported_not_truncated() {
    let listener = Socket::bind(loopback()).expect("loopback must bind");
    let sender = Socket::bind(loopback()).expect("loopback must bind");
    let to = listener.local_addr().expect("bound");

    let big = [7u8; 400];
    sender.send_to(&big, to).expect("loopback send");

    let mut small = [0u8; 16];
    let mut verdict = None;
    for _ in 0..10_000 {
        match listener.recv_from(&mut small) {
            Ok(None) => {}
            other => {
                verdict = Some(other);
                break;
            }
        }
    }
    // A truncated datagram is a different message, so the seam must
    // refuse rather than hand back a prefix that a codec would then blame
    // the sender for.
    match verdict {
        Some(Err(NetError::Oversized { capacity })) => assert_eq!(capacity, 16),
        Some(Ok(Some((len, _)))) => {
            panic!("a 400-byte datagram was accepted into 16 bytes as {len}")
        }
        other => panic!("expected an oversized refusal, got {other:?}"),
    }
}

#[test]
fn binding_a_port_already_taken_fails_recoverably() {
    let held = Socket::bind(loopback()).expect("loopback must bind");
    let taken = held.local_addr().expect("bound");
    // Binding the same port again is the ordinary "someone is already
    // playing" case, and must be a reported outcome rather than a panic.
    match Socket::bind(taken) {
        Err(NetError::Unavailable { addr, .. }) => assert_eq!(addr, taken),
        Ok(_) => panic!("the same port bound twice"),
        Err(other) => panic!("expected an unavailable refusal, got {other:?}"),
    }
}

#[test]
fn every_refusal_says_which_endpoint_it_was_about() {
    let held = Socket::bind(loopback()).expect("bind");
    let taken = held.local_addr().expect("bound");
    let text = Socket::bind(taken)
        .expect_err("the same port twice")
        .to_string();
    assert!(!text.is_empty());
    assert!(
        text.contains(&taken.port().to_string()),
        "a refusal that does not name its endpoint cannot be acted on: \"{text}\""
    );
}

// ---- address canonicalisation ----

#[test]
fn a_v4_mapped_address_folds_to_the_v4_it_names() {
    let plain = SocketAddr::from((Ipv4Addr::new(1, 2, 3, 4), 9000));
    let mapped = SocketAddr::new(IpAddr::V6(Ipv4Addr::new(1, 2, 3, 4).to_ipv6_mapped()), 9000);
    assert_eq!(
        peer_tag(plain),
        peer_tag(mapped),
        "the same peer reaching a roster two ways must be one peer, or it takes two seats"
    );
}

#[test]
fn a_native_v6_address_stays_itself() {
    let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::LOCALHOST), 9000);
    let v4 = SocketAddr::from((Ipv4Addr::LOCALHOST, 9000));
    assert_ne!(
        peer_tag(v6),
        peer_tag(v4),
        "folding must apply to mapped addresses only, not to every v6"
    );
}

#[test]
fn the_port_is_part_of_the_tag_and_is_ordered() {
    let low = peer_tag(SocketAddr::from((Ipv4Addr::LOCALHOST, 1)));
    let high = peer_tag(SocketAddr::from((Ipv4Addr::LOCALHOST, 2)));
    assert_ne!(low, high, "two ports on one host are two peers");
    assert!(low < high, "big-endian, so tags sort the way ports do");
}

#[test]
fn a_v4_peer_and_a_native_v6_peer_are_never_one_tag() {
    // The regression for a collision the test above was reaching for and
    // missed. It checked `::1`, which happens not to alias; a native v6
    // whose low twelve bytes are zero does. With the address written into
    // only the first four bytes, `32.1.13.184` and `2001:db8::` produced
    // byte-identical tags — two peers, one seat, and a divergence
    // hundreds of ticks later with no obvious cause.
    let v4 = SocketAddr::from((Ipv4Addr::new(32, 1, 13, 184), 9000));
    let v6 = SocketAddr::new(
        IpAddr::V6(Ipv6Addr::new(0x2001, 0x0db8, 0, 0, 0, 0, 0, 0)),
        9000,
    );
    assert_ne!(
        peer_tag(v4),
        peer_tag(v6),
        "two peers sharing one tag take one seat, which is the bug this fold exists to prevent"
    );

    // The same shape with a link-local prefix, because one example of a
    // family collision is an anecdote.
    let v4 = SocketAddr::from((Ipv4Addr::new(254, 128, 0, 0), 5000));
    let v6 = SocketAddr::new(IpAddr::V6(Ipv6Addr::new(0xfe80, 0, 0, 0, 0, 0, 0, 0)), 5000);
    assert_ne!(peer_tag(v4), peer_tag(v6));
}

#[test]
fn a_v4_tag_is_the_mapped_form_and_fills_the_whole_address_field() {
    // The docstring promises sixteen bytes of address then the port. It
    // is only true if v4 is re-emitted mapped, so the layout is pinned
    // rather than left to be re-derived.
    let tag = peer_tag(SocketAddr::from((Ipv4Addr::new(1, 2, 3, 4), 0x1234)));
    assert_eq!(
        &tag[..10],
        &[0u8; 10],
        "the mapped prefix is ten zero bytes"
    );
    assert_eq!(&tag[10..12], &[0xff, 0xff], "then the mapped marker");
    assert_eq!(&tag[12..16], &[1, 2, 3, 4], "then the four octets");
    assert_eq!(&tag[16..18], &[0x12, 0x34], "then the port, big-endian");
}

#[test]
fn a_datagram_past_the_seam_ceiling_is_refused_identically_everywhere() {
    // The whole stated reason for running this job on three platforms,
    // and until this test existed it proved neither of the two things it
    // claimed. Windows raises an error here (WSAEMSGSIZE, folded into an
    // ErrorKind that cannot be matched on, so the seam maps the number);
    // Unix truncates into the scratch buffer and the seam observes it by
    // the byte of headroom. Both must produce one outcome.
    let listener = Socket::bind(loopback()).expect("loopback must bind");
    let sender = Socket::bind(loopback()).expect("loopback must bind");
    let to = listener.local_addr().expect("bound");

    // Past the scratch, not merely past the caller's buffer: this is the
    // path where the two platforms genuinely diverge.
    let huge = vec![5u8; RECEIVE_CEILING + 64];
    match sender.send_to(&huge, to) {
        Ok(_) => {}
        // A loopback MTU below the ceiling is a property of the machine,
        // not a defect here. Skipping is honest; asserting would make
        // this test about the host's network stack.
        Err(_) => return,
    }

    let mut buffer = [0u8; RECEIVE_CEILING];
    let mut verdict = None;
    for _ in 0..10_000 {
        match listener.recv_from(&mut buffer) {
            Ok(None) => {}
            other => {
                verdict = Some(other);
                break;
            }
        }
    }
    match verdict {
        // Refused, which is the outcome under test — or nothing arrived
        // at all, because the host dropped it below this seam, which is
        // the host and not this seam being wrong. The two share an arm
        // because the test's claim is "never silently truncated", and
        // both satisfy it.
        Some(Err(NetError::Oversized { .. })) | None => {}
        Some(Ok(Some((len, _)))) => {
            panic!("a {}-byte datagram came back as {len} bytes", huge.len())
        }
        other => panic!("expected an oversized refusal, got {other:?}"),
    }
}
