#!/usr/bin/env bash
set -euo pipefail

fixture="${PINKY_PDF_ACCEPTANCE_FIXTURE:-}"
if [[ -z "$fixture" ]]; then
  for candidate in \
    /usr/share/doc/shared-mime-info/shared-mime-info-spec.pdf \
    /usr/share/cups/data/default-testpage.pdf
  do
    if [[ -r "$candidate" ]]; then
      fixture="$candidate"
      break
    fi
  done
fi

if [[ -z "$fixture" || ! -f "$fixture" ]]; then
  echo "No readable PDF fixture found. Set PINKY_PDF_ACCEPTANCE_FIXTURE to an embedded-text PDF." >&2
  exit 1
fi

for tool in /usr/bin/pdfunite /usr/bin/pdftotext; do
  if [[ ! -x "$tool" ]]; then
    echo "Required Poppler tool is unavailable: $tool" >&2
    exit 1
  fi
done

echo "Using private-host PDF fixture: $fixture"
PINKY_PDF_ACCEPTANCE_FIXTURE="$fixture" \
  cargo test -p pinky-core --test live_pdf \
  interrupting_real_poppler_cleans_vault_staging \
  -- --ignored --exact --nocapture
