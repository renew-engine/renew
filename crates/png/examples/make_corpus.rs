//! Write the decoder's seed corpus.
//!
//! **Every input here is written by this program**, so the corpus carries
//! no file this repository did not author and no licence question comes
//! with it. That is the same rule the sample atlases follow.
//!
//! A fuzzer finds its own way past a signature eventually, but it wastes
//! most of a budget doing it. These seeds put it on the far side of each
//! early refusal: the valid images give it a shape to mutate, and each
//! malformed one lands on a different named refusal, so a mutation of it
//! starts from somewhere the random walk rarely reaches. Between them the
//! fourteen seeds reach ten different answers, and the replay test beside
//! the crate holds a floor under that count.
//!
//! Run when the corpus needs regenerating:
//!
//! ```text
//! cargo run -p renew-png --example make_corpus
//! ```
//!
//! Existing files are left alone. The fuzzer adds its own finds to this
//! directory over time, and this program must never delete them.

// The crate bans filesystem access because the library never touches a
// file -- "a caller that writes an image owns the file". This program is
// that caller: writing the corpus is its whole job, and the same
// allowance sits on the replay test beside it for the same reason.
#![allow(clippy::disallowed_methods)]
// And the path-type ban with it: the crate takes bytes and never a path.
#![allow(clippy::disallowed_types)]

use std::path::PathBuf;

/// The format's eight-byte signature, written out here rather than
/// reached for in the crate: this program is test support, and widening a
/// public surface so a generator can borrow a constant would be the worse
/// trade.
const SIGNATURE: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];

/// CRC-32 over the chunk type and payload, the same polynomial the format
/// names. Written here for the reason above — and it has to be right: a
/// seed whose checksum is wrong refuses at the checksum and never reaches
/// the branch it was written to reach, which makes it a duplicate wearing
/// a different name.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = if crc & 1 == 1 { 0xEDB8_8320 } else { 0 };
            crc = (crc >> 1) ^ mask;
        }
    }
    !crc
}

/// A chunk, framed the way the format wants it: length, type, payload,
/// then the checksum over type and payload.
fn chunk(kind: [u8; 4], payload: &[u8]) -> Option<Vec<u8>> {
    let length = u32::try_from(payload.len()).ok()?;
    let mut out = Vec::with_capacity(payload.len() + 12);
    out.extend_from_slice(&length.to_be_bytes());
    out.extend_from_slice(&kind);
    out.extend_from_slice(payload);
    let mut framed = kind.to_vec();
    framed.extend_from_slice(payload);
    out.extend_from_slice(&crc32(&framed).to_be_bytes());
    Some(out)
}

/// A header payload with every field spelled out, so a caller can bend
/// exactly one of them.
fn ihdr(width: u32, height: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(13);
    out.extend_from_slice(&width.to_be_bytes());
    out.extend_from_slice(&height.to_be_bytes());
    out.push(depth);
    out.push(colour);
    out.push(0); // compression method
    out.push(0); // filter method
    out.push(interlace);
    out
}

/// A file that is a signature, one header, and nothing else.
fn header_only(width: u32, height: u32, depth: u8, colour: u8, interlace: u8) -> Vec<u8> {
    let mut out = SIGNATURE.to_vec();
    if let Some(framed) = chunk(*b"IHDR", &ihdr(width, height, depth, colour, interlace)) {
        out.extend_from_slice(&framed);
    }
    out
}

/// A small image whose pixels vary, so the encoder has something to
/// filter rather than a flat run.
fn varied(width: u32, height: u32) -> Vec<u8> {
    let mut pixels = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for y in 0..height {
        for x in 0..width {
            let red = u8::try_from((x * 7 + y * 3) % 256).unwrap_or(0);
            let green = u8::try_from((x * 11 + y * 5) % 256).unwrap_or(0);
            let blue = u8::try_from((x + y * 13) % 256).unwrap_or(0);
            pixels.extend_from_slice(&[red, green, blue, 255]);
        }
    }
    pixels
}

/// The first offset of `needle` in `haystack`, or `None`.
fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// A copy of `png` whose first compressed byte is nonsense and whose
/// enclosing chunk checksum agrees with the edit.
///
/// The checksum is **recomputed**, which is the whole point of this
/// function: without it the file refuses at the checksum and never
/// reaches the inflate at all.
fn corrupt_zlib_header(png: &[u8]) -> Option<Vec<u8>> {
    let mut out = png.to_vec();
    let kind_at = find(&out, b"IDAT")?;
    let length_at = kind_at.checked_sub(4)?;
    let mut length_bytes = [0u8; 4];
    length_bytes.copy_from_slice(out.get(length_at..length_at + 4)?);
    let length = u32::from_be_bytes(length_bytes) as usize;
    let payload_at = kind_at + 4;
    if length == 0 || payload_at + length + 4 > out.len() {
        return None;
    }
    // 0xFF is not a valid zlib compression-method-and-flags byte.
    out[payload_at] = 0xFF;
    let mut framed = out.get(kind_at..kind_at + 4)?.to_vec();
    framed.extend_from_slice(out.get(payload_at..payload_at + length)?);
    let crc = crc32(&framed).to_be_bytes();
    out.get_mut(payload_at + length..payload_at + length + 4)?
        .copy_from_slice(&crc);
    Some(out)
}

fn main() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../fuzz/corpus/png_decode");
    if let Err(error) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {error}", dir.display());
        return;
    }

    let (Ok(one), Ok(small), Ok(wide)) = (
        renew_png::encode(1, 1, &[255, 0, 0, 255]),
        renew_png::encode(4, 4, &varied(4, 4)),
        renew_png::encode(16, 9, &varied(16, 9)),
    ) else {
        eprintln!("the encoder refused a seed image; nothing written");
        return;
    };

    let mut truncated = wide.clone();
    truncated.truncate(wide.len() / 2);

    // One bit flipped inside a chunk's payload, so the framing is intact
    // and only the checksum disagrees.
    let mut bad_crc = small.clone();
    let midpoint = bad_crc.len() / 2;
    bad_crc[midpoint] ^= 0x01;

    let Some(bad_zlib) = corrupt_zlib_header(&small) else {
        eprintln!("could not build the bad-zlib seed; nothing written");
        return;
    };

    let signature_and_noise = {
        let mut out = SIGNATURE.to_vec();
        out.extend_from_slice(&[0xDE, 0xAD, 0xBE, 0xEF, 0x00, 0x11, 0x22, 0x33]);
        out
    };

    // A chunk whose declared length runs off the end of the file.
    let overrun = {
        let mut out = SIGNATURE.to_vec();
        out.extend_from_slice(&0xFFFF_FF00u32.to_be_bytes());
        out.extend_from_slice(b"IDAT");
        out.extend_from_slice(&[0; 8]);
        out
    };

    let seeds: [(&str, Vec<u8>); 14] = [
        ("valid-1x1.png", one),
        ("valid-4x4.png", small),
        ("valid-16x9.png", wide),
        ("truncated.png", truncated),
        ("bad-checksum.png", bad_crc),
        ("bad-zlib-header.png", bad_zlib),
        ("not-a-png.bin", b"this file is not a png at all".to_vec()),
        ("empty.bin", Vec::new()),
        ("signature-only.png", SIGNATURE.to_vec()),
        ("signature-and-noise.png", signature_and_noise),
        ("chunk-overruns.png", overrun),
        ("zero-width.png", header_only(0, 4, 8, 6, 0)),
        ("interlaced.png", header_only(4, 4, 8, 6, 1)),
        ("depth-one.png", header_only(4, 4, 1, 6, 0)),
    ];

    let mut written = 0usize;
    let mut kept = 0usize;
    for (name, bytes) in seeds {
        let path = dir.join(name);
        if path.exists() {
            kept += 1;
            continue;
        }
        if let Err(error) = std::fs::write(&path, &bytes) {
            eprintln!("{}: {error}", path.display());
            continue;
        }
        written += 1;
    }
    println!("{written} written, {kept} already present");
}
