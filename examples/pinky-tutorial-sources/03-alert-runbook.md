# Alder alert runbook

## IRIS417 — sustained unexpected flow

IRIS417 indicates that a monitored zone exceeded the sustained-flow threshold.
It is an investigation prompt, not proof that a pipe has failed.

1. Check whether the zone has an active irrigation schedule.
2. Confirm that the maintenance-suppression flag is false.
3. Compare the latest flow reading with the 12 litres-per-minute threshold.
4. Inspect the valve-state signal and the previous four minutes of readings.
5. If unexpected flow continues, isolate the zone manually and notify Jonah
   Bell.

Record the zone, first observation time, peak flow, valve state, and operator
action. Escalate immediately if two or more zones report IRIS417 together.

Do not close an alert solely because the sensor returned to normal. Attach the
retained measurements and use the resolution label `verified_transient` or
`confirmed_leak`.
