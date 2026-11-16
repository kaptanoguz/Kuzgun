---
title: Download
description: Get Kuzgun for Linux, macOS, or Windows, with install instructions for each.
nav_order: 1
seo:
  title: Download for Linux, macOS, and Windows
---

{% assign v = site.kuzgun_version %}
{% assign name = site.release_asset_prefix %}
{% assign app = site.release_app_name %}
{% assign base = "https://github.com/kaptanoguz/Kuzgun/releases/download/v" | append: v %}

Kuzgun was previously called FastsApp. Version 0.13.0 introduces the new
package and executable names. Your existing session and local data move
automatically when you first start Kuzgun; quit FastsApp before upgrading.

The current version is **v{{ v }}**. SHA-256 checksums are in
[checksums.txt]({{ base }}/checksums.txt). Older versions are on the
[releases page](https://github.com/kaptanoguz/Kuzgun/releases).

## Linux

Linux ships a `.deb`. On Debian, Ubuntu and their derivatives, apt fetches
the libraries it needs:

```sh
sudo apt install ./{{ name }}_{{ v }}_amd64.deb    # or _arm64.deb
```

- [{{ name }}_{{ v }}_amd64.deb]({{ base }}/{{ name }}_{{ v }}_amd64.deb)
- [{{ name }}_{{ v }}_arm64.deb]({{ base }}/{{ name }}_{{ v }}_arm64.deb)

{{ app }} needs glibc 2.36 or newer (Debian 12, Ubuntu 22.10 and newer), the
standard egui libraries, ALSA and libpipewire 0.3.53 or newer:
`libasound2t64`, `libpipewire-0.3-0t64`, `libgl1`, `libxkbcommon0`, and
`libwayland-client0`. Sound goes to PipeWire itself, else to PulseAudio, else
to ALSA. Emoji come from the desktop's colour emoji font when one is installed
(`fonts-noto-color-emoji`), else from the copy {{ app }} bundles. The file
picker uses `xdg-desktop-portal`.

## macOS

One download for both Apple Silicon and Intel:

- [{{ name }}-v{{ v }}-macos-universal.dmg]({{ base }}/{{ name }}-v{{ v }}-macos-universal.dmg)

Open it and drag **{{ app }}** to Applications.

If you use [Homebrew](https://brew.sh), you can install it with:

```sh
brew install --cask crmne/tap/kuzgun
```

The app is signed with Developer ID and notarized by Apple. The DMG includes
a validated notarization ticket.

When upgrading from FastsApp, remove the old FastsApp application bundle
after installing Kuzgun.

## Windows

The installer is a Windows Installer package (`.msi`). It adds {{ app }} to
the Start menu and needs no administrator rights. Choose x86_64 for most PCs
or aarch64 for Windows on ARM:

- [{{ name }}-v{{ v }}-x86_64-pc-windows-msvc.msi]({{ base }}/{{ name }}-v{{ v }}-x86_64-pc-windows-msvc.msi)
- [{{ name }}-v{{ v }}-aarch64-pc-windows-msvc.msi]({{ base }}/{{ name }}-v{{ v }}-aarch64-pc-windows-msvc.msi)

If you installed {{ app }} with the older setup program (`-setup.exe`), 0.20.0
and later move to the package through their own updates. On 0.19.0 or earlier,
run the `.msi` once: it installs into the same folder and replaces the old
installer's entry in Settings > Apps. Your session, chats and settings stay.

To run {{ app }} without installing it, download a zip, extract it, and run
`{{ name }}.exe`.

- [{{ name }}-v{{ v }}-x86_64-pc-windows-msvc.zip]({{ base }}/{{ name }}-v{{ v }}-x86_64-pc-windows-msvc.zip)
- [{{ name }}-v{{ v }}-aarch64-pc-windows-msvc.zip]({{ base }}/{{ name }}-v{{ v }}-aarch64-pc-windows-msvc.zip)

{{ app }} draws with OpenGL and needs a graphics driver with OpenGL 2.1 or
newer. Install the driver from the maker of your graphics chip: the Microsoft
Basic Display Adapter, some virtual machines, and some remote desktop sessions
offer no usable OpenGL, and Windows then shows a message instead of starting.

SmartScreen may warn about an unknown publisher on first run. Choose **More
info**, then **Run anyway**.
