#!/usr/bin/env bash
set -Eeuo pipefail

# Development-only Qdrant bootstrap. This is intentionally not Pinky's signed
# artifact onboarding path: the release URL and archive digest are pinned here
# so a developer can exercise the existing supervised sidecar before release
# manifests and trusted-key distribution are available.

readonly QDRANT_VERSION='1.19.1'
readonly QDRANT_ARCHIVE="qdrant-x86_64-unknown-linux-musl.tar.gz"
readonly QDRANT_ARCHIVE_BYTES='32315868'
readonly QDRANT_ARCHIVE_SHA256='70a40529e2ebe0a2787d574d3a2e28437cfe94f26f24fa419f6ac57b4ae817c9'
readonly QDRANT_URL="https://github.com/qdrant/qdrant/releases/download/v${QDRANT_VERSION}/${QDRANT_ARCHIVE}"

usage() {
  cat <<'EOF'
Usage: scripts/install-qdrant-dev.sh

Download and install the pinned Qdrant x86-64 musl binary for development.
The archive is checked against its pinned size and SHA-256 digest, extracted
in a temporary directory, and installed atomically below:

  ${XDG_DATA_HOME:-$HOME/.local/share}/pinky/dev-tools/qdrant/v1.19.1/qdrant

Override the destination with PINKY_QDRANT_DEV_DIR. This installer is a
development convenience and does not satisfy Pinky's signed release-artifact
acceptance gate.
EOF
}

die() {
  printf 'install-qdrant-dev: %s\n' "$1" >&2
  exit 1
}

if [ "$#" -gt 0 ]; then
  case "$1" in
    --help|-h)
      usage
      exit 0
      ;;
    *)
      usage >&2
      die "unknown argument: $1"
      ;;
  esac
fi

command -v curl >/dev/null 2>&1 || die 'curl is required'
command -v sha256sum >/dev/null 2>&1 || die 'sha256sum is required'
command -v tar >/dev/null 2>&1 || die 'tar is required'

data_root="${XDG_DATA_HOME:-${HOME:-}}"
[ -n "$data_root" ] || die 'HOME or XDG_DATA_HOME must be set'
install_root="${PINKY_QDRANT_DEV_DIR:-${data_root}/pinky/dev-tools/qdrant}"
case "$install_root" in
  /*) ;;
  *) die "PINKY_QDRANT_DEV_DIR must be an absolute path: $install_root" ;;
esac

mkdir -p "$install_root"
target_dir="$install_root/v${QDRANT_VERSION}"
target_binary="$target_dir/qdrant"
if [ -x "$target_binary" ]; then
  printf 'Pinned Qdrant %s is already installed:\n  %s\n' "$QDRANT_VERSION" "$target_binary"
  printf 'Set this before launching Pinky:\n  export PINKY_QDRANT_EXECUTABLE=%q\n' "$target_binary"
  exit 0
fi
if [ -e "$target_dir" ]; then
  die "refusing to overwrite an existing incomplete installation: $target_dir"
fi

archive_path=""
temporary_dir=""
cleanup() {
  if [ -n "${temporary_dir:-}" ] && [ -d "$temporary_dir" ]; then
    rm -rf -- "$temporary_dir"
  fi
  if [ -n "${archive_path:-}" ] && [ -f "$archive_path" ]; then
    rm -f -- "$archive_path"
  fi
}
trap cleanup EXIT

temporary_dir="$(mktemp -d "$install_root/.qdrant-${QDRANT_VERSION}.XXXXXX")"
archive_path="$temporary_dir/$QDRANT_ARCHIVE"
printf 'Downloading pinned Qdrant %s...\n' "$QDRANT_VERSION"
curl --fail --location --proto '=https' --tlsv1.2 --retry 3 \
  --silent --show-error "$QDRANT_URL" --output "$archive_path"

actual_bytes="$(stat -c '%s' "$archive_path" 2>/dev/null || stat -f '%z' "$archive_path")"
[ "$actual_bytes" = "$QDRANT_ARCHIVE_BYTES" ] || \
  die "archive size was ${actual_bytes} bytes, expected ${QDRANT_ARCHIVE_BYTES}"
actual_sha256="$(sha256sum "$archive_path" | awk '{print $1}')"
[ "$actual_sha256" = "$QDRANT_ARCHIVE_SHA256" ] || \
  die "archive SHA-256 was ${actual_sha256}, expected ${QDRANT_ARCHIVE_SHA256}"

extract_dir="$temporary_dir/extracted"
mkdir "$extract_dir"
tar --extract --gzip --file "$archive_path" --directory "$extract_dir" \
  --no-same-owner --no-same-permissions
extracted_binary="$extract_dir/qdrant"
[ -f "$extracted_binary" ] || die 'the pinned archive did not contain qdrant'
chmod 0755 "$extracted_binary"

staged_dir="$temporary_dir/installation"
mkdir "$staged_dir"
mv -- "$extracted_binary" "$staged_dir/qdrant"
printf '{\n  "artifact": "qdrant",\n  "version": "%s",\n  "url": "%s",\n  "archive_sha256": "%s",\n  "verification": "development_pinned_archive"\n}\n' \
  "$QDRANT_VERSION" "$QDRANT_URL" "$QDRANT_ARCHIVE_SHA256" > "$staged_dir/install.json"
mv -- "$staged_dir" "$target_dir"
cleanup
temporary_dir=""
archive_path=""
printf '\nPinned development Qdrant %s installed:\n  %s\n' "$QDRANT_VERSION" "$target_binary"
printf 'Set this before launching Pinky:\n  export PINKY_QDRANT_EXECUTABLE=%q\n' "$target_binary"
printf 'This installation is not a signed-release acceptance result.\n'
