#!/usr/bin/env bash
# Prepare dependencies only on a disposable GitHub-hosted Ubuntu VM.
set -euo pipefail
export LC_ALL=C
umask 077
fail() { printf 'rehearsal setup refused: %s\n' "$1" >&2; exit 1; }
[[ $# == 1 && $1 == --disposable ]] || fail 'use --disposable'
[[ $EUID == 0 && $(uname -s) == Linux ]] || fail 'root on Linux is required'
[[ ${CI:-} == true && ${GITHUB_ACTIONS:-} == true && ${RUNNER_ENVIRONMENT:-} == github-hosted ]] || fail 'a disposable GitHub-hosted runner is required'
[[ $(ps -p 1 -o comm=) == systemd ]] || fail 'a systemd VM is required'
# shellcheck source=/dev/null
source /etc/os-release
[[ $ID == ubuntu && ( $VERSION_ID == 22.04 || $VERSION_ID == 24.04 ) ]] || fail 'unsupported Ubuntu release'
case ${REHEARSAL_ENVIRONMENT:-}:$VERSION_ID in
    staging:22.04) expected_systemd=249 ;;
    production:24.04) expected_systemd=255 ;;
    *) fail 'the environment and runner pair do not match' ;;
esac
read -r _systemd systemd_version _rest < <(systemctl --version)
[[ $systemd_version == "$expected_systemd" ]] || fail 'unexpected systemd version'
for name in REHEARSAL_SOURCE_DIR REHEARSAL_CURRENT_ARTIFACT_DIR REHEARSAL_PREVIOUS_ARTIFACT_DIR; do
    value=${!name:-}
    [[ $value == /* && -d $value && ! -L $value ]] || fail 'an absolute existing input directory is required'
done
[[ ${REHEARSAL_EVIDENCE_DIR:-} == /* && ! -e $REHEARSAL_EVIDENCE_DIR && ! -L $REHEARSAL_EVIDENCE_DIR ]] || fail 'the evidence path must be new'
[[ -f $REHEARSAL_SOURCE_DIR/requirements.txt && -f $REHEARSAL_SOURCE_DIR/constraints.txt ]] || fail 'source dependencies are missing'
[[ ${CADDY_VERSION:-} == 2.11.7 && ${CADDY_SHA256:-} == 727b91701a392de6ebc5027509f548bf39979e5216340d0faed8fa5e69c84f8b ]] || fail 'the reviewed Caddy pin is required'
[[ -n ${REHEARSAL_PYTHON:-} && -x $REHEARSAL_PYTHON ]] || fail 'the CI Python interpreter is required'

work=$(mktemp -d /var/tmp/verdaxis-rehearsal-setup.XXXXXXXX)
trap 'rm -rf -- "$work"' EXIT
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq
apt-get install -y --no-install-recommends ca-certificates curl gnupg acl jq openssl perl binutils git python3-venv libpq-dev nftables
curl --fail --silent --show-error --location --retry 3 \
    https://www.postgresql.org/media/keys/ACCC4CF8.asc --output "$work/pgdg.asc"
fingerprint=$(gpg --batch --show-keys --with-colons "$work/pgdg.asc" | grep '^fpr:' | cut -d: -f10)
[[ $fingerprint == B97B0AFCAA1A47F044F244A07FCC7D46ACCC4CF8 ]] || fail 'unexpected PostgreSQL archive key'
gpg --batch --dearmor --output "$work/pgdg.gpg" "$work/pgdg.asc"
install -m 0644 "$work/pgdg.gpg" /usr/share/keyrings/verdaxis-rehearsal-pgdg.gpg
# The signed archive retains the exact database versions used by existing tests.
printf 'deb [signed-by=/usr/share/keyrings/verdaxis-rehearsal-pgdg.gpg] https://apt-archive.postgresql.org/pub/repos/apt %s-pgdg-archive main\n' "$VERSION_CODENAME" > /etc/apt/sources.list.d/verdaxis-rehearsal-pgdg.list
apt-get update -qq
postgres_version="17.9-1.pgdg${VERSION_ID}+1"
postgis_version="3.6.2+dfsg-1.pgdg${VERSION_ID}+1"
apt-get install -y --allow-downgrades --no-install-recommends \
    "postgresql-17=$postgres_version" "postgresql-client-17=$postgres_version" \
    "postgresql-17-postgis-3=$postgis_version" "postgresql-17-postgis-3-scripts=$postgis_version"
# Runner images can contain another cluster. Do not share its database or port.
while read -r version cluster _port status _rest; do
    if [[ $status == online* ]]; then
        pg_ctlcluster "$version" "$cluster" stop
    fi
done < <(pg_lsclusters --no-header)
# Native retirement checks use the real postgresql@17-main.service unit.
# Replace only this disposable runner's stock 17/main cluster.
if [[ -d /etc/postgresql/17/main ]]; then
    pg_dropcluster 17 main
fi
pg_createcluster 17 main --port 5432 --start-conf auto -- \
    --auth-local=peer --auth-host=scram-sha-256
pg_conftool 17 main set listen_addresses 127.0.0.1
pg_ctlcluster 17 main start
[[ $(runuser -u postgres -- /usr/lib/postgresql/17/bin/psql -XAtqc 'SHOW server_version') == 17.9* ]] || fail 'unexpected PostgreSQL server version'
runuser -u postgres -- /usr/lib/postgresql/17/bin/psql -Xv ON_ERROR_STOP=1 \
    -c 'CREATE EXTENSION postgis' postgres >/dev/null
[[ $(runuser -u postgres -- /usr/lib/postgresql/17/bin/psql -XAtqc 'SELECT postgis_lib_version()') == 3.6.2 ]] || fail 'unexpected PostGIS version'

curl --fail --silent --show-error --location --retry 3 \
    "https://github.com/caddyserver/caddy/releases/download/v${CADDY_VERSION}/caddy_${CADDY_VERSION}_linux_amd64.tar.gz" \
    --output "$work/caddy.tar.gz"
printf '%s  %s\n' "$CADDY_SHA256" "$work/caddy.tar.gz" | sha256sum --check --strict
tar -xzf "$work/caddy.tar.gz" -C "$work" caddy
install -m 0755 "$work/caddy" /usr/local/bin/caddy
[[ $(/usr/local/bin/caddy version) == v2.11.7* ]] || fail 'unexpected Caddy version'

# Copies avoid interpreter links in the retained disposable runtime closure.
[[ ! -e $REHEARSAL_SOURCE_DIR/venv ]] || fail 'the Python environment already exists'
"$REHEARSAL_PYTHON" -m venv --copies "$REHEARSAL_SOURCE_DIR/venv"
"$REHEARSAL_SOURCE_DIR/venv/bin/python" -m pip install \
    -r "$REHEARSAL_SOURCE_DIR/requirements.txt" -c "$REHEARSAL_SOURCE_DIR/constraints.txt"
printf 'Prepared PostgreSQL 17.9, PostGIS 3.6.2, Caddy 2.11.7 and the disposable Python environment.\n'
