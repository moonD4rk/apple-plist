# Security Policy

## Scope

`apple-plist` parses untrusted input: property-list files from arbitrary sources. A defect that lets a crafted file harm a legitimate user is in scope — for example a parser bug that causes a panic (the library is `panic`-free by contract), an unbounded allocation or non-terminating loop on a small crafted input, or a memory-safety issue (the crate forbids unsafe code crate-wide via the `unsafe_code = "forbid"` lint in `Cargo.toml`, so any such finding is high priority).

A plist that decodes to the values it encodes — even an unusual or adversarial one — is the library working as designed, not a vulnerability.

## Reporting a Vulnerability

Please report privately via GitHub's **"Report a vulnerability"** button under the repository's *Security* tab, rather than opening a public issue.

Include the affected version, a description, and a minimal reproducing input if you have one. You can expect an initial response within a few days.

## Supported Versions

Security fixes target the latest released `0.x` line.
