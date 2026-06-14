# Contributing

Thanks for considering a contribution. `apple-plist` is a serde-native Rust library for Apple property lists.

## Before You Start

1. **For non-trivial changes, open an issue first.** Anything that adds, removes, or reshapes the public API, changes a default, or alters a wire format should be discussed in an issue before the PR.

2. **Trivial changes are fine to PR directly** — typos, doc fixes, dependency bumps, small refactors.

## Development Workflow

```bash
# Format (nightly-only rustfmt options)
cargo +nightly fmt --all

# Lint (CI uses -D warnings; match this locally)
cargo clippy --workspace --all-targets --all-features -- -D warnings

# Test
cargo test --workspace --all-features

# Build without default features (verifies the feature gates compile bare)
cargo build --no-default-features

# Supply-chain check (requires cargo-deny; install via `cargo install cargo-deny`)
cargo deny check
```

CI runs the same gates on Ubuntu and macOS.

## Coding Conventions

- **No `unsafe` code.** The crate forbids it via the lint set.
- **No `.unwrap()` / `.expect()` / `panic!()` in library code.** Bubble errors via `Result<T, Error>`. Crafted input returns `Err`, never panics. Tests may unwrap (via a file-top `#[expect(...)]`).
- **Match Apple's plist behavior.** Same input → same output, except for deliberate, documented divergences.
- **Every dependency must earn its place.** New dependencies need a written justification, not just convenience.
- **Default to no comments.** Use self-documenting names. Add a comment only when the *why* is non-obvious.

## Commit Messages

- Imperative present tense, ≤ 72 characters in the subject.
- Body explains *why*, not *what* — the diff already shows what.

## License

By contributing, you agree your contribution is licensed under the [Apache License, Version 2.0](LICENSE), same as the project.
