#!/usr/bin/env bash
# Build ModelCraft for Linux and package it:
#   dist/release/modelcraft-<version>-linux-<arch>.tar.gz   (FORMATS: tar)
#   dist/release/modelcraft-<version>-linux-<arch>.deb      (FORMATS: deb, needs nfpm)
#   dist/release/modelcraft-<version>-linux-<arch>.rpm      (FORMATS: rpm, needs nfpm)
#
# Usage: packaging/linux/package.sh [--skip-build] [--formats "tar deb rpm"]
# Release builds set CRAFT_FONTS_DIR/CRAFT_FONTS_REQUIRED and CRAFT_I18N_DIR/CRAFT_I18N_REQUIRED.
set -euo pipefail
. "$(dirname "${BASH_SOURCE[0]}")/../env.sh"
HERE="$ROOT/packaging/linux"
APP_ID=ai.storyteller.modelcraft

SKIP_BUILD=0
FORMATS="tar deb rpm"
while [ $# -gt 0 ]; do
  case "$1" in
    --skip-build) SKIP_BUILD=1; shift ;;
    --formats) FORMATS="$2"; shift 2 ;;
    -h | --help) sed -n '2,10p' "$0"; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
done

ARCH="${CROSS_ARCH:-$(uname -m)}"
case "$ARCH" in
  x86_64) NFPM_ARCH=amd64 ;;
  aarch64 | arm64) ARCH=aarch64; NFPM_ARCH=arm64 ;;
  riscv64) NFPM_ARCH=riscv64 ;;
  *) echo "unsupported architecture $ARCH" >&2; exit 2 ;;
esac
export NFPM_ARCH
# TODO(maintainers): the package maintainer address.
export MODELCRAFT_MAINTAINER="${MODELCRAFT_MAINTAINER:-ModelCraft maintainers}"
BASENAME="modelcraft-$VERSION-linux-$ARCH"

echo "==> ModelCraft $VERSION for Linux $ARCH ($FORMATS)"
if [ "$SKIP_BUILD" = 0 ]; then
  (cd "$ROOT" && cargo build --release --locked -p modelcraft -p modelcraft-cli ${CROSS_TARGET:+--target "$CROSS_TARGET"})
fi
BIN="$CARGO_TARGET_DIR/${CROSS_TARGET:+$CROSS_TARGET/}release"
WORK="$CARGO_TARGET_DIR/linux-package"
STAGE="$WORK/root"
rm -rf "$WORK"

install -Dm755 "$BIN/modelcraft" "$STAGE/usr/bin/modelcraft"
install -Dm755 "$BIN/modelcraft-cli" "$STAGE/usr/bin/modelcraft-cli"
"${CROSS_COMPILE:-}strip" "$STAGE/usr/bin/modelcraft" "$STAGE/usr/bin/modelcraft-cli" 2>/dev/null || true
install -Dm644 "$HERE/$APP_ID.desktop" "$STAGE/usr/share/applications/$APP_ID.desktop"
install -Dm644 "$HERE/$APP_ID.mime.xml" "$STAGE/usr/share/mime/packages/$APP_ID.xml"
mkdir -p "$STAGE/usr/share/metainfo"
sed -e "s/@VERSION@/$VERSION/g" -e "s/@DATE@/$MODELCRAFT_BUILD_DATE/g" \
  "$HERE/$APP_ID.metainfo.xml.in" >"$STAGE/usr/share/metainfo/$APP_ID.metainfo.xml"
mkdir -p "$STAGE/usr/share/icons"
cp -R "$ROOT/assets/app-icon/hicolor" "$STAGE/usr/share/icons/"
install -Dm644 -t "$STAGE/usr/share/doc/modelcraft" "$ROOT/README.md" "$ROOT/LICENSE-MIT" "$ROOT/LICENSE-APACHE" "$ROOT/NOTICE" "$ROOT/ATTRIBUTION.md"

if command -v desktop-file-validate >/dev/null; then
  desktop-file-validate "$STAGE/usr/share/applications/$APP_ID.desktop"
fi

has() { case " $FORMATS " in *" $1 "*) return 0 ;; *) return 1 ;; esac; }

if has tar; then
  mkdir -p "$WORK/tar"
  cp -R "$STAGE/usr" "$WORK/tar/$BASENAME"
  tar -C "$WORK/tar" -czf "$DIST/$BASENAME.tar.gz" "$BASENAME"
  echo "wrote $DIST/$BASENAME.tar.gz"
fi
for fmt in deb rpm; do
  if has "$fmt"; then
    sed "s|\${STAGE}|$STAGE|g" "$HERE/nfpm.yaml" >"$WORK/nfpm.yaml"
    (cd "$ROOT" && nfpm package -f "$WORK/nfpm.yaml" -p "$fmt" -t "$DIST/$BASENAME.$fmt")
    echo "wrote $DIST/$BASENAME.$fmt"
  fi
done
