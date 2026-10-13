#!/usr/bin/env bash
# Pull, build, and restart an existing systemd-managed Zakura node on this host.
set -euo pipefail

usage() {
    cat <<'EOF'
Usage: ./scripts/update-node.sh

Run on the Linux node host, as the user who owns the source checkout.
The script pulls the current branch's upstream with --ff-only, builds zakurad,
installs it at the service's ExecStart path, and restarts the existing service.
It preserves the service configuration and chain data.

Optional environment variables:
  REPO_DIR        Source checkout (default: this script's parent directory)
  SERVICE_NAME    Existing systemd service (default: zakurad.service)
  BUILD_DIR       Cargo output directory (default: REPO_DIR/target/node-update)
  BUILD_FEATURES  Extra Cargo features, e.g. indexer,commit-metrics
  CHECK_SECONDS   Seconds to check that the new process stays active (default: 10)

Examples:
  ./scripts/update-node.sh
  SERVICE_NAME=zakurad-testnet.service ./scripts/update-node.sh
  BUILD_FEATURES=indexer ./scripts/update-node.sh

Requires the usual Zakura build dependencies, git, systemd, and flock.
Uses sudo only for installing the binary and restarting the service.
The previous binary is saved as <binary-path>.bak.
EOF
}

die() {
    printf 'Error: %s\n' "$*" >&2
    exit 1
}

run_root() {
    if (( EUID == 0 )); then
        "$@"
    else
        sudo "$@"
    fi
}

if [[ $# -gt 0 ]]; then
    case "$1" in
        -h|--help) usage; exit 0 ;;
        *) usage >&2; exit 1 ;;
    esac
fi

REPO_DIR="${REPO_DIR:-$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd -P)}"
SERVICE_NAME="${SERVICE_NAME:-zakurad.service}"
CHECK_SECONDS="${CHECK_SECONDS:-10}"
[[ "$CHECK_SECONDS" =~ ^[1-9][0-9]{0,2}$ ]] && (( CHECK_SECONDS <= 300 )) \
    || die 'CHECK_SECONDS must be an integer between 1 and 300.'

for command in git cargo rustc systemctl flock readlink timeout; do
    command -v "$command" >/dev/null || die "Required command is missing: $command"
done

cd -- "$REPO_DIR"
REPO_DIR="$(pwd -P)"
[[ "$(git rev-parse --show-toplevel)" == "$REPO_DIR" ]] \
    || die 'REPO_DIR must point to the root of the Zakura checkout.'
[[ -f crates/zakurad/Cargo.toml ]] || die 'This checkout does not contain Zakura.'

# Serialize updates from this checkout without creating untracked source files.
exec 9>"$(git rev-parse --git-path zakura-node-update.lock)"
flock -n 9 || die 'Another node update is already running from this checkout.'

git diff --quiet && git diff --cached --quiet \
    || die 'The checkout has tracked changes. Commit or stash them before updating.'
branch="$(git symbolic-ref --quiet --short HEAD)" \
    || die 'Check out a branch before updating; detached HEAD is not supported.'
upstream="$(git rev-parse --abbrev-ref --symbolic-full-name '@{upstream}')" \
    || die "Branch $branch needs a Git upstream before it can be pulled."

[[ "$(systemctl show --property=LoadState --value -- "$SERVICE_NAME")" == loaded ]] \
    || die "Systemd service is not installed: $SERVICE_NAME"
# Support the direct zakurad ExecStart used by the repository's service units.
binary_path="$(systemctl show --property=ExecStart --value -- "$SERVICE_NAME" \
    | sed -n 's/^{ path=\([^;]*\) ; argv\[\]=.*/\1/p')"
[[ "$binary_path" == /* && "$(basename -- "$binary_path")" == zakurad ]] \
    || die 'The service must run an absolute zakurad path directly in ExecStart.'
binary_path="$(readlink -f -- "$binary_path")"
[[ -f "$binary_path" && -x "$binary_path" ]] \
    || die "The service binary is missing or not executable: $binary_path"

if (( EUID != 0 )); then
    command -v sudo >/dev/null || die 'sudo is required to install and restart the node.'
    sudo -v
fi

printf 'Pulling %s from %s...\n' "$branch" "$upstream"
timeout 300 git pull --ff-only
printf 'Building commit %s...\n' "$(git rev-parse --short HEAD)"

# Keep Cargo away from the live executable even if ExecStart uses target/release.
BUILD_DIR="${BUILD_DIR:-$REPO_DIR/target/node-update}"
mkdir -p -- "$BUILD_DIR"
BUILD_DIR="$(cd -- "$BUILD_DIR" && pwd -P)"
[[ "$binary_path" != "$BUILD_DIR/"* ]] \
    || die 'BUILD_DIR must not contain the binary used by the running service.'
host_target="$(rustc -vV | sed -n 's/^host: //p')"
[[ -n "$host_target" ]] || die 'Could not determine the native Rust target.'
build_args=(build --release --locked -p zakura --bin zakurad
    --target "$host_target" --target-dir "$BUILD_DIR")
if [[ -n "${BUILD_FEATURES:-}" ]]; then
    build_args+=(--features "$BUILD_FEATURES")
fi
cargo "${build_args[@]}"
built_binary="$BUILD_DIR/$host_target/release/zakurad"
[[ -x "$built_binary" ]] || die "Build did not produce an executable: $built_binary"
"$built_binary" --version

if (( EUID != 0 )); then
    sudo -v
fi

# Rename a staged file on the same filesystem instead of overwriting a live binary.
staged_binary=""
cleanup() {
    if [[ -n "$staged_binary" ]]; then
        run_root rm -f -- "$staged_binary"
    fi
}
trap cleanup EXIT
staged_binary="$(run_root mktemp "${binary_path}.new.XXXXXX")"
run_root install -m 755 -- "$built_binary" "$staged_binary"
run_root cp -p -- "$binary_path" "${binary_path}.bak"
run_root mv -f -- "$staged_binary" "$binary_path"
staged_binary=""

printf 'Restarting %s...\n' "$SERVICE_NAME"
if ! run_root timeout 180 systemctl restart -- "$SERVICE_NAME"; then
    die "Restart failed. Inspect: sudo journalctl -u $SERVICE_NAME -n 100 --no-pager"
fi
new_pid="$(systemctl show --property=MainPID --value -- "$SERVICE_NAME")"
[[ "$new_pid" =~ ^[1-9][0-9]*$ ]] || die 'The restarted service has no running process.'
for (( second = 0; second < CHECK_SECONDS; second++ )); do
    sleep 1
    if ! systemctl is-active --quiet -- "$SERVICE_NAME" \
        || [[ "$(systemctl show --property=MainPID --value -- "$SERVICE_NAME")" != "$new_pid" ]]; then
        die "The service exited or restarted. Inspect: sudo journalctl -u $SERVICE_NAME -n 100 --no-pager"
    fi
done

systemctl --no-pager --full status -- "$SERVICE_NAME"
printf 'Update complete. Previous binary: %s.bak\n' "$binary_path"
