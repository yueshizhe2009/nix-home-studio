# nix_log — donated from NixBox

| Field | Value |
| --- | --- |
| Upstream repository | https://github.com/SINGH-RAJVEER/nixbox |
| Upstream commit | `7bab22d13a6ab70b9dede01d359dd546bb489060` (2026-10-23, "Merge pull request #31 from SINGH-RAJVEER/dev") |
| Original path | `crates/nixbox-nix/src/nix_log.rs` |
| Licence | Apache-2.0 (see `NOTICE` at the repository root) |
| Local path | `crates/nhs-core/src/donated/nix-log/nix_log.rs` |

## What it does

Parses Nix's structured log (`--log-format internal-json`): `start`, `stop`,
`result` and `msg` records, one per stderr line prefixed with `@nix `, plus
`strip_ansi` and `store_name` helpers.

## Changes made here

Every change is mechanical or a lint fix; the parsing logic and the upstream
tests are unchanged.

1. Re-indented with `rustfmt` (upstream uses tabs).
2. The doc comment on `STRUCTURED_ARGS` no longer names a system-rebuild wrapper
   by name: `tests/safety.rs` greps the crate for that string, and a comment is
   not worth an exception to the gate.
3. `unwrap_or`/`unwrap_or_default` kept; no `unwrap`, `expect`, `panic`,
   `as` cast or slice indexing was introduced. `u8::try_from` (upstream) already
   satisfies the `as_conversions` lint.
4. Kept the upstream unit tests verbatim, including the captured `@nix` line they
   assert on.

## Not included

The upstream runner (`crates/nixbox-nix/src/build.rs`, Tokio + `libc::setpgid`)
was **not** copied. This project's `proc.rs` implements the same idea
synchronously; that code is original, not donated.
