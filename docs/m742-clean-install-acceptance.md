# M7.4.2 — clean-profile install acceptance

Date: 2026-11-07. Baseline: local `6f0e5c0`; upstream `614f4ee`.

## Scope and method

This was a fresh **user/profile** acceptance, not a fresh Nix-store build. The
host had only 1.6 GiB free on `/home` (and 5.5 GiB on `/var/tmp`), so the test
reused the already-built M7.4.1 store outputs and did not rebuild either
package. A temporary HOME, all four XDG roots, and a Nix profile were created
under `/var/tmp/nhs-m742-clean.BXHCGL`; no real user configuration was used.

Installed store outputs:

- CLI: `/nix/store/3sjjpn3dfp9z69f0xfqpck894l51b1b2-nhs-cli-0.1.0`
- GUI: `/nix/store/wsbwg3izvr5v66bkcallnqninvrpvggx-nix-home-studio-0.1.0`

Install command:

```sh
nix profile add --profile /var/tmp/nhs-m742-clean.BXHCGL/profile \
  /nix/store/3sjjpn3dfp9z69f0xfqpck894l51b1b2-nhs-cli-0.1.0 \
  /nix/store/wsbwg3izvr5v66bkcallnqninvrpvggx-nix-home-studio-0.1.0
```

`nix profile list` showed both packages, and both `bin/nhs` and `bin/nhs-gui`
resolved to the expected store outputs.

## Results

| Check | Result |
| --- | --- |
| `nhs --version` / `nhs --help` | pass; `nhs 0.1.0` |
| `nhs discover --json` with empty HOME | pass; `unknown`, no generations, no configuration guessed |
| `nhs-gui --help` / `--version` | pass without GTK/display initialisation; `nhs-gui 0.1.0` |
| `nhs-gui --check --bogus` | exit 2; stdout empty |
| `nhs-gui --check` with empty HOME | pass; `unknown`, `switch.available=false`, editing refused because no configuration exists |
| `nhs-gui --flow-check` | pass; 11 steps, validation succeeded, 2 scratch files written, 1 scratch snapshot, History reached |
| Native GUI launch | packaged GUI stayed alive for a 12-second smoke test in the active Wayland session; no stdout/stderr; terminated by the test timeout as intended |
| `check-packaging.js` | pass; version `0.1.0`, platform `x86_64-linux` |
| `check-apply.js` | pass; History remains read-only, zero buttons, no rollback control |

The flow-check scratch tree was self-cleaned. The actual History screen was not
manually navigated in this run; its no-rollback contract was checked by the DOM
gate, and native desktop History had already been verified during M7.3.

## Safety and repository state

- Real Home Manager profile still points to `home-manager-3-link`.
- `/home/ysz/.config/home-manager` remains absent.
- `/home/ysz/.local/state/nix-home-studio` remains absent.
- All test HOME/XDG state and profile data stayed under `/var/tmp`.
- No `home-manager switch`, Nix GC, push, or tag movement.
- Working tree remained clean at `6f0e5c0`; upstream remained `614f4ee`.

During candidate selection, an older cached M7.3 CLI output was rejected because
it did not implement `--version`. The final acceptance used the M7.4.1 CLI
output above; this was an artifact-selection correction, not a source defect.

## Conclusion

M7.4.2 clean-profile installation and automated/runtime smoke acceptance pass.
The test proves installability and basic operation against the existing Nix
store closure; it does **not** claim a clean-store rebuild or a fresh manual
visual walkthrough of the History screen.
