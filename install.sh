#!/bin/sh
# Install anything-to-skill.
#
#   curl -fsSL https://raw.githubusercontent.com/asale-ai/anything-to-skill/main/install.sh | sh
#
# Options (environment variables):
#   VERSION   release to install, v0.2.0 or 0.2.0 (default: latest release)
#   BIN_DIR   where to put the binary            (default: ~/.local/bin)
#   SKILL     1 to install the skill too         (default: 0, binary only)
#   SKILL_DIR where to put the skill             (default: ~/.agents/skills)
#
# The download is verified against the release's published SHA256 before it is
# installed. Verification failing, the checksum file being unreachable, and no
# SHA256 tool being available are all fatal — nothing is written in any of them.
# Set INSECURE_SKIP_VERIFY=1 to install without the check anyway (unsafe: it
# leaves you trusting the network).

set -eu

REPO="asale-ai/anything-to-skill"
BIN="anything-to-skill"
BIN_DIR="${BIN_DIR:-$HOME/.local/bin}"
SKILL_DIR="${SKILL_DIR:-$HOME/.agents/skills}"

say() { printf '%s\n' "$*"; }
die() { printf 'error: %s\n' "$*" >&2; exit 1; }

need() {
    command -v "$1" >/dev/null 2>&1 || die "this installer needs '$1', which was not found"
}

for tool in uname mkdir mktemp tar sed awk grep cp mv chmod rm; do
    need "$tool"
done

# curl or wget, whichever is present. Both are pinned to HTTPS across redirects:
# a release download redirects to a separate asset host, and without this a
# hijacked redirect could quietly drop the transfer down to plaintext HTTP.
if command -v curl >/dev/null 2>&1; then
    CURL_OPTS="-fsSL --proto =https --proto-redir =https --tlsv1.2 --retry 3"
    # shellcheck disable=SC2086  # word splitting of the option list is intended
    fetch() { curl $CURL_OPTS "$1" -o "$2"; }
    # shellcheck disable=SC2086
    fetch_stdout() { curl $CURL_OPTS "$1"; }
elif command -v wget >/dev/null 2>&1; then
    # busybox wget has neither flag, so only pass them when they are understood.
    WGET_OPTS="-q"
    if wget --help 2>&1 | grep -q -- '--https-only'; then
        WGET_OPTS="$WGET_OPTS --https-only --secure-protocol=TLSv1_2"
    fi
    # shellcheck disable=SC2086
    fetch() { wget $WGET_OPTS -O "$2" "$1"; }
    # shellcheck disable=SC2086
    fetch_stdout() { wget $WGET_OPTS -O- "$1"; }
else
    die "this installer needs curl or wget"
fi

# Print the SHA256 of a file, or return non-zero if no tool here produced one.
# The result is checked for shape rather than trusted: piping through awk hides
# the tool's own exit status, so a tool that is present but broken would
# otherwise hand back an empty string that reads as a checksum mismatch.
sha256_of() {
    hash=""
    if command -v sha256sum >/dev/null 2>&1; then
        hash="$(sha256sum "$1" | awk '{print $1}')"
    elif command -v shasum >/dev/null 2>&1; then
        hash="$(shasum -a 256 "$1" | awk '{print $1}')"
    elif command -v openssl >/dev/null 2>&1; then
        hash="$(openssl dgst -sha256 "$1" | awk '{print $NF}')"
    fi
    [ ${#hash} -eq 64 ] || return 1
    case "$hash" in
        *[!0-9a-fA-F]*) return 1 ;;
    esac
    printf '%s\n' "$hash"
}

# ---------------------------------------------------------------- platform ---

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux)  os_part="unknown-linux" ;;
    *)      die "unsupported operating system: $os
Windows users: run this in PowerShell instead
  irm https://raw.githubusercontent.com/$REPO/main/install.ps1 | iex" ;;
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
else
    # Releases are tagged "vX.Y.Z" while the archives are named "X.Y.Z", so a
    # bare VERSION=0.1.0 is the obvious thing to type. Accept both spellings.
    case "$version" in
        v*) ;;
        *)  version="v$version" ;;
    esac
fi
number="${version#v}"

name="${BIN}-${number}-${target}"
url="https://github.com/$REPO/releases/download/${version}/${name}.tar.gz"

# ---------------------------------------------------------------- download ---

tmp="$(mktemp -d)"
cleanup() { rm -rf "$tmp"; }
trap cleanup EXIT
# Without an explicit exit these signals would run the handler and then let the
# script carry on against a temp directory that is no longer there.
trap 'cleanup; exit 130' INT
trap 'cleanup; exit 143' TERM

say "Downloading $name ..."
fetch "$url" "$tmp/$name.tar.gz" || die "download failed: $url
Check that a release for $target exists at https://github.com/$REPO/releases"

# Verify before unpacking, so a tampered archive is never even extracted.
if [ "${INSECURE_SKIP_VERIFY:-0}" = "1" ]; then
    say "warning: INSECURE_SKIP_VERIFY=1 — installing without verifying the download"
else
    sums_url="https://github.com/$REPO/releases/download/${version}/SHA256SUMS"
    fetch "$sums_url" "$tmp/SHA256SUMS" || die "could not download the checksums: $sums_url
Refusing to install unverified. Retry, or set INSECURE_SKIP_VERIFY=1 to bypass."

    # Escape the regex metacharacters in the filename (it contains dots) so the
    # pattern matches literally rather than treating them as wildcards.
    escaped="$(printf '%s' "${name}.tar.gz" | sed 's/[.[\*^$]/\\&/g')"
    # Anchored, and the hash has to look like one. The trailing [[:space:]]* is
    # for CR bytes, which the Windows half of the release build leaves behind.
    expected="$(sed -n "s/^\([0-9a-fA-F]\{64\}\)  *${escaped}[[:space:]]*\$/\1/p" \
        "$tmp/SHA256SUMS" | head -n 1)"
    [ -n "$expected" ] || die "no checksum published for ${name}.tar.gz
Refusing to install unverified. Set INSECURE_SKIP_VERIFY=1 to bypass."

    actual="$(sha256_of "$tmp/$name.tar.gz")" || die "could not compute the download's SHA256
Needs a working sha256sum, shasum, or openssl. Refusing to install unverified.
Set INSECURE_SKIP_VERIFY=1 to bypass."

    [ "$actual" = "$expected" ] || die "checksum mismatch — refusing to install
  expected $expected
  actual   $actual"
    say "Checksum verified."
fi

# ----------------------------------------------------------------- install ---

tar -C "$tmp" -xzf "$tmp/$name.tar.gz"
[ -f "$tmp/$name/$BIN" ] || die "archive did not contain $BIN"

mkdir -p "$BIN_DIR" || die "could not create $BIN_DIR"

# Stage next to the destination and rename over it. The rename is atomic, so an
# interrupted install cannot leave a half-written binary, and replacing a copy
# that is currently running works instead of failing with ETXTBSY.
dest="$BIN_DIR/$BIN"
staged="$dest.new.$$"
cp "$tmp/$name/$BIN" "$staged" || die "could not write to $BIN_DIR — check permissions"
chmod 0755 "$staged"
mv -f "$staged" "$dest" || { rm -f "$staged"; die "could not install to $dest"; }

say ""
say "Installed $BIN $version to $dest"

# ------------------------------------------------------------------- skill ---

# Off unless asked for. This script's job is the binary, and writing into an
# agent's configuration is not a thing to do to someone who only asked for that.
#
# The skill goes to ~/.agents/skills/, the vendor-neutral path Codex, Cursor,
# Gemini CLI, and opencode all read. Claude Code reads only ~/.claude/skills/,
# so it gets a symlink to the same directory rather than a second copy that
# would then have to be kept in step.
#
# Both files come out of the archive verified above, so nothing here is fetched
# a second time or trusted unchecked. `npx skills add` does all of this and
# covers many more agents; this path exists for machines without Node.
if [ "${SKILL:-0}" = "1" ]; then
    skill_dest="$SKILL_DIR/$BIN"

    mkdir -p "$skill_dest" || die "could not create $skill_dest"
    # Copied file by file rather than replacing the directory: anything else the
    # user keeps in there is theirs, and an installer should not eat it.
    cp "$tmp/$name/SKILL.md" "$skill_dest/SKILL.md" || die "could not write $skill_dest/SKILL.md"
    cp "$tmp/$name/LICENSE" "$skill_dest/LICENSE"   || die "could not write $skill_dest/LICENSE"

    say ""
    say "Installed the skill to $skill_dest"

    # Only for agents that are actually here — creating a configuration
    # directory for a tool this machine does not have is litter, not support.
    if [ -d "$HOME/.claude" ] && command -v ln >/dev/null 2>&1; then
        link="$HOME/.claude/skills/$BIN"
        if [ -e "$link" ] && [ ! -L "$link" ]; then
            say "  $link already exists and is not a link — left alone"
        else
            # Not fatal: the skill is installed either way, and a filesystem
            # without symlinks is a reason to tell the user, not to fail.
            if mkdir -p "$HOME/.claude/skills" && rm -f "$link" && ln -s "$skill_dest" "$link"; then
                say "  linked into ~/.claude/skills/ for Claude Code"
            else
                say "  could not link into ~/.claude/skills/ — copy $skill_dest there by hand"
            fi
        fi
    fi
fi

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
