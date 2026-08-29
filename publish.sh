#!/usr/bin/env bash
# Cut a release: bump the version, run the gates, tag, and publish everywhere.
#
#   ./publish.sh patch                     # 0.1.1 -> 0.1.2
#   ./publish.sh minor                     # 0.1.1 -> 0.2.0
#   ./publish.sh major                     # 0.1.1 -> 1.0.0
#   ./publish.sh 0.3.0                     # an exact version
#   ./publish.sh patch -m "Fix the PDF reader"
#   ./publish.sh patch --dry-run           # rehearse the whole thing, change nothing
#
# Options:
#   -m, --message <text>
#                commit message for the release commit
#                (default: "Release vX.Y.Z")
#   --dry-run    run every check and both dry-run publishes, but never commit,
#                push, or publish for real
#   --no-wait    do not wait for the GitHub release build to produce binaries,
#                nor for the workflow to publish the npm package
#   --skip-tests skip fmt/clippy/test (they already ran in CI, say)
#   --local-npm  publish the npm package from this machine instead of waiting
#                for the release workflow to do it. npm asks for a 2FA code
#                interactively, so this needs a terminal and an `npm login`.
#
# There is no confirmation prompt. Once the gates and both rehearsal publishes
# pass, the release runs straight through to the end — so --dry-run is the only
# way to see what a release would do without doing it.
#
# Uncommitted work in the tree is part of the release: it is committed together
# with the version bump, so the tag covers exactly what was tested. Everything
# going into that commit is listed as it happens.
#
# The release has five destinations and this script does them in dependency
# order: the git tag drives the GitHub Actions build that produces the binaries
# install.sh downloads and the npm package fetches on install, so the tag has to
# land — and the build has to finish — before anything advertises the new
# version. Publishing is not reversible — crates.io only allows yanking —
# so everything that can fail is made to fail before the first push.
#
# Four of those five are published from here. npm is not: the release workflow
# publishes it over OIDC (npm trusted publishing), so no npm credential exists
# on this machine or in the repository secrets. This script waits for that
# version to appear on the registry and says so if it never does. --local-npm
# publishes it from here instead, for the case where the workflow could not.

set -euo pipefail

REPO_SLUG="asale-ai/anything-to-skill"
CRATE="anything-to-skill"
SKILL_SLUG="anything-to-skill"
SKILL_OWNER="asale-ai"
NPM_PKG="@asale/anything-to-skill"
NPM_DIR="npm"
MAIN_BRANCH="main"
WAIT_TIMEOUT=1500       # seconds to wait for the release build (25 minutes)
NPM_WAIT_TIMEOUT=900    # seconds to wait for the workflow's npm job (15 minutes)

cd "$(dirname "$0")"

# ------------------------------------------------------------------ output ---

if [ -t 1 ]; then
    BOLD=$(printf '\033[1m'); DIM=$(printf '\033[2m')
    RED=$(printf '\033[31m'); GREEN=$(printf '\033[32m'); YELLOW=$(printf '\033[33m')
    RESET=$(printf '\033[0m')
else
    BOLD=""; DIM=""; RED=""; GREEN=""; YELLOW=""; RESET=""
fi

step() { printf '\n%s==>%s %s%s%s\n' "$GREEN" "$RESET" "$BOLD" "$*" "$RESET"; }
info() { printf '    %s\n' "$*"; }
warn() { printf '%swarning:%s %s\n' "$YELLOW" "$RESET" "$*" >&2; }
die()  { printf '%serror:%s %s\n' "$RED" "$RESET" "$*" >&2; exit 1; }

# ---------------------------------------------------------------- argument ---

LEVEL=""
MESSAGE=""
DRY_RUN=0
WAIT_FOR_BUILD=1
SKIP_TESTS=0
LOCAL_NPM=0

while [ $# -gt 0 ]; do
    case "$1" in
        -m|--message)
            [ $# -ge 2 ] || die "$1 needs a commit message"
            MESSAGE="$2"
            shift
            ;;
        -m*)          MESSAGE="${1#-m}" ;;
        --message=*)  MESSAGE="${1#--message=}" ;;
        --dry-run)    DRY_RUN=1 ;;
        # Kept as a no-op: it used to skip the confirmation prompt, and typing it
        # out of habit should not abort a release.
        --yes|-y)     ;;
        --no-wait)    WAIT_FOR_BUILD=0 ;;
        --skip-tests) SKIP_TESTS=1 ;;
        --local-npm)  LOCAL_NPM=1 ;;
        # The header comment is the help text: print it up to the blank line
        # that ends it, minus the leading "# ".
        -h|--help)    sed -n '2,/^$/p' "$0" | sed 's/^#\{0,1\} \{0,1\}//'; exit 0 ;;
        -*)           die "unknown option: $1" ;;
        *)
            [ -z "$LEVEL" ] || die "give only one version argument (got '$LEVEL' and '$1')"
            LEVEL="$1"
            ;;
    esac
    shift
done

[ -n "$LEVEL" ] || die "say what to release: patch | minor | major | X.Y.Z
Run '$0 --help' for the full usage."

# Reject a typo now rather than after a git fetch and a crates.io round trip.
case "$LEVEL" in
    major|minor|patch) ;;
    [0-9]*.[0-9]*.[0-9]*) ;;
    *) die "'$LEVEL' is not a version or a bump level (patch | minor | major | X.Y.Z)" ;;
esac

# ------------------------------------------------------------------ tokens ---

# Credentials live in .env, which is gitignored. Anything already exported wins,
# so CI can supply them without a file.
if [ -f .env ]; then
    set -a
    # shellcheck disable=SC1091
    . ./.env
    set +a
fi

# cargo reads CARGO_REGISTRY_TOKEN; .env calls it CARGO_API_KEY.
if [ -z "${CARGO_REGISTRY_TOKEN:-}" ] && [ -n "${CARGO_API_KEY:-}" ]; then
    export CARGO_REGISTRY_TOKEN="$CARGO_API_KEY"
fi
# The publish steps name the registry explicitly (--registry crates-io), and for
# a named registry cargo looks up CARGO_REGISTRIES_<NAME>_TOKEN instead.
if [ -n "${CARGO_REGISTRY_TOKEN:-}" ]; then
    export CARGO_REGISTRIES_CRATES_IO_TOKEN="${CARGO_REGISTRIES_CRATES_IO_TOKEN:-$CARGO_REGISTRY_TOKEN}"
fi

# No npm credential is read here. npm publishes through trusted publishing in
# the release workflow, and this package is set to require 2FA and disallow
# bypass tokens — so an automation token could not publish it even if one
# existed. --local-npm uses the machine's own `npm login` and its 2FA prompt.

# ---------------------------------------------------------------- preflight ---

step "Preflight"

for tool in git cargo clawhub npm curl sed awk; do
    command -v "$tool" >/dev/null 2>&1 || die "this script needs '$tool', which was not found"
done
if [ "$WAIT_FOR_BUILD" = 1 ]; then
    command -v gh >/dev/null 2>&1 || die "waiting for the release build needs the 'gh' CLI.
Install it, or pass --no-wait."
fi

git rev-parse --git-dir >/dev/null 2>&1 || die "not inside a git repository"

branch="$(git rev-parse --abbrev-ref HEAD)"
[ "$branch" = "$MAIN_BRANCH" ] || die "on branch '$branch', but releases are cut from '$MAIN_BRANCH'"

# Uncommitted work is not an error — it ships with this release. It is listed
# here and again just before the commit, because `git add -A` will sweep up
# anything lying around that git is not already ignoring.
pending="$(git status --porcelain)"
if [ -n "$pending" ]; then
    info "Uncommitted changes — these will be committed with the version bump:"
    git status --short | sed 's/^/      /'
else
    info "Working tree is clean; the release commit will be the version bump alone."
fi

info "Fetching origin..."
git fetch --quiet origin "$MAIN_BRANCH" --tags
local_head="$(git rev-parse HEAD)"
remote_head="$(git rev-parse "origin/$MAIN_BRANCH")"
if [ "$local_head" != "$remote_head" ]; then
    if git merge-base --is-ancestor "$remote_head" "$local_head"; then
        info "Local $MAIN_BRANCH is ahead of origin; the release push will include those commits."
    else
        die "local $MAIN_BRANCH is behind or has diverged from origin/$MAIN_BRANCH. Pull first."
    fi
fi

[ -n "${CARGO_REGISTRY_TOKEN:-}" ] \
    || die "no crates.io token. Put CARGO_API_KEY in .env, export CARGO_REGISTRY_TOKEN, or run 'cargo login'."

clawhub whoami >/dev/null 2>&1 \
    || die "clawhub is not authenticated. Run 'clawhub login' (or set CLAWHUB_API_KEY in .env)."
info "clawhub user: $(clawhub whoami 2>/dev/null | tail -1)"

[ -f "$NPM_DIR/package.json" ] \
    || die "$NPM_DIR/package.json is missing — the npm launcher is part of the release."

# The registry is named on every npm call for the same reason the cargo steps
# pass --registry crates-io: a machine with a mirror configured —
# registry.npmmirror.com is a common one — would have `npm view` answer "no
# such version" about a registry nobody publishes to, and --local-npm push the
# release somewhere nobody installs from. A mirror is a read-only cache.
NPM_REGISTRY="https://registry.npmjs.org/"

npm_run() { ( cd "$NPM_DIR" && npm --registry "$NPM_REGISTRY" "$@" ); }

if [ "$LOCAL_NPM" = 1 ]; then
    # Checked here, not at the publish step. That step is the last thing the
    # release does — after the commit, the tag push and crates.io — and
    # discovering then that nothing can answer npm's 2FA prompt is the worst
    # possible moment to discover it.
    [ -t 0 ] \
        || die "--local-npm needs a terminal: npm asks for a 2FA code interactively."
    npm_whoami="$(npm_run whoami 2>/dev/null || true)"
    [ -n "$npm_whoami" ] \
        || die "--local-npm needs an npm login. Run:
  npm login --registry $NPM_REGISTRY"
    info "npm user: $npm_whoami  (--local-npm: publishing npm from this machine)"
else
    info "npm: published by the release workflow over OIDC; this script waits for it"
fi

# ----------------------------------------------------------------- version ---

step "Version"

current="$(awk '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && /^version[[:space:]]*=/ && !found {
        gsub(/^version[[:space:]]*=[[:space:]]*"|"[[:space:]]*$/, "")
        print; found = 1
    }
' Cargo.toml)"
[ -n "$current" ] || die "could not read the current version out of Cargo.toml"

case "$LEVEL" in
    major|minor|patch)
        IFS=. read -r cur_major cur_minor cur_patch <<EOF
$current
EOF
        case "$cur_major$cur_minor$cur_patch" in
            *[!0-9]*|"") die "current version '$current' is not X.Y.Z — bump it by hand" ;;
        esac
        case "$LEVEL" in
            major) version="$((cur_major + 1)).0.0" ;;
            minor) version="${cur_major}.$((cur_minor + 1)).0" ;;
            patch) version="${cur_major}.${cur_minor}.$((cur_patch + 1))" ;;
        esac
        ;;
    # An explicit version; its shape was already checked during argument parsing.
    *) version="$LEVEL" ;;
esac

tag="v$version"
info "$current  ->  $BOLD$version$RESET   (tag $tag)"

# Every destination is checked up front: finding out that the tag is taken after
# crates.io has already accepted the upload leaves a half-published release.
git rev-parse -q --verify "refs/tags/$tag" >/dev/null \
    && die "tag $tag already exists locally. Pick another version, or delete it with 'git tag -d $tag'."
git ls-remote --exit-code --tags origin "refs/tags/$tag" >/dev/null 2>&1 \
    && die "tag $tag already exists on origin. Pick another version."

info "Checking crates.io..."
published="$(curl -fsSL -H "User-Agent: $CRATE release script" \
    "https://crates.io/api/v1/crates/$CRATE/$version" 2>/dev/null || true)"
case "$published" in
    *'"num":"'"$version"'"'*)
        die "$CRATE $version is already on crates.io. Versions cannot be replaced — pick another." ;;
esac

if npm_run view "$NPM_PKG@$version" version >/dev/null 2>&1; then
    die "$NPM_PKG@$version is already on npm. Versions cannot be replaced — pick another."
fi

# ------------------------------------------------------------------- write ---

step "Bumping Cargo.toml and package.json"

# Keep byte-for-byte copies to roll back to. `git checkout` would be wrong here:
# the tree is allowed to carry uncommitted work, and checkout would throw away
# any of it that touched these two files.
backup_dir="$(mktemp -d)"
cp Cargo.toml "$backup_dir/Cargo.toml"
[ -f Cargo.lock ] && cp Cargo.lock "$backup_dir/Cargo.lock"
cp "$NPM_DIR/package.json" "$backup_dir/package.json"

restore_tree() {
    cp "$backup_dir/Cargo.toml" Cargo.toml 2>/dev/null || true
    [ -f "$backup_dir/Cargo.lock" ] && cp "$backup_dir/Cargo.lock" Cargo.lock 2>/dev/null
    cp "$backup_dir/package.json" "$NPM_DIR/package.json" 2>/dev/null || true
    # Copied in for packing, not checked in.
    rm -f "$NPM_DIR/LICENSE"
    rm -rf "$backup_dir"
    return 0
}
# Armed before the lockfile sync, because that is the first thing that can fail
# with the tree already modified.
trap 'restore_tree' EXIT

awk -v v="$version" '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && /^version[[:space:]]*=/ && !done {
        sub(/"[^"]*"/, "\"" v "\""); done = 1
    }
    { print }
' Cargo.toml > Cargo.toml.tmp && mv Cargo.toml.tmp Cargo.toml

# Read it back rather than diffing against HEAD: Cargo.toml may well have been
# modified before this script ran, which would make a diff prove nothing.
written="$(awk '
    /^\[/ { in_pkg = ($0 == "[package]") }
    in_pkg && /^version[[:space:]]*=/ && !found {
        gsub(/^version[[:space:]]*=[[:space:]]*"|"[[:space:]]*$/, "")
        print; found = 1
    }
' Cargo.toml)"
[ "$written" = "$version" ] \
    || die "the version bump did not apply — Cargo.toml still reads '$written'"

# The release workflow builds with --locked, so Cargo.lock has to record the new
# version before it is committed or that build fails on a stale lockfile.
cargo check --quiet --offline >/dev/null 2>&1 \
    || cargo check --quiet >/dev/null \
    || die "cargo check failed, so Cargo.lock could not be brought up to date"
# npm's own `version` command would tag and commit; this only edits the field.
# The package version is not merely cosmetic — the launcher downloads the
# GitHub release for its own version, so a mismatch fetches the wrong binary or
# a tag that does not exist.
npm_run pkg set "version=$version" >/dev/null \
    || die "could not set the version in $NPM_DIR/package.json"
npm_written="$(npm_run pkg get version | tr -d '"')"
[ "$npm_written" = "$version" ] \
    || die "the npm version bump did not apply — package.json still reads '$npm_written'"

# The package's `files` list names LICENSE, and there is one licence in this
# repository rather than a copy per package directory.
cp LICENSE "$NPM_DIR/LICENSE"

info "Cargo.toml, Cargo.lock and $NPM_DIR/package.json now say $version"

# ------------------------------------------------------------------- gates ---

if [ "$SKIP_TESTS" = 1 ]; then
    warn "skipping fmt/clippy/tests because --skip-tests was passed"
else
    step "Gates"
    info "cargo fmt --check"
    cargo fmt --all --check || die "formatting check failed — run 'cargo fmt --all'"
    info "cargo clippy"
    cargo clippy --all-targets --all-features -- -D warnings || die "clippy failed"
    info "cargo test"
    cargo test --all-features || die "tests failed"
fi

step "Rehearsing the publishes"
info "cargo publish --dry-run"
# --registry crates-io is not redundant: a machine with a mirror configured as
# [source.crates-io] replace-with = ... cannot publish at all without it, and
# naming the registry is harmless when no replacement is set up.
cargo publish --registry crates-io --dry-run --allow-dirty --quiet \
    || die "cargo publish --dry-run failed"
info "npm publish --dry-run"
npm_run publish --dry-run >/dev/null \
    || die "npm publish --dry-run failed"
info "clawhub publish --dry-run"
clawhub publish . --slug "$SKILL_SLUG" --owner "$SKILL_OWNER" --version "$version" --dry-run \
    || die "clawhub publish --dry-run failed"

if [ "$DRY_RUN" = 1 ]; then
    step "Dry run complete"
    info "Everything that can be checked without publishing passed."
    info "The version bump has been rolled back; nothing was committed or pushed."
    exit 0
fi

# ------------------------------------------------------- point of no return ---

message="${MESSAGE:-Release $tag}"

# Only for the listing below — the staging itself happens at commit time. Doing
# it here instead would leave a loaded index behind if any later step failed.
changes="$(git status --porcelain | sed 's/^...//')"
[ -n "$changes" ] || die "nothing to commit — the version bump produced no change"
change_count="$(printf '%s\n' "$changes" | wc -l | tr -d ' ')"

if [ "$LOCAL_NPM" = 1 ]; then
    npm_plan_line="publish $NPM_PKG@$version to npm from here    ${DIM}— npm will ask for 2FA${RESET}"
else
    npm_plan_line="wait for the workflow to publish $NPM_PKG@$version to npm"
fi

step "Releasing $CRATE $version"
cat <<EOF
    Commit message: ${BOLD}${message}${RESET}
    Files in the commit ($change_count):
$(printf '%s\n' "$changes" | sed 's/^/      /')

    Now, in order:
      1. commit those $change_count file(s) and tag the commit $tag
      2. push $MAIN_BRANCH and $tag to origin
         (the tag starts the GitHub Actions build that produces the binaries
          install.sh downloads)
      3. publish $CRATE $version to crates.io      ${DIM}— cannot be undone, only yanked${RESET}
      4. $npm_plan_line
      5. publish the skill to ClawHub as $SKILL_OWNER/$SKILL_SLUG@$version
EOF

step "Committing and tagging"
git add -A
git commit --quiet -m "$message"
git tag -a "$tag" -m "$CRATE $version"
info "committed $(git rev-parse --short HEAD), tagged $tag"

# The bump is now part of history, so there is nothing left to roll back to.
rm -rf "$backup_dir"

# From here a failure leaves real state behind, so the trap explains where.
trap 'printf "\n%serror:%s the release stopped partway through.\n  Local commit and tag %s exist.\n  To undo (only safe if nothing was pushed):\n    git tag -d %s && git reset --hard HEAD~1\n" "$RED" "$RESET" "$tag" "$tag" >&2' EXIT

step "Pushing"
git push --quiet origin "$MAIN_BRANCH"
git push --quiet origin "$tag"
info "pushed $MAIN_BRANCH and $tag"
info "release build: https://github.com/$REPO_SLUG/actions/workflows/release.yml"

trap 'printf "\n%serror:%s the release stopped after pushing %s.\n  The tag is public and the GitHub build may already have run.\n  Re-run the remaining steps by hand, or release %s next.\n" "$RED" "$RESET" "$tag" "$tag" >&2' EXIT

step "Publishing to crates.io"
cargo publish --registry crates-io --quiet \
    || die "cargo publish failed. The tag $tag is already pushed;
fix the problem and run 'cargo publish --registry crates-io' by hand,
or yank and release a new version."
info "$CRATE $version is on crates.io"

if [ "$WAIT_FOR_BUILD" = 1 ]; then
    step "Waiting for the release binaries"
    info "install.sh serves these, so the skill should not announce $version before they exist."
    deadline=$(( $(date +%s) + WAIT_TIMEOUT ))
    while :; do
        if gh release view "$tag" --repo "$REPO_SLUG" --json assets \
             --jq '[.assets[].name] | index("SHA256SUMS")' 2>/dev/null | grep -qv '^null$'; then
            info "release assets are up"
            break
        fi
        if [ "$(date +%s)" -ge "$deadline" ]; then
            warn "gave up after $((WAIT_TIMEOUT / 60)) minutes. The build may still be running:
    https://github.com/$REPO_SLUG/actions/workflows/release.yml
  Publishing the skill anyway."
            break
        fi
        sleep 20
    done
fi

# npm goes after the wait and not before it. The published package downloads
# the GitHub release for its own version on install, so publishing it while the
# build is still running would ship a version that cannot install itself.
if [ "$LOCAL_NPM" = 1 ]; then
    step "Publishing $NPM_PKG to npm"
    info "npm will ask for a 2FA code now."
    npm_run publish \
        || die "npm publish failed. Everything before it shipped; retry with:
  ( cd $NPM_DIR && cp ../LICENSE LICENSE && npm publish --registry $NPM_REGISTRY )"
    info "$NPM_PKG@$version is on npm"
elif [ "$WAIT_FOR_BUILD" = 0 ]; then
    # --no-wait already declined to wait for the build the npm job runs after,
    # so waiting for the job itself would contradict the flag.
    step "Not waiting for $NPM_PKG"
    info "--no-wait was passed. Confirm the workflow published it:"
    info "  npm view $NPM_PKG@$version version --registry $NPM_REGISTRY"
else
    step "Waiting for the workflow to publish $NPM_PKG"
    info "The npm job runs after the release job and publishes over OIDC."
    # Confirmed against the registry rather than read off a green workflow: the
    # npm job is guarded by `if: startsWith(github.ref, 'refs/tags/v')`, so a
    # workflow_dispatch run skips it — and a skipped job does not fail the run.
    npm_deadline=$(( $(date +%s) + NPM_WAIT_TIMEOUT ))
    npm_published=0
    while :; do
        if npm_run view "$NPM_PKG@$version" version >/dev/null 2>&1; then
            npm_published=1
            info "$NPM_PKG@$version is on npm"
            break
        fi
        [ "$(date +%s)" -ge "$npm_deadline" ] && break
        sleep 20
    done
    if [ "$npm_published" = 0 ]; then
        warn "$NPM_PKG@$version is not on npm after $((NPM_WAIT_TIMEOUT / 60)) minutes.
  Check the npm job: https://github.com/$REPO_SLUG/actions/workflows/release.yml
  Everything else shipped. The binaries and the tag are already public, so
  publishing npm afterwards is safe — re-run the workflow, or from here:
    ./publish.sh $version --local-npm    (or by hand)
    ( cd $NPM_DIR && cp ../LICENSE LICENSE && npm publish --registry $NPM_REGISTRY )"
    fi
fi
rm -f "$NPM_DIR/LICENSE"

step "Publishing the skill to ClawHub"
clawhub publish . \
    --slug "$SKILL_SLUG" \
    --owner "$SKILL_OWNER" \
    --version "$version" \
    --source-repo "$REPO_SLUG" \
    --source-commit "$(git rev-parse HEAD)" \
    --source-ref "$tag" \
    --source-path SKILL.md \
    --changelog "Release $tag" \
    || die "clawhub publish failed. Everything else shipped; retry with:
  clawhub publish . --slug $SKILL_SLUG --owner $SKILL_OWNER --version $version"

trap - EXIT

step "Released $CRATE $version"
cat <<EOF
    crates.io  https://crates.io/crates/$CRATE/$version
    npm        https://www.npmjs.com/package/$NPM_PKG/v/$version
    release    https://github.com/$REPO_SLUG/releases/tag/$tag
    skill      clawhub install @$SKILL_OWNER/$SKILL_SLUG

    Verify it installs the way the README says it does:
      npx -y $NPM_PKG@$version --version
EOF
