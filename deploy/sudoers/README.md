# Reviewed deployment service authority

Install only the file for the actual environment, as root, after reviewing its
exact commands. Production has no Rust authority. No wildcards, shell, arbitrary
unit, enable, daemon-reload or file-install authority is granted to the app user.
An administrator retains installer and sudoers installation authority.

Operator-only sequence (not run by CI or an agent):

```bash
# From the exact approved clean environment checkout; select staging or production.
ENVIRONMENT=staging
SOURCE="deploy/sudoers/verdaxis-deploy-$ENVIRONMENT"
sudo /usr/sbin/visudo -cf "$SOURCE"
# Inspect the target first. Back up an existing regular file with cp -a.
DEST="/etc/sudoers.d/verdaxis-deploy-$ENVIRONMENT"
sudo test ! -L "$DEST"
if sudo test -e "$DEST"; then
  sudo cp -a "$DEST" "$DEST.before-s2"
fi
sudo install -o root -g root -m 0440 "$SOURCE" "$DEST"
sudo /usr/sbin/visudo -c
```

Inspect the existing `verdaxis-deploy` and emergency `verdaxis-rust-staging`
fragments before installation. They are not repository-owned. The operator must
review and retire redundant grants separately, preserving backups; do not delete
or overwrite them blindly. New deploys use single-unit commands only. Deploy
preflight uses `sudo -n -l` for every exact restart/stop/disable command it can
execute, before state/source mutation. Privileged execution must be noninteractive.

This source is not proof that a host installed it. A refused preflight is a stop,
not a reason to broaden authority or give the service account general sudo.
