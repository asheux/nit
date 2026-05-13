#!/usr/bin/env bash
# nit installer for macOS / Linux / WSL.
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/asheux/nit/main/install.sh | bash
#
# Environment overrides:
#   NIT_VERSION   Tag to install (default: latest). Example: v0.1.0.
#   NIT_INSTALL_DIR  Where to put the binaries (default: $HOME/.nit/bin).
#   NIT_REPO      owner/repo (default: asheux/nit).
#   NIT_NO_MODIFY_PATH  Set to 1 to skip the PATH-export hint.

set -euo pipefail

NIT_REPO="${NIT_REPO:-asheux/nit}"
NIT_VERSION="${NIT_VERSION:-latest}"
NIT_INSTALL_DIR="${NIT_INSTALL_DIR:-$HOME/.nit/bin}"

err() { printf '\033[1;31merror:\033[0m %s\n' "$*" >&2; exit 1; }
info() { printf '\033[1;34minfo:\033[0m  %s\n' "$*" >&2; }
warn() { printf '\033[1;33mwarn:\033[0m  %s\n' "$*" >&2; }

need() { command -v "$1" >/dev/null 2>&1 || err "$1 is required but not installed."; }

need uname
need tar
need mkdir
if command -v curl >/dev/null 2>&1; then
  fetch() { curl -fsSL --retry 3 --retry-delay 2 -o "$2" "$1"; }
elif command -v wget >/dev/null 2>&1; then
  fetch() { wget -q -O "$2" "$1"; }
else
  err "curl or wget is required."
fi

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Darwin) os_tag="apple-darwin" ;;
    Linux)  os_tag="unknown-linux-gnu" ;;
    *)      err "Unsupported OS: $os. nit currently ships binaries for macOS and Linux." ;;
  esac

  case "$arch" in
    arm64|aarch64) arch_tag="aarch64" ;;
    x86_64|amd64)  arch_tag="x86_64" ;;
    *)             err "Unsupported architecture: $arch." ;;
  esac

  # Linux arm64 is not yet shipped; surface a friendly message.
  if [ "$os" = "Linux" ] && [ "$arch_tag" = "aarch64" ]; then
    err "Linux aarch64 prebuilt binaries are not yet published. Build from source: cargo build --release."
  fi

  echo "${arch_tag}-${os_tag}"
}

resolve_tag() {
  if [ "$NIT_VERSION" != "latest" ]; then
    echo "$NIT_VERSION"
    return
  fi
  # GitHub API returns the latest non-prerelease release.
  local api_url="https://api.github.com/repos/${NIT_REPO}/releases/latest"
  local tmp
  tmp="$(mktemp)"
  fetch "$api_url" "$tmp" || err "Failed to query latest release from $api_url"
  local tag
  tag="$(grep -E '"tag_name":' "$tmp" | head -1 | sed -E 's/.*"tag_name": *"([^"]+)".*/\1/')"
  rm -f "$tmp"
  [ -n "$tag" ] || err "Could not parse latest release tag."
  echo "$tag"
}

verify_sha256() {
  local file="$1" expected="$2"
  local actual
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$file" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$file" | awk '{print $1}')"
  else
    warn "No sha256sum / shasum available; skipping checksum verification."
    return 0
  fi
  if [ "$actual" != "$expected" ]; then
    err "Checksum mismatch for $(basename "$file"): expected $expected, got $actual"
  fi
}

main() {
  local target tag asset asset_url sums_url tmp_dir
  target="$(detect_target)"
  tag="$(resolve_tag)"
  asset="nit-${tag}-${target}.tar.gz"
  asset_url="https://github.com/${NIT_REPO}/releases/download/${tag}/${asset}"
  sums_url="https://github.com/${NIT_REPO}/releases/download/${tag}/SHA256SUMS"

  info "Repository:    ${NIT_REPO}"
  info "Tag:           ${tag}"
  info "Target:        ${target}"
  info "Install dir:   ${NIT_INSTALL_DIR}"

  tmp_dir="$(mktemp -d)"
  trap 'rm -rf "$tmp_dir"' EXIT

  info "Downloading ${asset}..."
  fetch "$asset_url" "$tmp_dir/$asset" || err "Failed to download $asset_url"

  info "Verifying checksum..."
  if fetch "$sums_url" "$tmp_dir/SHA256SUMS" 2>/dev/null; then
    local expected
    expected="$(grep -E " +${asset}$" "$tmp_dir/SHA256SUMS" | awk '{print $1}' | head -1)"
    if [ -n "$expected" ]; then
      verify_sha256 "$tmp_dir/$asset" "$expected"
    else
      warn "Asset ${asset} not listed in SHA256SUMS; skipping verification."
    fi
  else
    warn "SHA256SUMS not published yet for ${tag}; skipping verification."
  fi

  info "Extracting..."
  tar -xzf "$tmp_dir/$asset" -C "$tmp_dir"
  local extracted="$tmp_dir/nit-${tag}-${target}"
  [ -d "$extracted" ] || err "Unexpected archive layout: $extracted not found"

  mkdir -p "$NIT_INSTALL_DIR"
  install -m 0755 "$extracted/nit" "$NIT_INSTALL_DIR/nit"
  install -m 0755 "$extracted/nit-mcp-server" "$NIT_INSTALL_DIR/nit-mcp-server"

  info "Installed:"
  info "  ${NIT_INSTALL_DIR}/nit"
  info "  ${NIT_INSTALL_DIR}/nit-mcp-server"

  # PATH hint
  case ":$PATH:" in
    *":$NIT_INSTALL_DIR:"*) ;;
    *)
      if [ "${NIT_NO_MODIFY_PATH:-0}" != "1" ]; then
        printf '\n'
        printf '\033[1;33mAdd this to your shell config to make `nit` reachable:\033[0m\n'
        printf '\n'
        printf '  export PATH="%s:$PATH"\n' "$NIT_INSTALL_DIR"
        printf '\n'
      fi
      ;;
  esac

  printf '\033[1;32mDone.\033[0m Run `nit --version` to verify.\n'
}

main "$@"
