#!/usr/bin/env bash
set -u -o pipefail

usage() {
  cat <<'EOF'
Usage: scripts/healthcheck.sh [--deep]

Runs a quick repo healthcheck.
  --deep    Also run clippy and full-feature test suite.
EOF
}

deep=0
if [[ "${1:-}" == "--deep" ]]; then
  deep=1
  shift
fi

if [[ "${1:-}" == "-h" || "${1:-}" == "--help" ]]; then
  usage
  exit 0
fi

if [[ $# -ne 0 ]]; then
  echo "Unknown argument: $1" >&2
  usage >&2
  exit 2
fi

fail=0

run_info() {
  local cmd="$1"
  echo
  echo "==> ${cmd}"
  eval "${cmd}" || true
}

run_check() {
  local cmd="$1"
  echo
  echo "==> ${cmd}"
  if eval "${cmd}"; then
    echo "[green] pass"
  else
    local rc=$?
    echo "[red] fail (exit ${rc})"
    fail=1
  fi
}

echo "nit healthcheck"
echo "cwd: $(pwd)"

run_info "git status --short"
run_info "git check-ignore -v Cargo.lock vendor vendor/time"
run_info "rg -n 'rust-version|patch\\.crates-io' Cargo.toml && rg -n 'rust-version' vendor/time/Cargo.toml rust-toolchain.toml"
run_info "sed -n '1,220p' .github/workflows/ci.yml && sed -n '1,160p' deny.toml"
run_info "rg -n -i --hidden -g '.github/**' 'dependabot|renovate|gitleaks|trufflehog|secret'"

run_check "cargo check --workspace --all-targets"
run_check "cargo fmt --all -- --check"

if command -v cargo-deny >/dev/null 2>&1; then
  echo
  echo "==> cargo deny check"
  deny_output="$(cargo deny check 2>&1)"
  deny_rc=$?
  if (( deny_rc == 0 )); then
    printf '%s\n' "${deny_output}"
    echo "[green] pass"
  elif printf '%s' "${deny_output}" | rg -q "failed to acquire advisory database lock"; then
    printf '%s\n' "${deny_output}"
    echo "[yellow] skipped: advisory DB lock path is read-only in this environment"
  else
    printf '%s\n' "${deny_output}"
    echo "[red] fail (exit ${deny_rc})"
    fail=1
  fi
else
  echo
  echo "==> cargo deny check"
  echo "[yellow] skipped: cargo-deny not installed"
fi

if (( deep )); then
  run_check "cargo clippy --workspace --all-targets --all-features -- -D warnings"
  run_check "cargo test --workspace --all-targets --all-features"
fi

echo
if (( fail )); then
  echo "Healthcheck completed with failures."
  exit 1
fi
echo "Healthcheck completed successfully."
