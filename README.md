# apple-plist

[![crates.io](https://img.shields.io/crates/v/apple-plist.svg)](https://crates.io/crates/apple-plist)
[![docs.rs](https://img.shields.io/docsrs/apple-plist)](https://docs.rs/apple-plist)
[![CI](https://github.com/moonD4rk/apple-plist/actions/workflows/ci.yml/badge.svg)](https://github.com/moonD4rk/apple-plist/actions/workflows/ci.yml)
[![codecov](https://codecov.io/gh/moonD4rk/apple-plist/branch/main/graph/badge.svg)](https://codecov.io/gh/moonD4rk/apple-plist)
![MSRV](https://img.shields.io/badge/MSRV-1.88-blue.svg)
[![License: Apache-2.0](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

A serde-native Rust library for reading and writing **Apple property lists** in all four dialects — XML, binary (`bplist00`), OpenStep, and GNUStep.

## Why apple-plist?

Most Rust plist crates handle XML and binary. `apple-plist` targets **all four** Apple property-list formats in **both** directions, and handles the edge cases that show up in real-world files: 128-bit integer truncation, lax OpenStep coercion, `CF$UID` keyed-archive aliasing, binary object uniquing, and the three date encodings.

| Format | Read | Write |
|---|:--:|:--:|
| XML | ✅ | ✅ |
| Binary (`bplist00`) | ✅ | ✅ |
| OpenStep (ASCII) | ✅ | ✅ |
| GNUStep | ✅ | ✅ |

Decoding auto-detects the format; encoding picks the format you name.

## Quick start

```rust
use apple_plist::{detect, Format, Value};

fn main() -> Result<(), apple_plist::Error> {
    let bytes = apple_plist::to_vec(&true, Format::Xml)?;

    let value: Value = apple_plist::from_slice(&bytes)?;
    assert_eq!(value, Value::Boolean(true));
    assert_eq!(detect(&bytes), Some(Format::Xml));
    Ok(())
}
```

## Cargo features

All on by default; disable what you don't need. Every combination compiles, and features only add API.

- `serde` — derive-driven (de)serialization.
- `xml` — the XML codec.
- `binary` — the binary (`bplist00`) codec.
- `openstep` — the OpenStep / GNUStep text codec (the two dialects share a parser; OpenStep-versus-GNUStep is a runtime outcome).

## Minimum Supported Rust Version

1.88. Raising it is a deliberate, semver-relevant change.

> **Pre-1.0.** The public API may change before `1.0`.

## License

Licensed under the [Apache License, Version 2.0](LICENSE). See [NOTICE](NOTICE) for third-party attributions.
