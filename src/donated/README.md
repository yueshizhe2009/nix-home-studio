# Donated source

Donated **code** lives inside the crate it serves, so it is compiled and tested
with the rest of the project: see `crates/nhs-core/src/donated/<module>/`, each
with its own `PROVENANCE.md`.

Nothing may be placed there without that sibling `PROVENANCE.md` recording:

- upstream repository URL
- exact upstream commit SHA
- original repository-relative path
- a statement of what was changed

The retained copyright and licence header of every donated file must
remain intact. See `NOTICE` at the repository root.

Current donors:

| Module | Upstream | Commit |
| --- | --- | --- |
| `crates/nhs-core/src/donated/nix-log/` | NixBox `crates/nixbox-nix/src/nix_log.rs` | `7bab22d13a6ab70b9dede01d359dd546bb489060` |
| `crates/nhs-core/src/donated/nix-search/` | NixBox `crates/nixbox-nix/src/search.rs` | `7bab22d13a6ab70b9dede01d359dd546bb489060` |

`nix-home-manager-gui` is **not** a donor: it has no licence. Its behaviour may
be studied, but no code is copied.
