# R6 privacy inspection

R6 requires a direct check that questions, answers, prompts, and citation
identifiers do not leak into the profile outside the mounted encrypted vault.
Pinky provides a bounded scanner for this acceptance check:

```bash
cd apps/desktop
npm run inspect:plaintext -- \
  --root "$PINKY_PROFILE" \
  --vault "$PINKY_VAULT_MOUNT" \
  --marker "IRIS417" \
  --marker "pinky://source/" \
  --marker "BEGIN-pinky-evidence-"
```

`--root` is the isolated profile or application-data directory used for the
acceptance run. `--vault` is the verified readable gocryptfs mount and is
excluded from the scan because retained data is expected there. The scanner
follows no symlinks, skips non-regular files and files larger than 16 MiB, and
returns exit status 1 with a JSON finding for every marker found outside the
vault. Run it after asking the fixed tutorial question and after restarting
Pinky so both live and restored paths are covered.

The scanner does not replace the target-host process-boundary checks. The R6
operator must also verify that the configured model endpoint is the explicit
loopback/SSH-tunnel endpoint, no cloud or LAN address was contacted, and no
child process or container remains after cancellation.
