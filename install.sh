#!/bin/sh
# Install anything-to-skill.
#
#   curl -fsSL https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.sh | sh
#
# Options (environment variables):
#   VERSION   tag to install, e.g. v0.2.0   (default: latest release)
#   BIN_DIR   where to put the binary        (default: ~/.local/bin)
#
# The download is verified against the release's published SHA256 before it is
# installed. If verification fails, nothing is written.

set -eu

REPO="asale-ai/anything-to-skill"
BIN="anything-to-skill"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1', which was not found"
}

need uname
need mkdir
need tar

# curl or wget, whichever is present.
if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL "$1" -o "$2"; }
    fetch_stdout() { curl -fsSL "$1"; }
elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -qO "$2" "$1"; }
    fetch_stdout() { wget -qO- "$1"; }
else
    die "this installer needs curl or wget"
fi

# ---------------------------------------------------------------- platform ---

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux)  os_part="unknown-linux" ;;
    *)      die "unsupported operating system: $os
Windows users: download the .zip from https://github.com/$REPO/releases/latest" ;;
esac

case "$arch" in
    x86_64 | amd64)  arch_part="x86_64" ;;
    arm64 | aarch64) arch_part="aarch64" ;;
    *)               die "unsupported architecture: $arch" ;;
esac

if [ "$os_part" = "unknown-linux" ]; then
    # Prefer the statically-linked musl build: it runs on any distro, including
    # ones whose glibc is older than the build machine's.
    target="${arch_part}-unknown-linux-musl"
else
    target="${arch_part}-${os_part}"
fi

# ----------------------------------------------------------------- version ---

version="${VERSION:-}"
if [ -z "$version" ]; then
    say "Finding the latest release..."
    version="$(fetch_stdout "https://api.github.com/repos/$REPO/releases/latest" \
        | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
    [ -n "$version" ] || die "could not determine the latest release
Set VERSION explicitly, e.g. VERSION=v0.1.0 sh install.sh"
fi
number="${version#v}"

name="${BIN}-${number}-${target}"
url="https://github.com/$REPO/releases/download/${version}/${name}.tar.gz"

# ---------------------------------------------------------------- download ---

tmp="$(mktemp -d)"
# shellcheck disable=SC2064  # expand $tmp now, not at trap time
trap "rm -rf '$tmp'" EXIT INT TERM

say "Downloading $name ..."
fetch "$url" "$tmp/$name.tar.gz" || die "download failed: $url
Check that a release for $target exists at https://github.com/$REPO/releases"

# Verify before unpacking. A failed check is fatal — never install unverified.
if fetch "https://github.com/$REPO/releases/download/${version}/SHA256SUMS" "$tmp/SHA256SUMS" 2>/dev/null; then
    expected="$(grep " ${name}.tar.gz\$" "$tmp/SHA256SUMS" | awk '{print $1}' | head -n 1)"
    if [ -n "$expected" ]; then
        if command -v sha256sum >/dev/null 2>&1; then
            actual="$(sha256sum "$tmp/$name.tar.gz" | awk '{print $1}')"
        elif command -v shasum >/dev/null 2>&1; then
            actual="$(shasum -a 256 "$tmp/$name.tar.gz" | awk '{print $1}')"
        else
            actual=""
            say "warning: no sha256 tool found — skipping checksum verification"
        fi
        if [ -n "$actual" ]; then
            [ "$actual" = "$expected" ] || die "checksum mismatch — refusing to install
  expected $expected
  actual   $actual"
            say "Checksum verified."
        fi
    else
        say "warning: no checksum published for $name.tar.gz"
    fi
else
    say "warning: SHA256SUMS not available for $version — skipping verification"
fi

# ----------------------------------------------------------------- install ---

tar -C "$tmp" -xzf "$tmp/$name.tar.gz"
[ -f "$tmp/$name/$BIN" ] || die "archive did not contain $BIN"

mkdir -p "$BIN_DIR"
install -m 0755 "$tmp/$name/$BIN" "$BIN_DIR/$BIN" 2>/dev/null \
    || { cp "$tmp/$name/$BIN" "$BIN_DIR/$BIN" && chmod 0755 "$BIN_DIR/$BIN"; }

say ""
say "Installed $BIN $version to $BIN_DIR/$BIN"

case ":$PATH:" in
    *":$BIN_DIR:"*) ;;
    *)
        say ""
        say "$BIN_DIR is not on your PATH. Add it:"
        say "  echo 'export PATH=\"$BIN_DIR:\$PATH\"' >> ~/.profile"
        ;;
esac

say ""
say "Next:"
say "  $BIN check              # see what optional tools are available"
say "  $BIN extract book.pdf   # pull the text out of a document"
