#!/usr/bin/env sh
# minihoard installer — downloads both binaries from the latest GitHub release.
# Usage: curl -fsSL https://github.com/irongollem/minihoard/releases/latest/download/install.sh | sh
# Override install dir:  BIN_DIR=/usr/local/bin sh install.sh
set -e

REPO="irongollem/minihoard"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

OS=$(uname -s)
ARCH=$(uname -m)

case "$OS-$ARCH" in
  Darwin-arm64|Darwin-aarch64) TARGET="aarch64-apple-darwin" ;;
  Darwin-x86_64)               TARGET="x86_64-apple-darwin" ;;
  *)
    echo "Unsupported platform: $OS $ARCH"
    echo "Download manually from https://github.com/$REPO/releases/latest"
    exit 1
    ;;
esac

LATEST=$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" \
  | grep '"tag_name"' | head -1 | cut -d'"' -f4)

if [ -z "$LATEST" ]; then
  echo "Could not fetch latest release. Check your internet connection."
  exit 1
fi

echo "Installing minihoard $LATEST ($TARGET) to $BIN_DIR"
mkdir -p "$BIN_DIR"

for BIN in minihoard minihoard-mcp; do
  URL="https://github.com/$REPO/releases/download/$LATEST/${BIN}-${TARGET}"
  echo "  $BIN..."
  curl -fsSL -o "$BIN_DIR/$BIN" "$URL"
  chmod +x "$BIN_DIR/$BIN"
done

echo ""
echo "Installed:"
echo "  $BIN_DIR/minihoard"
echo "  $BIN_DIR/minihoard-mcp"

case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *)
    echo ""
    echo "Note: $BIN_DIR is not on your PATH."
    echo "Add this to your shell profile (~/.zshrc or ~/.bashrc):"
    echo "  export PATH=\"$BIN_DIR:\$PATH\""
    ;;
esac

echo ""
echo "Next: run 'minihoard configure' to set up."
