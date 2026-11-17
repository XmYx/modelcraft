#!/usr/bin/env bash
# Regenerate the platform app icons in assets/app-icon/:
#
#   assets/app-icon/hicolor/*/apps/ai.storyteller.modelcraft.png, modelcraft-1024.png
#   assets/app-icon/modelcraft.ico    Windows (PNG entries 16–256 px)
#   assets/app-icon/modelcraft.icns   macOS (PNG entries 16–1024 px)
#
# All of them are rendered from assets/app-icon/modelcraft.svg by `cargo xtask icons` (craft-svg's
# bounded renderer and craft-image-codecs; no platform tools needed). The outputs are committed,
# so builds and packaging never need this; package scripts call it only when an output is missing.
#
#   packaging/icons.sh [ico] [icns]     (arguments accepted for compatibility; everything is rendered)
set -euo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
(cd "$ROOT" && cargo run -q -p xtask -- icons)
