#!/usr/bin/env bash
# Installs Nerea for the current user, no root required.
#
#   ./install.sh              -> ~/.local
#   PREFIX=/usr ./install.sh  -> system wide (needs write access)
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
prefix="${PREFIX:-$HOME/.local}"
bin="$prefix/bin"
icons="$prefix/share/icons/hicolor/512x512/apps"
apps="$prefix/share/applications"

install -Dm755 "$here/nerea-desktop" "$bin/nerea-desktop"
install -Dm644 "$here/nerea.png" "$icons/nerea.png"
install -d "$apps"
sed "s|^Exec=.*|Exec=$bin/nerea-desktop|" "$here/nerea.desktop" > "$apps/nerea.desktop"
chmod 644 "$apps/nerea.desktop"

# The voice bank is machine-local: point the app at the checkout or at a copy.
echo
echo "installed: $bin/nerea-desktop"
echo "app menu:  $apps/nerea.desktop"
echo

case ":$PATH:" in
  *":$bin:"*) ;;
  *) echo "note: $bin is not in PATH; the desktop entry already uses the full path" ;;
esac

echo "voice: export NEREA_BANK_DIR=/path/to/nerea/assets/voice/bank"
echo "       without it the app runs and stays silent, with subtitles."
