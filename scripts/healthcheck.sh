#!/usr/bin/env bash
# healthcheck.sh — verify repo integrity for the nit workspace.
#
# Checks formatting, compilation, license compliance, and optionally
# runs the full clippy + test suite in "deep" mode.
set -u -o pipefail

# --- Configuration & Constants ---

readonly SCRIPT_NAME="scripts/healthcheck.sh"

# Status labels used in check output.
readonly PASS_LABEL="[green] pass"
readonly FAIL_LABEL="[red] fail"
readonly SKIP_LABEL="[yellow] skipped"

# Accumulates the overall exit status across all checks.
fail=0

# Whether to include expensive analysis (clippy + tests).
deep_mode=0

# --- Output Formatting ---

# Print a section divider with the command being executed.
# Arg 1: the command string to display.
print_header() {
  printf '\n==> %s\n' "$1"
}

# Record a check failure with its exit code.
# Arg 1: numeric exit code from the failed command.
record_failure() {
  local exit_code="${1:?exit code required}"
  echo "${FAIL_LABEL} (exit ${exit_code})"
  fail=1
}

# --- Usage & Argument Parsing ---

# Display help text describing available flags and exit codes.
usage() {
  cat <<HELP
Usage: ${SCRIPT_NAME} [OPTIONS]

Verify nit workspace integrity.

Options:
  --deep    Include clippy and full test suite
  -h        Show this help message

Exit codes:
  0   All checks passed
  1   One or more checks failed
  2   Invalid arguments
HELP
}

# Parse command-line flags into global configuration variables.
# Returns 0 on success, 1 on error, 2 to signal --help was requested.
parse_arguments() {
  while [[ $# -gt 0 ]]; do
    case "$1" in
      --deep)
        deep_mode=1
        shift
        ;;
      -h|--help)
        usage
        return 2
        ;;
      *)
        printf 'Unknown argument: %s\n' "$1" >&2
        usage >&2
        return 1
        ;;
    esac
  done
}

# --- Check Execution ---

# Run a command for informational display; failures are non-fatal.
# Arg 1: command string to evaluate.
execute_info_probe() {
  local cmd="$1"
  print_header "${cmd}"
  eval "${cmd}" || true
}

# Run a command as a pass/fail gate; failures set the global flag.
# Arg 1: command string to evaluate.
execute_gate_check() {
  local cmd="$1"
  print_header "${cmd}"

  if eval "${cmd}"; then
    echo "${PASS_LABEL}"
  else
    record_failure $?
  fi
}

# --- Cargo-Deny Verification ---

# Run cargo-deny with special handling for advisory DB lock errors.
# This check is skipped gracefully when cargo-deny is not installed
# or when the advisory database path is read-only.
verify_cargo_deny() {
  local label="cargo deny check"
  print_header "${label}"

  if ! command -v cargo-deny >/dev/null 2>&1; then
    echo "${SKIP_LABEL}: cargo-deny not installed"
    return 0
  fi

  local captured_output exit_status
  captured_output="$(cargo deny check 2>&1)"
  exit_status=$?

  printf '%s\n' "${captured_output}"

  if (( exit_status == 0 )); then
    echo "${PASS_LABEL}"
    return 0
  fi

  # Advisory DB lock contention in CI or read-only filesystems.
  local lock_error_pattern="failed to acquire advisory database lock"
  if printf '%s' "${captured_output}" | rg -q "${lock_error_pattern}"; then
    echo "${SKIP_LABEL}: advisory DB lock path is read-only in this environment"
    return 0
  fi

  record_failure "${exit_status}"
}

# --- Repository Probes ---

# Collect working-tree status and gitignore configuration.
probe_working_tree() {
  execute_info_probe "git status --short"

  # Verify that vendored paths are properly ignored.
  local ignore_targets="Cargo.lock vendor vendor/time"
  execute_info_probe "git check-ignore -v ${ignore_targets}"
}

# Show Rust version pins and patch overrides from workspace manifests.
probe_toolchain_pins() {
  local workspace_pattern='rust-version|patch\.crates-io'
  local vendor_files="vendor/time/Cargo.toml rust-toolchain.toml"

  execute_info_probe "rg -n '${workspace_pattern}' Cargo.toml && rg -n 'rust-version' ${vendor_files}"
}

# Display excerpts of CI and deny configuration for visual review.
probe_ci_configuration() {
  local ci_path=".github/workflows/ci.yml"
  local deny_path="deny.toml"

  execute_info_probe "sed -n '1,220p' ${ci_path} && sed -n '1,160p' ${deny_path}"
}

# Detect secret-scanning and dependency-update automation in the repo.
probe_security_tooling() {
  local tool_names='dependabot|renovate|gitleaks|trufflehog|secret'
  execute_info_probe "rg -n -i --hidden -g '.github/**' '${tool_names}'"
}

# --- Deep Analysis ---

# Run expensive static analysis: clippy with deny-warnings and the
# full workspace test suite with all features enabled.
run_deep_analysis() {
  execute_gate_check "cargo clippy --workspace --all-targets --all-features -- -D warnings"
  execute_gate_check "cargo test --workspace --all-targets --all-features"
}

# --- Result Reporting ---

# Summarize the healthcheck outcome and return the appropriate status.
summarize_result() {
  echo

  if (( fail )); then
    echo "Healthcheck completed with failures."
    return 1
  fi

  echo "Healthcheck completed successfully."
}

# --- Entry Point ---

# Orchestrate all healthcheck phases in sequence.
main() {
  parse_arguments "$@"
  local parse_status=$?

  # Help was requested — exit cleanly.
  if (( parse_status == 2 )); then
    exit 0
  fi

  # Invalid arguments — propagate the error code.
  if (( parse_status != 0 )); then
    exit "${parse_status}"
  fi

  echo "nit healthcheck"
  echo "cwd: $(pwd)"

  # Phase 1 — informational probes (non-fatal)
  probe_working_tree
  probe_toolchain_pins
  probe_ci_configuration
  probe_security_tooling

  # Phase 2 — compilation and formatting gates
  execute_gate_check "cargo check --workspace --all-targets"
  execute_gate_check "cargo fmt --all -- --check"

  # Phase 3 — license and advisory verification
  verify_cargo_deny

  # Phase 4 — deep analysis (clippy + tests, when requested)
  if (( deep_mode )); then
    run_deep_analysis
  fi

  summarize_result
}

main "$@"
