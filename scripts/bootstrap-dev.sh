#!/usr/bin/env bash
set -Eeuo pipefail

usage() {
  cat <<'EOF'
Usage: scripts/bootstrap-dev.sh [--skip-e2e]

Install the Debian/Ubuntu development prerequisites, Rust components, the
locked JavaScript dependencies, the Poppler PDF text runtime, the Tesseract
OCR runtime, the ImageMagick OCR preprocessing tool, and (unless skipped)
tauri-driver.

This script does not install Ollama or model files. Qdrant is also not
installed automatically; use scripts/install-qdrant-dev.sh for the pinned
development-only Qdrant sidecar, or use the future signed artifact onboarding
for release acceptance.
EOF
}

die() {
  printf 'bootstrap-dev: %s\n' "$1" >&2
  exit 1
}

warn() {
  printf 'bootstrap-dev: warning: %s\n' "$1" >&2
}

node_version_is_supported() {
  local version="${1#v}"
  local major minor patch
  IFS=. read -r major minor patch <<< "$version"
  case "$major:$minor" in
    20:*) [ "$minor" -ge 19 ] ;;
    22:*) [ "$minor" -ge 12 ] ;;
    2[3-9]:*|[3-9][0-9]:*) return 0 ;;
    *) return 1 ;;
  esac
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
  imagemagick
  libayatana-appindicator3-dev
  libdbus-1-dev
  libgtk-3-dev
  libsecret-tools
  libssl-dev
  libwebkit2gtk-4.1-dev
  libxdo-dev
  librsvg2-dev
  pkg-config
  patchelf
  poppler-utils
  podman
  tesseract-ocr
  tesseract-ocr-eng
  vulkan-tools
  webkit2gtk-driver
  wget
  xvfb
)

if [ "$node_major" -lt 20 ] || ! command -v npm >/dev/null 2>&1; then
  # Distribution npm must only be considered when the selected Node runtime
  # does not already provide it.  NodeSource's Node 20 package includes npm;
  # requesting Ubuntu's npm alongside it can create an unrelated dependency
  # conflict on otherwise usable hosts.
  apt_packages+=(nodejs npm)
fi

printf 'Installing Debian/Ubuntu packages...\n'
"${apt_command[@]}" update

# libwebkit2gtk requires Soup development headers that exactly match the
# installed Soup runtime. Some multimedia PPAs upgrade the runtime without
# changing apt's preferred development-header candidate. Prefer the installed
# version only when both matching packages are actually available, avoiding a
# needless downgrade or a resolver failure on those hosts.
installed_soup_version="$(dpkg-query -W -f='${Version}' libsoup-3.0-0 2>/dev/null || true)"
if [ -n "$installed_soup_version" ] \
  && apt-cache show "libsoup-3.0-dev=${installed_soup_version}" >/dev/null 2>&1 \
  && apt-cache show "gir1.2-soup-3.0=${installed_soup_version}" >/dev/null 2>&1; then
  apt_packages+=(
    "libsoup-3.0-dev=${installed_soup_version}"
    "gir1.2-soup-3.0=${installed_soup_version}"
  )
  printf 'Using Soup development headers matching installed runtime %s.\n' "$installed_soup_version"
fi
"${apt_command[@]}" install -y --no-install-recommends "${apt_packages[@]}"

node_version="$(node --version 2>/dev/null || printf 'none')"
node_version_is_supported "$node_version" || die "Node.js 20.19.0+ or 22.12.0+ is required; active runtime is ${node_version} at $(command -v node 2>/dev/null || printf 'none'). If NVM shadows a newer system Node, run this script with PATH=/usr/bin:\$PATH or install/select a supported NVM version, then rerun."
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
printf '  Tesseract:   %s\n' "$(tesseract --version 2>&1 | head -n 1)"
printf '  ImageMagick: %s\n' "$(magick --version 2>&1 | head -n 1)"
printf '  pdftotext:   %s\n' "$(command -v pdftotext || printf 'missing')"
printf '  pdftoppm:    %s\n' "$(command -v pdftoppm || printf 'missing')"
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

Ollama and its model files are intentionally not downloaded by this script.
For development-only Qdrant setup, run:
  scripts/install-qdrant-dev.sh
See README.md for the local-model and hybrid-retrieval setup.
EOF
