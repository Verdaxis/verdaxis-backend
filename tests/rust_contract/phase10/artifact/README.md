# Artifact contract fixtures

All keys in this corpus are **throwaway test material**, generated inside a
private temporary directory during each run. No installed signing key is used.
The corpus builds the valid archive with the unchanged CI packager and creates
its signed record with the unchanged workstation attestor. A local `gh` fixture
stands in for the remote attestation service; this proves format compatibility,
not GitHub provenance or live-release acceptance.

`contract.py` generates valid and independently signed invalid archives, records,
and signatures, then invokes the shell host verifier as a subprocess. The
candidate is an executable marker script and must never run. Python is only a
test-fixture dependency; the host verifier uses Bash, GNU coreutils/tar, jq and
OpenSSL. Run on Linux with `bash deploy/tests/artifact-contract.sh`.
