#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-dev.sh [--skip-e2e]

Install the Debian/Ubuntu development prerequisites, Rust components, the
locked JavaScript dependencies, and (unless skipped) tauri-driver.

This script does not install Ollama, model files, or Qdrant. Those are large,
optional runtimes and must be installed/configured separately.
EOF
}

die() {
  printf 'bootstrap-dev: %s\n' "$1" >&2
  exit 1
}

warn() {
  printf 'bootstrap-dev: warning: %s\n' "$1" >&2
}

skip_e2e=0
for argument in "$@"; do
  case "$argument" in
    --skip-e2e)
      skip_e2e=1
      ;;
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown argument: $argument"
      ;;
  esac
done

script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
repo_dir="$(cd -- "${script_dir}/.." && pwd)"
user_home="${HOME:-}"
[ -n "$user_home" ] || die 'HOME is not set'

if [ ! -f /etc/os-release ]; then
  die 'cannot identify the Linux distribution; this bootstrap supports Debian/Ubuntu only'
fi

# shellcheck disable=SC1091
source /etc/os-release
distribution_ids="${ID:-} ${ID_LIKE:-}"
if ! printf '%s\n' "$distribution_ids" | grep -Eiq '(^|[[:space:]])(debian|ubuntu)([[:space:]]|$)'; then
  die "unsupported distribution: ${PRETTY_NAME:-unknown}; install the documented prerequisites manually"
fi

if [ "$(id -u)" -eq 0 ]; then
  apt_command=(apt-get)
else
  command -v sudo >/dev/null 2>&1 || die 'sudo is required to install system packages'
  apt_command=(sudo apt-get)
fi

node_major=0
if command -v node >/dev/null 2>&1; then
  node_major="$(node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null || printf '0')"
fi

apt_packages=(
  build-essential
  ca-certificates
  curl
  dbus-x11
  file
  git
  gnome-keyring
  gocryptfs
  libayatana-appindicator3-dev
  libdbus-1-dev
  libgtk-3-dev
  libsecret-tools
  libssl-dev
  libwebkit2gtk-4.1-dev
  libxdo-dev
  librsvg2-dev
  npm
  pkg-config
  patchelf
  podman
  vulkan-tools
  webkit2gtk-driver
  wget
  xvfb
)

if [ "$node_major" -lt 20 ]; then
  apt_packages+=(nodejs)
fi

printf 'Installing Debian/Ubuntu packages...\n'
"${apt_command[@]}" update
"${apt_command[@]}" install -y --no-install-recommends "${apt_packages[@]}"

node_major="$(node -p 'Number(process.versions.node.split(".")[0])' 2>/dev/null || printf '0')"
(( node_major >= 20 )) || die "Node.js 20 or newer is required; found $(node --version 2>/dev/null || printf 'none'). Install a newer LTS release and rerun this script."
command -v npm >/dev/null 2>&1 || die 'npm is unavailable after package installation'

# Load an existing rustup installation before deciding whether Rust is absent.
if [ -f "$user_home/.cargo/env" ]; then
  # shellcheck disable=SC1090
  source "$user_home/.cargo/env"
fi

if ! command -v cargo >/dev/null 2>&1; then
  command -v curl >/dev/null 2>&1 || die 'curl is required to install Rust'
  rustup_installer="$(mktemp)"
  trap 'rm -f -- "$rustup_installer"' EXIT
  printf 'Downloading the official rustup installer...\n'
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error \
    https://sh.rustup.rs -o "$rustup_installer"
  sh "$rustup_installer" -y --default-toolchain stable --profile minimal
  rm -f -- "$rustup_installer"
  trap - EXIT
  [ -f "$user_home/.cargo/env" ] || die 'rustup installed without its Cargo environment file'
  # shellcheck disable=SC1090
  source "$user_home/.cargo/env"
fi

command -v cargo >/dev/null 2>&1 || die 'Cargo is unavailable; open a new shell or source ~/.cargo/env and rerun this script'
if command -v rustup >/dev/null 2>&1; then
  rustup default stable
  rustup component add rustfmt clippy
else
  warn 'rustup was not found; rustfmt and clippy were not installed'
fi

printf 'Fetching Rust dependencies...\n'
(cd "$repo_dir" && cargo fetch --locked)

printf 'Installing locked desktop JavaScript dependencies...\n'
(cd "$repo_dir/apps/desktop" && npm ci)

if [ "$skip_e2e" -eq 0 ]; then
  if command -v tauri-driver >/dev/null 2>&1; then
    printf 'tauri-driver is already installed.\n'
  else
    printf 'Installing tauri-driver for native end-to-end tests...\n'
    cargo install tauri-driver --locked
  fi
fi

printf '\nDevelopment prerequisites are ready.\n'
printf '  Rust:        %s\n' "$(rustc --version)"
printf '  Cargo:       %s\n' "$(cargo --version)"
printf '  Node.js:     %s\n' "$(node --version)"
printf '  npm:         %s\n' "$(npm --version)"
printf '  gocryptfs:   %s\n' "$(gocryptfs --version 2>&1 | head -n 1)"
printf '  secret-tool: %s\n' "$(command -v secret-tool)"
printf '  Podman:      %s\n' "$(podman --version)"
vulkan_status='not available'
if command -v vulkaninfo >/dev/null 2>&1 && vulkaninfo --summary >/dev/null 2>&1; then
  vulkan_status='available'
fi
printf '  Vulkan:      %s\n' "$vulkan_status"

if [ "$skip_e2e" -eq 1 ]; then
  printf '\nE2E setup was skipped. Run without --skip-e2e to install tauri-driver.\n'
fi

cat <<'EOF'

Next steps:
  cd apps/desktop
  npm test
  npm run build
  npm run tauri dev

Ollama, its model files, and Qdrant are intentionally not downloaded by this
script. See README.md for the optional local-model and hybrid-retrieval setup.
EOF
