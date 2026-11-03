#!/bin/sh
# Every gate in one place. Run from the repository root:
#
#   sh crates/nhs-gui/scripts/check-all.sh
#
# Cargo needs a target directory outside the project (the repo keeps no target/).
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")/../../.." && pwd)
GUI_TARGET=${GUI_TARGET:-/var/tmp/nhs-gui/t}
WS_TARGET=${WS_TARGET:-/var/tmp/nhs-home-studio-review-target}
# The accepted evidence was produced with 1.98; a floating stable changes lints
# without changing the code. CI pins the version and passes it here; local runs
# keep using the stable channel.
TOOLCHAIN=${NHS_TOOLCHAIN:-+stable}

cd "$ROOT"
echo "== front-end =="
for file in crates/nhs-gui/dist/app.js crates/nhs-gui/dist/i18n.js; do
  node --check "$file"
done
node crates/nhs-gui/scripts/check-i18n.js
node crates/nhs-gui/scripts/check-diff-fold.js
node crates/nhs-gui/scripts/check-diagnosis.js
node crates/nhs-gui/scripts/check-apply.js
node crates/nhs-gui/scripts/check-a11y.js

echo "== gui crate =="
CARGO_TARGET_DIR="$GUI_TARGET" cargo $TOOLCHAIN test --manifest-path crates/nhs-gui/Cargo.toml
CARGO_TARGET_DIR="$GUI_TARGET" cargo $TOOLCHAIN clippy --manifest-path crates/nhs-gui/Cargo.toml --all-targets -- -D warnings
cargo $TOOLCHAIN fmt --manifest-path crates/nhs-gui/Cargo.toml --check

echo "== workspace =="
CARGO_TARGET_DIR="$WS_TARGET" cargo $TOOLCHAIN test --workspace
CARGO_TARGET_DIR="$WS_TARGET" cargo $TOOLCHAIN clippy --workspace --all-targets -- -D warnings
cargo $TOOLCHAIN fmt --all --check

echo "== headless flows =="
"$GUI_TARGET/debug/nhs-gui" --check > /dev/null
"$GUI_TARGET/debug/nhs-gui" --flow-check > /dev/null

echo "== source scan =="
# `flowcheck.rs` builds a scratch HOME for --flow-check; everything else must not
# write. No file may spawn processes.
if grep -rn "File::create\|OpenOptions\|fs::write\|fs::remove\|fs::rename" crates/nhs-gui/src \
  | grep -v "src/flowcheck.rs"; then
  echo "the GUI must not touch files" >&2
  exit 1
fi
if grep -rn "Command::new\|std::process::Command" crates/nhs-gui/src; then
  echo "the GUI must not spawn processes" >&2
  exit 1
fi
grep -q "std::env::temp_dir()" crates/nhs-gui/src/flowcheck.rs
echo "writes confined to the flow-check scratch tree; no process spawns anywhere"

echo "all checks passed"
