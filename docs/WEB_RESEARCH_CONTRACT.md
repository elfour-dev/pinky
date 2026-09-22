# Public-web research boundary

This document describes the first R11 implementation slice. It is a safety
boundary, not a claim that the complete web-research workflow is finished.

## Current implementation

`pinky-core::research` provides:

- a versioned research budget bounded to 40 documents, 250 MiB, 20 minutes,
  depth two, ten pages per origin, and four concurrent requests;
- normalization and a hard limit of five search queries;
- user-configurable domain blocks;
- rejection of credentials, fragments, unsupported schemes, `.local`, LAN,
  loopback, link-local, private, documentation, multicast, and other
  non-public IP targets;
- DNS resolution that fails closed if any returned address is non-public;
- an address-pinned HTTP client with redirects disabled;
- cancellation-aware bounded response reads.
- a SearXNG client restricted to an explicitly ported `127.0.0.1` endpoint,
  with at most five queries, 50 deduplicated results, bounded JSON, and
  unsafe result URLs discarded before they can become fetch targets.

The model or a web page cannot change these limits. URL validation must happen
before a request is scheduled, and callers must use `ResolvedPublicUrl` rather
than passing a parsed hostname directly to the fetch helper.

## Still required for R11 completion

- supervised local SearXNG and Chromium sidecars (the client contract exists,
  but sidecar lifecycle is not wired into the desktop task graph);
- robots, `Retry-After`, and `Cache-Control` handling;
- result-page/snippet archival and source-version ingestion;
- origin pacing, document/depth accounting, and budget transitions to
  `waiting_for_user`;
- prompt-injection fixtures and end-to-end cancellation/cleanup tests;
- rerun retrieval and mark answers as “researched now”.
