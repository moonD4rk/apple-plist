//! Depth-limit suite: deeply nested documents return
//! [`Error::MaxDepthExceeded`] before the recursive parsers exhaust the stack.
//!
//! Each body runs on a 256 KiB thread (RFC 0005
//! §4) so a regressed guard manifests as a crash on the small stack rather than
//! a silent pass on the roomy main stack. The XML and text parsers
//! increment-then-check (depth 128 parses, 129 errors); the binary parser
//! rejects at container-stack length 128 *before* the push. The `+50`/`+10`
//! margins prove the guard fired, not the exact boundary.

#![expect(
    clippy::unwrap_used,
    reason = "tests assert via unwrap; a failure is the test failing"
)]

use std::thread;

use apple_plist::{Error, MAX_PARSE_DEPTH, Value, from_slice};

/// 256 KiB — small enough that an unguarded recursive parser overflows it,
/// large enough for the bounded (depth-128) parsers to finish (RFC 0005 §4).
const SMALL_STACK: usize = 256 * 1024;

/// Runs `body` on a thread with a deliberately small stack and propagates its
/// panic (an assertion failure) to the test thread.
fn on_small_stack<F: FnOnce() + Send + 'static>(body: F) {
    thread::Builder::new()
        .stack_size(SMALL_STACK)
        .spawn(body)
        .unwrap()
        .join()
        .unwrap();
}

/// `buildDeepBplist(depth)`: a chain of `depth`
/// single-element arrays (`A1` + a 2-byte object ref to the next) terminating
/// in the ASCII string `"x"` (`51 78`), with 2-byte offsets and refs.
fn build_deep_bplist(depth: usize) -> Vec<u8> {
    let num_objects = depth + 1;
    let mut body = b"bplist00".to_vec();

    let mut offsets = Vec::with_capacity(num_objects);
    for i in 0..depth {
        offsets.push(body.len());
        let next = u16::try_from(i + 1).unwrap();
        body.push(0xA1);
        body.extend_from_slice(&next.to_be_bytes());
    }
    offsets.push(body.len());
    body.extend_from_slice(&[0x51, b'x']);

    let offset_table_offset = body.len();
    for off in &offsets {
        body.extend_from_slice(&u16::try_from(*off).unwrap().to_be_bytes());
    }

    let mut trailer = [0u8; 32];
    trailer[6] = 2; // OffsetIntSize
    trailer[7] = 2; // ObjectRefSize
    trailer[8..16].copy_from_slice(&u64::try_from(num_objects).unwrap().to_be_bytes());
    trailer[16..24].copy_from_slice(&0u64.to_be_bytes()); // top object = 0
    trailer[24..32].copy_from_slice(&u64::try_from(offset_table_offset).unwrap().to_be_bytes());
    body.extend_from_slice(&trailer);
    body
}

#[test]
fn max_parse_depth_matches_reference() {
    // The 128 pin must survive somewhere forever.
    assert_eq!(MAX_PARSE_DEPTH, 128);
}

#[test]
fn xml_plist_depth_limit() {
    // n = maxParseDepth + 50.
    on_small_stack(|| {
        let n = MAX_PARSE_DEPTH + 50;
        let mut doc =
            String::from(r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0">"#);
        for _ in 0..n {
            doc.push_str("<array>");
        }
        doc.push_str("<string>x</string>");
        for _ in 0..n {
            doc.push_str("</array>");
        }
        doc.push_str("</plist>");

        let result = from_slice::<Value>(doc.as_bytes());
        assert!(
            matches!(result, Err(Error::MaxDepthExceeded)),
            "expected MaxDepthExceeded, got {result:?}",
        );
    });
}

#[test]
fn text_plist_depth_limit() {
    // n = maxParseDepth + 50.
    on_small_stack(|| {
        let n = MAX_PARSE_DEPTH + 50;
        let mut doc = String::new();
        for _ in 0..n {
            doc.push_str("{a=");
        }
        doc.push('x');
        for _ in 0..n {
            doc.push_str(";}");
        }

        let result = from_slice::<Value>(doc.as_bytes());
        assert!(
            matches!(result, Err(Error::MaxDepthExceeded)),
            "expected MaxDepthExceeded, got {result:?}",
        );
    });
}

#[test]
fn bplist_depth_limit() {
    // depth = maxParseDepth + 10.
    on_small_stack(|| {
        let data = build_deep_bplist(MAX_PARSE_DEPTH + 10);
        let result = from_slice::<Value>(&data);
        assert!(
            matches!(result, Err(Error::MaxDepthExceeded)),
            "expected MaxDepthExceeded, got {result:?}",
        );
    });
}

#[test]
fn bplist_shallow_still_parses() {
    // Guards against an over-aggressive depth/bounds check.
    on_small_stack(|| {
        let data = build_deep_bplist(2);
        let result = from_slice::<Value>(&data);
        assert!(result.is_ok(), "valid shallow bplist failed: {result:?}");
    });
}
