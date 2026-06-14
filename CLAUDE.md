# CLAUDE.md

Project-specific instructions for Claude. Global rules in `~/.claude/CLAUDE.md` still apply.

## Architecture

A serde-native Rust library for Apple property lists (XML, binary, OpenStep, GNUStep). A single library crate, edition 2024, MSRV 1.88. Codecs sit behind feature flags (`serde`, `xml`, `binary`, `openstep`), all on by default; the always-compiled core is `Format` / `Error` / `Uid` / `Value`. The design is specified in the local `rfc2rust/` RFCs (0001–0007).

## Development Workflow

```bash
cargo +nightly fmt --all                                              # format (nightly-only rustfmt options)
cargo clippy --workspace --all-targets --all-features -- -D warnings  # lint
cargo test  --workspace --all-features                                # test
cargo build --no-default-features                                     # verify feature gates compile bare
cargo deny check                                                      # supply-chain
```

Formatting requires nightly rustfmt — `rustfmt.toml` uses `group_imports` / `imports_granularity`, which are nightly-only. Only formatting needs nightly; everything else (build, test, MSRV) stays on stable.

CI runs all of these on Ubuntu and macOS. A PR must pass them. The lint set denies bare `#[allow]`: every suppression must be `#[expect(<lints>, reason = "...")]`, listing only the lints that actually fire (an unfulfilled `expect` is itself an error). In tests that means a module-level `#![expect(clippy::unwrap_used, clippy::expect_used, reason = "...")]` with whatever the test code triggers.

## Core Rules

- No `unsafe` (`unsafe_code = forbid`).
- Library code uses `Result`, not `panic!` / `unwrap` / `expect`. Crafted input returns `Err`, never panics.
- Match the established plist edge-case behavior — same input, same output — except for deliberate, documented divergences (e.g. dictionaries preserve insertion order).
- Every dependency must earn its place with a written justification (RFC 0007 §2).
- MSRV 1.88 is part of the semver contract; bumping it is a deliberate commit.
- New root-level files must be added to `.gitignore` (whitelist mode — the root is ignored by default).
