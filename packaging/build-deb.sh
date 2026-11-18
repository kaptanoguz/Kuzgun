#!/bin/bash
# build-deb.sh — Kuzgun yerel .deb paketi derleme ve paketleme aracı
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
VERSION=$(grep '^version' "$PROJECT_DIR/Cargo.toml" | head -1 | sed 's/version = "\(.*\)"/\1/')
BUILD_DIR="$PROJECT_DIR/target/deb-build"
DEB_NAME="kuzgun_${VERSION}_amd64"
DEB_ROOT="$BUILD_DIR/$DEB_NAME"

echo "==> Kuzgun v$VERSION için optimize edilmiş Release binary derleniyor..."
cd "$PROJECT_DIR"
cargo build --release

echo "==> .deb paket ağacı hazırlanıyor..."
rm -rf "$BUILD_DIR"
mkdir -p "$DEB_ROOT/DEBIAN"
mkdir -p "$DEB_ROOT/usr/bin"
mkdir -p "$DEB_ROOT/usr/share/applications"
mkdir -p "$DEB_ROOT/usr/share/metainfo"
mkdir -p "$DEB_ROOT/usr/share/icons/hicolor"

# Binary kopyala
install -Dm755 "$PROJECT_DIR/target/release/kuzgun" "$DEB_ROOT/usr/bin/kuzgun"

# Desktop launcher
install -Dm644 "$PROJECT_DIR/packaging/applications/io.github.kaptanoguz.Kuzgun.desktop" \
    "$DEB_ROOT/usr/share/applications/io.github.kaptanoguz.Kuzgun.desktop"

# Metainfo
if [ -f "$PROJECT_DIR/packaging/metainfo/io.github.kaptanoguz.Kuzgun.metainfo.xml" ]; then
    install -Dm644 "$PROJECT_DIR/packaging/metainfo/io.github.kaptanoguz.Kuzgun.metainfo.xml" \
        "$DEB_ROOT/usr/share/metainfo/io.github.kaptanoguz.Kuzgun.metainfo.xml"
fi

# Hicolor ikonları kopyala
cp -r "$PROJECT_DIR/packaging/icons/hicolor/"* "$DEB_ROOT/usr/share/icons/hicolor/"

# Scalable SVG
if [ -f "$PROJECT_DIR/packaging/icons/kuzgun.svg" ]; then
    mkdir -p "$DEB_ROOT/usr/share/icons/hicolor/scalable/apps"
    install -Dm644 "$PROJECT_DIR/packaging/icons/kuzgun.svg" \
        "$DEB_ROOT/usr/share/icons/hicolor/scalable/apps/io.github.kaptanoguz.Kuzgun.svg"
fi

# DEBIAN/control
cat > "$DEB_ROOT/DEBIAN/control" <<EOF
Package: kuzgun
Version: $VERSION
Section: net
Priority: optional
Architecture: amd64
Depends: libasound2t64 | libasound2, libpipewire-0.3-0t64 | libpipewire-0.3-0, libgl1, libegl1, libxkbcommon0
Maintainer: Oguz Kaptan <kaptanoguz@github.com>
Homepage: https://github.com/kaptanoguz/Kuzgun
Description: Kuzgun — Yerel ve ultra hızlı WhatsApp istemcisi
 Rust ve egui ile geliştirilmiş, donanım ivmeli, yerel ve gizlilik odaklı
 masaüstü WhatsApp istemcisi.
EOF

# DEBIAN/postinst
cat > "$DEB_ROOT/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
update-desktop-database -q /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postinst"

# DEBIAN/postrm
cat > "$DEB_ROOT/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
update-desktop-database -q /usr/share/applications 2>/dev/null || true
gtk-update-icon-cache -q /usr/share/icons/hicolor 2>/dev/null || true
EOF
chmod 755 "$DEB_ROOT/DEBIAN/postrm"

# Boyut hesaplama
INSTALLED_SIZE=$(du -sk "$DEB_ROOT" | cut -f1)
echo "Installed-Size: $INSTALLED_SIZE" >> "$DEB_ROOT/DEBIAN/control"

echo "==> dpkg-deb ile .deb paketi oluşturuluyor..."
dpkg-deb --build --root-owner-group "$DEB_ROOT" "$PROJECT_DIR/${DEB_NAME}.deb"

echo ""
echo "==========================================================="
echo "✓ Paket başarıyla oluşturuldu:"
echo "  $PROJECT_DIR/${DEB_NAME}.deb"
echo ""
echo "Kurmak için:"
echo "  sudo dpkg -i $PROJECT_DIR/${DEB_NAME}.deb"
echo "==========================================================="
