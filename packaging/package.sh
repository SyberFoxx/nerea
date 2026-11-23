#!/usr/bin/env bash
# Builds the Linux deliverables from an already-built release binary:
#
#   .deb         Debian, Ubuntu 24.04+, Mint, Pop!_OS, Zorin
#   .tar.gz      any x86_64 distribution (installs into ~/.local, no root)
#   .AppImage    any x86_64 distribution, single file (built by the Tauri CLI)
#
# Usage: packaging/package.sh        (expects target/release/nerea-desktop)
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/.."

version="$(sed -n 's/^version = "\(.*\)"/\1/p' src-tauri/Cargo.toml | head -1)"
binary="target/release/nerea-desktop"
dist="$PWD/dist"

[ -x "$binary" ] || { echo "missing $binary: run cargo build --release -p nerea-desktop" >&2; exit 1; }
[ -n "$version" ] || { echo "could not read the version" >&2; exit 1; }

# The .deb must declare the glibc it was actually built against.
glibc="$(objdump -T "$binary" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -V | tail -1)"
glibc="${glibc:-2.39}"

mkdir -p "$dist"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

echo "packaging nerea $version (glibc floor $glibc)"

# --- .deb -------------------------------------------------------------------
root="$work/root"
install -Dm755 "$binary" "$root/usr/bin/nerea-desktop"
install -Dm644 packaging/nerea.desktop "$root/usr/share/applications/nerea.desktop"
install -Dm644 src-tauri/icons/icon.png "$root/usr/share/icons/hicolor/512x512/apps/nerea.png"

control="$work/control"
mkdir -p "$control"
cat > "$control/control" <<EOF
Package: nerea
Version: $version
Architecture: amd64
Maintainer: SyberFoxx <syberfoxx@users.noreply.github.com>
Installed-Size: $(du -sk "$root/usr" | cut -f1)
Depends: libwebkit2gtk-4.1-0, libjavascriptcoregtk-4.1-0, libgtk-3-0, libsoup-3.0-0, libasound2t64 | libasound2, libc6 (>= $glibc)
Section: utils
Priority: optional
Homepage: https://github.com/SyberFoxx/nerea
Description: Nerea, a desktop companion with a cloned voice
 She lives on the desktop as a single portrait: a chat panel attaches to
 her body, she speaks short Japanese lines with Spanish subtitles, and the
 conversation survives a restart. Rust core, Tauri shell, no Electron.
EOF

(cd "$root" && find usr -type f -exec md5sum {} +) > "$control/md5sums"

cat > "$control/postinst" <<'EOF'
#!/bin/sh
set -e
gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q /usr/share/applications 2>/dev/null || true
EOF
chmod 755 "$control/postinst"

(cd "$control" && tar --owner=0 --group=0 -czf "$work/control.tar.gz" ./control ./md5sums ./postinst)
(cd "$root" && tar --owner=0 --group=0 -czf "$work/data.tar.gz" ./usr)
printf '2.0\n' > "$work/debian-binary"

(cd "$work" && ar rcs "$dist/nerea_${version}_amd64.deb" debian-binary control.tar.gz data.tar.gz)
echo "  dist/nerea_${version}_amd64.deb"

# --- portable tarball -------------------------------------------------------
bundle="nerea-${version}-x86_64-linux"
mkdir -p "$work/$bundle"
cp "$binary" "$work/$bundle/nerea-desktop"
cp packaging/install.sh "$work/$bundle/install.sh"
cp packaging/nerea.desktop "$work/$bundle/nerea.desktop"
cp src-tauri/icons/icon.png "$work/$bundle/nerea.png"
chmod 755 "$work/$bundle/nerea-desktop" "$work/$bundle/install.sh"
(cd "$work" && tar -czf "$dist/$bundle.tar.gz" "$bundle")
echo "  dist/$bundle.tar.gz"

# --- AppImage ---------------------------------------------------------------
appimage="$(ls target/release/bundle/appimage/*.AppImage 2>/dev/null | head -1 || true)"
if [ -n "$appimage" ]; then
  cp "$appimage" "$dist/"
  echo "  dist/$(basename "$appimage")"
else
  echo "  (no AppImage: run npx @tauri-apps/cli@^2 build --bundles appimage)"
fi

echo
ls -lh "$dist" | tail -n +2 | awk '{printf "%-42s %s\n", $9, $5}'
