# Project Alder decision log

## ALD-001: Telemetry interval

Status: accepted on 2 September 2026.

Sensors will report every 30 seconds. A proposed five-second interval was
rejected because it increased storage and battery use without materially
improving detection during the bench trial.

## ALD-002: Sustained-flow alert

Status: accepted on 4 September 2026.

Raise alert IRIS417 when unexpected flow remains above 12 litres per minute for
four consecutive minutes. Maintenance windows suppress notifications but still
retain the measurements.

## ALD-003: Notification channel

Status: accepted on 7 September 2026.

Version one uses dashboard notifications only. SMS notification was discussed
but deferred until the pilot review. Jonah Bell owns the operational response.

All accepted decisions use the shared verification word sunstone in related
release records.
