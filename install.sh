#!/bin/sh
# maapp installer — https://github.com/yangsi7/maapp
#
# Usage:
#   ./install.sh              install (release download when published, else build from source)
#   ./install.sh --uninstall  remove the installed binary
#
# Security posture:
#   - No sudo, ever: installs to a user-writable dir ($MAAPP_INSTALL, else ~/.local/bin).
#   - Release mode (once published) downloads over HTTPS only and verifies the
#     sha256 checksum of the artifact BEFORE installing (content verification,
#     never a size/status proxy; hard fail on mismatch).
#   - Read the script before piping it to sh:
#     https://raw.githubusercontent.com/yangsi7/maapp/main/install.sh
#
# Release artifacts are produced by dist (axodotdev/cargo-dist, pinned in
# dist-workspace.toml): maapp-<target-triple>.tar.xz per platform, each with a
# .sha256 next to it, plus one unified sha256.sum — uploaded to GitHub
# Releases. That "versionless" <name>-<target><ext> naming is also one of
# cargo-binstall's default lookup patterns, so `cargo binstall maapp` finds
# the same artifacts without extra metadata.

set -u

# The GitHub org/user that owns the public maapp repo. Once a tagged release
# exists, release download is attempted; otherwise the script falls back to
# building from source (MODE B below).
OWNER="yangsi7"

# GitHub releases download base, dist convention. Derived from OWNER. When no
# tagged release exists yet, the download 404s and the source-build fallback
# (MODE B) takes over.
RELEASE_BASE="${OWNER:+https://github.com/$OWNER/maapp/releases/latest/download}"

say() { printf '%s\n' "$*"; }
err() { printf 'maapp-install: %s\n' "$*" >&2; }

# uname -s/-m -> Rust target triple. Handles Rosetta (an x86_64 report under
# translation is really an arm64 mac) and musl (Alpine et al. via ldd banner).
detect_triple() {
    os=$(uname -s)
    arch=$(uname -m)
    case "$os" in
        Darwin)
            if [ "$arch" = "x86_64" ] && [ "$(sysctl -n sysctl.proc_translated 2>/dev/null || echo 0)" = "1" ]; then
                arch=arm64
            fi
            case "$arch" in
                arm64 | aarch64) echo "aarch64-apple-darwin" ;;
                x86_64) echo "x86_64-apple-darwin" ;;
                *) return 1 ;;
            esac
            ;;
        Linux)
            libc=gnu
            if command -v ldd >/dev/null 2>&1 && ldd --version 2>&1 | grep -qi musl; then
                libc=musl
            fi
            case "$arch" in
                x86_64) echo "x86_64-unknown-linux-$libc" ;;
                aarch64 | arm64) echo "aarch64-unknown-linux-$libc" ;;
                *) return 1 ;;
            esac
            ;;
        *) return 1 ;;
    esac
}

# One-line PATH advice, never a profile mutation. Rationale: rustup's env-file
# + rc-edit machinery is a lot of moving parts for a single static binary; a
# printed line keeps the script transparent, idempotent, and shell-agnostic.
check_path() {
    case ":$PATH:" in
        *":$1:"*) ;;
        *)
            say ""
            say "NOTE: $1 is not on your PATH. Add it with:"
            say "  export PATH=\"$1:\$PATH\""
            ;;
    esac
}

# MODE A: download the published release artifact and verify its checksum
# BEFORE install. Hard fail on any mismatch — a corrupt artifact must never
# reach the install dir.
# Prebuilt artifacts cover the targets in dist-workspace.toml (aarch64/x86_64
# macOS, x86_64 linux-gnu; windows ships as a .zip via the powershell
# installer). A detected triple without a prebuilt artifact (aarch64 linux,
# musl) fails the download — clone the repo and run ./install.sh from the
# checkout to build from source instead.
install_release() {
    triple=$1
    install_dir=$2
    artifact="maapp-$triple.tar.xz"

    tmp=$(mktemp -d) || {
        err "could not create a temp dir"
        return 1
    }
    trap 'rm -rf "$tmp"' EXIT

    say "downloading $RELEASE_BASE/$artifact"
    curl -fsSL -o "$tmp/$artifact" "$RELEASE_BASE/$artifact" || {
        err "download failed: $RELEASE_BASE/$artifact"
        return 1
    }
    curl -fsSL -o "$tmp/sha256.sum" "$RELEASE_BASE/sha256.sum" || {
        err "checksum download failed: $RELEASE_BASE/sha256.sum"
        return 1
    }

    say "verifying sha256 checksum"
    if command -v sha256sum >/dev/null 2>&1; then
        (cd "$tmp" && grep "$artifact" sha256.sum | sha256sum -c -) || {
            err "CHECKSUM MISMATCH for $artifact — refusing to install"
            return 1
        }
    else
        (cd "$tmp" && grep "$artifact" sha256.sum | shasum -a 256 -c -) || {
            err "CHECKSUM MISMATCH for $artifact — refusing to install"
            return 1
        }
    fi

    tar -xJf "$tmp/$artifact" -C "$tmp" || {
        err "could not extract $artifact"
        return 1
    }
    # cargo-dist layouts nest the binary under maapp-<triple>/; accept both.
    if [ -x "$tmp/maapp" ]; then
        bin="$tmp/maapp"
    elif [ -x "$tmp/maapp-$triple/maapp" ]; then
        bin="$tmp/maapp-$triple/maapp"
    else
        err "extracted archive does not contain a maapp binary"
        return 1
    fi
    install_binary "$bin" "$install_dir"
}

# MODE B: build from source. Only valid when this script sits inside a maapp
# checkout (Cargo.toml with name = "maapp" next to it).
install_source() {
    script_dir=$1
    install_dir=$2

    if [ ! -f "$script_dir/Cargo.toml" ] || ! grep -q '^name = "maapp"' "$script_dir/Cargo.toml"; then
        err "no release published yet and this is not a maapp checkout"
        err "clone the repo and run ./install.sh from its root: git clone <repo> && cd maapp && ./install.sh"
        return 1
    fi
    if ! command -v cargo >/dev/null 2>&1; then
        err "building from source needs cargo (https://rustup.rs)"
        return 1
    fi

    say "building from source (cargo build --release --locked)"
    (cd "$script_dir" && cargo build --release --locked) || {
        err "cargo build failed"
        return 1
    }
    install_binary "$script_dir/target/release/maapp" "$install_dir"
}

# Copy the built/downloaded binary into place. rm first so re-running is an
# upgrade even while an old copy is running (no 'text file busy' on Linux).
install_binary() {
    src=$1
    install_dir=$2
    mkdir -p "$install_dir" || {
        err "could not create $install_dir"
        return 1
    }
    rm -f "$install_dir/maapp"
    cp "$src" "$install_dir/maapp" || {
        err "could not copy maapp into $install_dir"
        return 1
    }
    chmod 755 "$install_dir/maapp"
    say "installed $install_dir/maapp"
}

uninstall() {
    install_dir=$1
    if [ -f "$install_dir/maapp" ]; then
        rm -f "$install_dir/maapp"
        say "removed $install_dir/maapp"
    else
        say "nothing to remove: $install_dir/maapp is not installed"
    fi
}

main() {
    install_dir="${MAAPP_INSTALL:-$HOME/.local/bin}"

    if [ "${1:-}" = "--uninstall" ]; then
        uninstall "$install_dir"
        return 0
    fi

    triple=$(detect_triple) || {
        err "unsupported platform: $(uname -s) $(uname -m)"
        err "supported: aarch64/x86_64 macOS, x86_64/aarch64 Linux (gnu or musl)"
        return 1
    }
    say "platform: $triple"
    say "install dir: $install_dir"

    script_dir=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd) || return 1

    if [ -n "$RELEASE_BASE" ]; then
        install_release "$triple" "$install_dir" || return 1
    else
        say "no release published yet; falling back to a source build"
        install_source "$script_dir" "$install_dir" || return 1
    fi

    check_path "$install_dir"
    say ""
    "$install_dir/maapp" --version || return 1
    say "next: run 'maapp init' inside a project to wire it for the graph loop"
}

main "$@" || exit 1
