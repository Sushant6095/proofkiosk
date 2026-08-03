#!/usr/bin/env bash
# Build one kiosk plugin for wasm32-wasip2 and assemble the directory layout
# `zeroclaw plugin install` expects: a dir holding manifest.toml plus the
# component named exactly as the manifest's `wasm_path`.
#
#   ./scripts/stage-plugin.sh kiosk-charge
#   zeroclaw plugin install ./staged/kiosk-charge/
#
# With no argument, stages all three.

set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
PLUGINS=("${@:-kiosk-charge kiosk-watch kiosk-attest}")
# shellcheck disable=SC2206
PLUGINS=(${PLUGINS[*]})

rustup target list --installed 2>/dev/null | grep -q wasm32-wasip2 \
  || { echo "wasm32-wasip2 target missing — run: rustup target add wasm32-wasip2" >&2; exit 1; }

for p in "${PLUGINS[@]}"; do
  src="$ROOT/plugins/$p"
  [ -d "$src" ] || { echo "no such plugin: $p" >&2; exit 1; }

  # The manifest names the artifact it expects; read it rather than guessing.
  wasm_path=$(grep -E '^wasm_path' "$src/manifest.toml" | head -1 | cut -d'"' -f2)
  built="$src/target/wasm32-wasip2/release/${p//-/_}.wasm"
  out="$ROOT/staged/$p"

  echo "[stage] building $p"
  ( cd "$src" && CARGO_TARGET_DIR="$src/target" \
      cargo build --locked --target wasm32-wasip2 --release >/dev/null )

  mkdir -p "$out"
  cp "$src/manifest.toml" "$out/manifest.toml"
  cp "$built" "$out/$wasm_path"
  echo "[stage] $out  ($(( $(wc -c < "$out/$wasm_path") / 1024 )) KB)"
done

echo
echo "Install with:"
for p in "${PLUGINS[@]}"; do echo "  zeroclaw plugin install ./staged/$p/"; done
echo "  zeroclaw config set plugins.enabled true"
echo "  zeroclaw plugin list"
