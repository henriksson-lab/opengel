APP_NAME := OpenGel
APP_VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
APP_BUNDLE := target/osx/$(APP_NAME).app
APP_BINARY := target/release/opengel
CLI_BINARY := target/release/gel
APP_UNIVERSAL_BINARY := target/osx/opengel-universal
APP_EXE := $(APP_BUNDLE)/Contents/MacOS/opengel
APP_PLIST := $(APP_BUNDLE)/Contents/Info.plist
# App icon: assets/icon.svg is the source of truth; assets/icon-1024.png is the
# committed 1024px master it renders to. The .icns is built from that master at
# packaging time with macOS built-ins only (sips + iconutil) — no SVG rasterizer.
APP_ICON_SRC := assets/icon-1024.png
APP_ICONSET := target/osx/AppIcon.iconset
APP_ICNS := $(APP_BUNDLE)/Contents/Resources/AppIcon.icns

# --- Linux install (freedesktop layout) -------------------------------------
# Standard prefix vars; packagers override DESTDIR (staging) and PREFIX.
PREFIX ?= /usr/local
DESTDIR ?=
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share
ICONDIR := $(DATADIR)/icons/hicolor
MIMEDIR := $(DATADIR)/mime
MIMEPKGDIR := $(MIMEDIR)/packages
# The window's Wayland app_id / X11 WM_CLASS; the icon PNG/SVG and the .desktop
# StartupWMClass are all keyed to this name so the desktop finds the icon.
LINUX_APP_ID := opengel
# Extra cargo flags for the Linux release build. Camera capture is opt-in on
# Linux (needs libv4l-dev at build time; adds a libv4l-0 runtime dep), so the
# packaged app enables it by default to match the always-on capture on macOS /
# Windows. Override with `make deb CARGO_BUILD_FLAGS=` to build without a camera.
CARGO_BUILD_FLAGS ?= --features camera
DEB_NAME := opengel
DEB_ARCH ?= $(shell dpkg --print-architecture 2>/dev/null || printf 'amd64')
# make deb derives ELF shared-library dependencies with dpkg-shlibdeps, including
# versioned libc/glibc constraints. DEB_DEPENDS is for non-ELF package deps.
DEB_DEPENDS ?= shared-mime-info
DEB_RECOMMENDS ?= desktop-file-utils, gtk-update-icon-cache, xdg-desktop-portal
DEB_MAINTAINER ?= OpenGel Maintainers <opengel-maintainers@users.noreply.github.com>
DEB_ROOT := target/deb/root
DEB_DOCDIR := $(DEB_ROOT)/usr/share/doc/$(DEB_NAME)
DEB_MANDIR := $(DEB_ROOT)/usr/share/man/man1
DEB_SHLIBDEPS_DIR := target/deb/shlibdeps
DEB_FILE := target/deb/$(DEB_NAME)_$(APP_VERSION)_$(DEB_ARCH).deb

.PHONY: deb osx-app osx-app-universal osx-bundle clean-osx-app install uninstall

# Install the release binaries (GUI `opengel` + CLI `gel`), the .desktop entry,
# the MIME package and the icons into a freedesktop layout. Uses the scalable SVG
# (any size, no rasterizer needed) plus the committed 256px PNG as a fallback for
# themes that ignore SVG.
install:
	cargo build --release $(CARGO_BUILD_FLAGS)
	install -Dm755 "$(APP_BINARY)" "$(BINDIR)/$(LINUX_APP_ID)"
	install -Dm755 "$(CLI_BINARY)" "$(BINDIR)/gel"
	install -Dm644 packaging/opengel.desktop \
		"$(DATADIR)/applications/$(LINUX_APP_ID).desktop"
	install -Dm644 packaging/opengel-mime.xml \
		"$(MIMEPKGDIR)/$(LINUX_APP_ID).xml"
	install -Dm644 assets/icon.svg \
		"$(ICONDIR)/scalable/apps/$(LINUX_APP_ID).svg"
	install -Dm644 assets/window-icon.png \
		"$(ICONDIR)/256x256/apps/$(LINUX_APP_ID).png"
	if [ -z "$(DESTDIR)" ] && command -v update-mime-database >/dev/null 2>&1; then update-mime-database "$(MIMEDIR)"; fi
	if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$(DATADIR)/applications"; fi
	if [ -z "$(DESTDIR)" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f "$(ICONDIR)"; fi
	@printf 'Installed to %s\n' "$(DESTDIR)$(PREFIX)"

uninstall:
	rm -f "$(BINDIR)/$(LINUX_APP_ID)" \
		"$(BINDIR)/gel" \
		"$(DATADIR)/applications/$(LINUX_APP_ID).desktop" \
		"$(MIMEPKGDIR)/$(LINUX_APP_ID).xml" \
		"$(ICONDIR)/scalable/apps/$(LINUX_APP_ID).svg" \
		"$(ICONDIR)/256x256/apps/$(LINUX_APP_ID).png"
	if [ -z "$(DESTDIR)" ] && command -v update-mime-database >/dev/null 2>&1; then update-mime-database "$(MIMEDIR)"; fi
	if [ -z "$(DESTDIR)" ] && command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database "$(DATADIR)/applications"; fi
	if [ -z "$(DESTDIR)" ] && command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f "$(ICONDIR)"; fi

deb:
	rm -rf "$(DEB_ROOT)"
	rm -rf "$(DEB_SHLIBDEPS_DIR)"
	rm -f target/deb/*.deb
	$(MAKE) install DESTDIR="$(CURDIR)/$(DEB_ROOT)" PREFIX=/usr
	if command -v strip >/dev/null 2>&1; then strip --strip-unneeded "$(DEB_ROOT)/usr/bin/$(LINUX_APP_ID)" || strip "$(DEB_ROOT)/usr/bin/$(LINUX_APP_ID)" || true; fi
	if command -v strip >/dev/null 2>&1; then strip --strip-unneeded "$(DEB_ROOT)/usr/bin/gel" || strip "$(DEB_ROOT)/usr/bin/gel" || true; fi
	mkdir -p "$(DEB_ROOT)/DEBIAN" "$(DEB_DOCDIR)" "$(DEB_MANDIR)" "$(DEB_SHLIBDEPS_DIR)/debian" target/deb
	printf '%s\n' \
		'Source: $(DEB_NAME)' \
		'Section: science' \
		'Priority: optional' \
		'Maintainer: $(DEB_MAINTAINER)' \
		'Standards-Version: 4.6.2' \
		'' \
		'Package: $(DEB_NAME)' \
		'Architecture: any' \
		'Depends: $${shlibs:Depends}' \
		'Description: Gel electrophoresis image capture and quantification' \
		' OpenGel captures, simulates, detects and quantifies gel electrophoresis images.' \
		> "$(DEB_SHLIBDEPS_DIR)/debian/control"
	shlibs_depends="$$(cd "$(DEB_SHLIBDEPS_DIR)" && dpkg-shlibdeps -O -e"../root/usr/bin/$(LINUX_APP_ID)" | sed 's/^shlibs:Depends=//')"; \
	if [ -n "$(DEB_DEPENDS)" ]; then package_depends="$$shlibs_depends, $(DEB_DEPENDS)"; else package_depends="$$shlibs_depends"; fi; \
	printf '%s\n' \
		"$$package_depends" \
		> target/deb/depends
	printf '%s\n' \
		'Package: $(DEB_NAME)' \
		'Version: $(APP_VERSION)' \
		'Section: science' \
		'Priority: optional' \
		'Architecture: $(DEB_ARCH)' \
		'Maintainer: $(DEB_MAINTAINER)' \
		"Depends: $$(cat target/deb/depends)" \
		'Recommends: $(DEB_RECOMMENDS)' \
		'Description: Gel electrophoresis image capture and quantification' \
		' OpenGel captures gel images from a USB camera (with multi-exposure HDR),' \
		' auto-detects lanes, bands and the ladder, and quantifies band amounts in' \
		' ng and molarity for DNA, RNA and protein gels. Ships a GUI (opengel) and a' \
		' headless CLI (gel).' \
		> "$(DEB_ROOT)/DEBIAN/control"
	printf '%s\n' \
		'Format: https://www.debian.org/doc/packaging-manuals/copyright-format/1.0/' \
		'Upstream-Name: opengel' \
		'Source: https://github.com/opengel/opengel' \
		'' \
		'Files: *' \
		'Copyright: 2026 OpenGel contributors' \
		'License: MIT or Apache-2.0' \
		' This program is dual-licensed under the MIT and Apache License 2.0 licenses;' \
		' you may choose either. The MIT license text follows.' \
		' .' \
		' Permission is hereby granted, free of charge, to any person obtaining a copy' \
		' of this software and associated documentation files (the "Software"), to deal' \
		' in the Software without restriction, including without limitation the rights' \
		' to use, copy, modify, merge, publish, distribute, sublicense, and/or sell' \
		' copies of the Software, and to permit persons to whom the Software is' \
		' furnished to do so, subject to the following conditions:' \
		' .' \
		' The above copyright notice and this permission notice shall be included in all' \
		' copies or substantial portions of the Software.' \
		' .' \
		' THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR' \
		' IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,' \
		' FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE' \
		' AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER' \
		' LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,' \
		' OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE' \
		' SOFTWARE.' \
		> "$(DEB_DOCDIR)/copyright"
	printf '%s\n' \
		'opengel ($(APP_VERSION)) unstable; urgency=medium' \
		'' \
		'  * Local release build.' \
		'' \
		' -- $(DEB_MAINTAINER)  $(shell date -R)' \
		| gzip -9n > "$(DEB_DOCDIR)/changelog.gz"
	printf '%s\n' \
		'.TH OPENGEL 1 "$(shell date +%Y-%m-%d)" "$(APP_VERSION)" "User Commands"' \
		'.SH NAME' \
		'opengel \- capture and quantify gel electrophoresis images' \
		'.SH SYNOPSIS' \
		'.B opengel' \
		'[\fIFILE\fR] [\fB--demo\fR]' \
		'.SH DESCRIPTION' \
		'.B OpenGel' \
		'captures gel images from a USB camera (with multi-exposure HDR), auto-detects lanes, bands and the ladder, and quantifies band amounts in ng and molarity for DNA, RNA and protein gels.' \
		'.SH FILES' \
		'.B opengel' \
		'opens OpenGel project files (.gel.zip). The headless CLI is available as \fBgel\fR(1).' \
		'.SH AUTHOR' \
		'OpenGel contributors' \
		| gzip -9n > "$(DEB_MANDIR)/opengel.1.gz"
	printf '%s\n' \
		'.TH GEL 1 "$(shell date +%Y-%m-%d)" "$(APP_VERSION)" "User Commands"' \
		'.SH NAME' \
		'gel \- headless gel image analysis' \
		'.SH SYNOPSIS' \
		'.B gel' \
		'\fICOMMAND\fR [\fIOPTIONS\fR]' \
		'.SH DESCRIPTION' \
		'.B gel' \
		'is the headless companion to \fBopengel\fR(1): analyze, quantify, run the evaluation harness, and simulate gels from the command line. See \fBgel --help\fR.' \
		'.SH AUTHOR' \
		'OpenGel contributors' \
		| gzip -9n > "$(DEB_MANDIR)/gel.1.gz"
	printf '%s\n' \
		'#!/bin/sh' \
		'set -e' \
		'if command -v update-mime-database >/dev/null 2>&1; then update-mime-database /usr/share/mime || true; fi' \
		'if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database /usr/share/applications || true; fi' \
		'if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true; fi' \
		'exit 0' \
		> "$(DEB_ROOT)/DEBIAN/postinst"
	printf '%s\n' \
		'#!/bin/sh' \
		'set -e' \
		'if command -v update-mime-database >/dev/null 2>&1; then update-mime-database /usr/share/mime || true; fi' \
		'if command -v update-desktop-database >/dev/null 2>&1; then update-desktop-database /usr/share/applications || true; fi' \
		'if command -v gtk-update-icon-cache >/dev/null 2>&1; then gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true; fi' \
		'exit 0' \
		> "$(DEB_ROOT)/DEBIAN/postrm"
	chmod 755 "$(DEB_ROOT)/usr/share/doc" "$(DEB_DOCDIR)"
	chmod 644 "$(DEB_DOCDIR)/copyright" "$(DEB_DOCDIR)/changelog.gz"
	chmod 755 "$(DEB_ROOT)/usr/share/man" "$(DEB_MANDIR)"
	chmod 644 "$(DEB_MANDIR)/opengel.1.gz" "$(DEB_MANDIR)/gel.1.gz"
	chmod 755 "$(DEB_ROOT)/DEBIAN/postinst" "$(DEB_ROOT)/DEBIAN/postrm"
	dpkg-deb --build --root-owner-group "$(DEB_ROOT)" "$(DEB_FILE)"
	@printf 'Built %s\n' "$(DEB_FILE)"

# --- macOS .app bundle ------------------------------------------------------
osx-app:
	cargo build --release
	$(MAKE) osx-bundle APP_BUNDLE_BINARY="$(APP_BINARY)"

osx-app-universal:
	rustup target add x86_64-apple-darwin aarch64-apple-darwin
	cargo build --release --target x86_64-apple-darwin
	cargo build --release --target aarch64-apple-darwin
	mkdir -p target/osx
	lipo -create \
		target/x86_64-apple-darwin/release/opengel \
		target/aarch64-apple-darwin/release/opengel \
		-output "$(APP_UNIVERSAL_BINARY)"
	lipo "$(APP_UNIVERSAL_BINARY)" -verify_arch x86_64 arm64
	$(MAKE) osx-bundle APP_BUNDLE_BINARY="$(APP_UNIVERSAL_BINARY)"

osx-bundle:
	mkdir -p "$(APP_BUNDLE)/Contents/MacOS" "$(APP_BUNDLE)/Contents/Resources" "$(APP_ICONSET)"
	cp "$(APP_BUNDLE_BINARY)" "$(APP_EXE)"
	chmod +x "$(APP_EXE)"
	set -e; for pair in "16 icon_16x16" "32 icon_16x16@2x" "32 icon_32x32" \
		"64 icon_32x32@2x" "128 icon_128x128" "256 icon_128x128@2x" \
		"256 icon_256x256" "512 icon_256x256@2x" "512 icon_512x512" \
		"1024 icon_512x512@2x"; do \
		set -- $$pair; \
		sips -z $$1 $$1 "$(APP_ICON_SRC)" --out "$(APP_ICONSET)/$$2.png" >/dev/null; \
	done
	iconutil -c icns "$(APP_ICONSET)" -o "$(APP_ICNS)"
	printf '%s\n' \
		'<?xml version="1.0" encoding="UTF-8"?>' \
		'<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">' \
		'<plist version="1.0">' \
		'<dict>' \
		'  <key>CFBundleDevelopmentRegion</key>' \
		'  <string>en</string>' \
		'  <key>CFBundleDisplayName</key>' \
		'  <string>$(APP_NAME)</string>' \
		'  <key>CFBundleExecutable</key>' \
		'  <string>opengel</string>' \
		'  <key>CFBundleIconFile</key>' \
		'  <string>AppIcon</string>' \
		'  <key>CFBundleIdentifier</key>' \
		'  <string>org.opengel.OpenGel</string>' \
		'  <key>CFBundleDocumentTypes</key>' \
		'  <array>' \
		'    <dict>' \
		'      <key>CFBundleTypeName</key>' \
		'      <string>OpenGel gel project</string>' \
		'      <key>CFBundleTypeRole</key>' \
		'      <string>Editor</string>' \
		'      <key>CFBundleTypeIconFile</key>' \
		'      <string>AppIcon</string>' \
		'      <key>LSHandlerRank</key>' \
		'      <string>Owner</string>' \
		'      <key>LSItemContentTypes</key>' \
		'      <array>' \
		'        <string>org.opengel.gel-archive</string>' \
		'      </array>' \
		'    </dict>' \
		'  </array>' \
		'  <key>CFBundleInfoDictionaryVersion</key>' \
		'  <string>6.0</string>' \
		'  <key>CFBundleName</key>' \
		'  <string>$(APP_NAME)</string>' \
		'  <key>CFBundlePackageType</key>' \
		'  <string>APPL</string>' \
		'  <key>CFBundleShortVersionString</key>' \
		'  <string>$(APP_VERSION)</string>' \
		'  <key>CFBundleVersion</key>' \
		'  <string>$(APP_VERSION)</string>' \
		'  <key>LSMinimumSystemVersion</key>' \
		'  <string>11.0</string>' \
		'  <key>NSHighResolutionCapable</key>' \
		'  <true/>' \
		'  <key>NSCameraUsageDescription</key>' \
		'  <string>OpenGel captures gel images from a connected camera.</string>' \
		'  <key>UTExportedTypeDeclarations</key>' \
		'  <array>' \
		'    <dict>' \
		'      <key>UTTypeIdentifier</key>' \
		'      <string>org.opengel.gel-archive</string>' \
		'      <key>UTTypeDescription</key>' \
		'      <string>OpenGel gel project</string>' \
		'      <key>UTTypeConformsTo</key>' \
		'      <array><string>public.zip-archive</string></array>' \
		'      <key>UTTypeTagSpecification</key>' \
		'      <dict><key>public.filename-extension</key><array><string>gel.zip</string></array></dict>' \
		'    </dict>' \
		'  </array>' \
		'</dict>' \
		'</plist>' \
		> "$(APP_PLIST)"
	@printf 'Built %s\n' "$(APP_BUNDLE)"

clean-osx-app:
	rm -rf "$(APP_BUNDLE)" "$(APP_ICONSET)" "$(APP_UNIVERSAL_BINARY)"
