#!/usr/bin/env bash
# Usage: bash packaging/test-install.sh ubuntu:24.04 native-packages-output
# The compiler runs on the host so it cannot supply missing runtime libraries.
set -euo pipefail
image=${1:?Supply an Ubuntu or Debian container image}
packages=$(realpath "${2:?Supply a native-packages output directory}")
case "$(uname -m)" in
  x86_64) target=linux-amd64 ;;
  aarch64) target=linux-arm64 ;;
  *) echo 'Unsupported test architecture' >&2; exit 1 ;;
esac
case "$image" in
  ubuntu:*|debian:*) format=deb ;;
  *) echo 'Unsupported test distribution' >&2; exit 1 ;;
esac
package_dir="$packages/packages/$target/$format"
test -d "$package_dir"
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
checks=$(mktemp -d)
trap 'rm -rf -- "$checks"' EXIT
cc -std=c99 -Wall -Wextra -Werror "$script_dir/check-runtime-libs.c" -ldl -o "$checks/check-runtime-libs"
docker run --rm \
  --volume "$package_dir:/packages:ro" \
  --volume "$checks:/checks:ro" \
  --env "FORMAT=$format" "$image" sh -ec '
    set -- /packages/*."$FORMAT"
    test "$#" -eq 1
    test -f "$1"
    mkdir -p /root/.config/kuzgun
    printf "%s\n" "preserve-existing-settings" > /root/.config/kuzgun/fixture
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends "$1"
    dpkg-query -W kuzgun
    kuzgun --version
    /checks/check-runtime-libs
    test -s /usr/share/applications/io.github.kaptanoguz.Kuzgun.desktop
    test -s /usr/share/metainfo/io.github.kaptanoguz.Kuzgun.metainfo.xml
    test -s /usr/share/icons/hicolor/scalable/apps/io.github.kaptanoguz.Kuzgun.svg
    test -s /usr/share/icons/hicolor/48x48/apps/io.github.kaptanoguz.Kuzgun.png
    grep -qx "Icon=io.github.kaptanoguz.Kuzgun" /usr/share/applications/io.github.kaptanoguz.Kuzgun.desktop
    grep -qx "StartupWMClass=io.github.kaptanoguz.Kuzgun" /usr/share/applications/io.github.kaptanoguz.Kuzgun.desktop
    test -s /usr/share/kuzgun/omarchy/kuzgun.json.tpl
    test -x /usr/share/kuzgun/omarchy/kuzgun-theme
    apt-get remove -y kuzgun
    test ! -e /usr/bin/kuzgun
    test ! -e /usr/share/applications/io.github.kaptanoguz.Kuzgun.desktop
    test ! -e /usr/share/icons/hicolor/scalable/apps/io.github.kaptanoguz.Kuzgun.svg
    test ! -e /usr/share/kuzgun/omarchy/kuzgun.json.tpl
    test ! -e /usr/share/kuzgun/omarchy/kuzgun-theme
    test "$(cat /root/.config/kuzgun/fixture)" = preserve-existing-settings
  '
