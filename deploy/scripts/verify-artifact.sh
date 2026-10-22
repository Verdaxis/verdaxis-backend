#!/usr/bin/env bash
# Host-only verification. Candidate bytes are data, never commands.
set -euo pipefail
export LC_ALL=C
umask 077
unset TAR_OPTIONS
max_archive=$((256 * 1024 * 1024))
max_member=$((128 * 1024 * 1024))
work=
fail() { printf 'artifact verification refused: %s\n' "$1" >&2; exit 1; }
cleanup() { if [[ -n "$work" ]]; then rm -rf -- "$work"; fi; }
trap cleanup EXIT
trap 'exit 1' HUP INT TERM
archive= record= signature= public_key= output= source_sha=
repository=Verdaxis/verdaxis-backend
workflow_path=.github/workflows/rust-read-ci.yml
workflow_ref=refs/heads/staging
target=x86_64-unknown-linux-gnu
while (( $# )); do
    (( $# >= 2 )) || fail 'an option has no value'
    case "$1" in
        --archive) archive=$2 ;; --source-sha) source_sha=$2 ;;
        --output-dir) output=$2 ;; --provenance-record) record=$2 ;;
        --provenance-signature) signature=$2 ;; --provenance-public-key) public_key=$2 ;;
        --repository) repository=$2 ;; --workflow-path) workflow_path=$2 ;;
        --workflow-ref) workflow_ref=$2 ;; --target) target=$2 ;;
        *) fail 'unknown host verification option' ;;
    esac
    shift 2
done
# Preserve relative CLI inputs before entering the pinned output parent.
for input_var in archive record signature public_key; do
    input_value=${!input_var}
    if [[ -n "$input_value" && "$input_value" != /* ]]; then
        printf -v "$input_var" '%s/%s' "$PWD" "$input_value"
    fi
done
[[ "$source_sha" =~ ^[0-9a-f]{40}$ ]] || fail 'a full lowercase source SHA is required'
[[ "$repository" == Verdaxis/verdaxis-backend && "$workflow_path" == .github/workflows/rust-read-ci.yml && "$target" == x86_64-unknown-linux-gnu ]] || fail 'unapproved build identity'
[[ "$workflow_ref" == refs/heads/staging || "$workflow_ref" == refs/heads/prod ]] || fail 'unapproved source ref'
[[ -n "$archive" && -n "$record" && -n "$signature" && -n "$public_key" && -n "$output" ]] || fail 'required input is missing'
for tool in tar openssl sha256sum jq stat dd od cmp realpath timeout; do
    command -v "$tool" >/dev/null || fail 'a trusted system tool is missing'
done
tar --version | grep -q 'GNU tar' || fail 'GNU tar is required'

# Walk existing ancestors without following a symlink. Retain directory descriptors
# and compare identities after opening: later operations use the pinned directory,
# not a path that a rename could redirect.
walk_directory() {
    local path=$1 part before after
    [[ "$path" == /* ]] || path="$PWD/$path"
    IFS=/ read -r -a components <<< "$path"
    cd /
    for part in "${components[@]}"; do
        [[ -n "$part" ]] || continue
        [[ "$part" != . && "$part" != .. && ! -L "$part" && -d "$part" ]] || fail 'unsafe directory component'
        before=$(stat -c '%d:%i' -- "$part")
        exec {walk_fd}< "$part"
        [[ ! -L "$part" ]] || fail 'directory changed during open'
        cd -P -- "/proc/self/fd/$walk_fd"
        after=$(stat -c '%d:%i' .)
        exec {walk_fd}<&-
        [[ "$before" == "$after" ]] || fail 'directory changed during open'
    done
}
# Snapshot bounded regular inputs from pinned parents into our private directory.
# Open first, then fstat and copy only from that descriptor, never reopen a name.
snapshot() {
    local input=$1 dest=$2 limit=$3 name before fd size
    name=$(basename -- "$input")
    (
        walk_directory "$(dirname -- "$input")"
        [[ ! -L "$name" && -f "$name" ]] || fail 'input is not a regular file'
        before=$(stat -c '%d:%i' -- "$name")
        exec {fd}< "$name"
        [[ ! -L "$name" && -f "/proc/self/fd/$fd" ]] || fail 'input changed during open'
        [[ "$before" == "$(stat -Lc '%d:%i' "/proc/self/fd/$fd")" ]] || fail 'input changed during open'
        size=$(stat -Lc %s "/proc/self/fd/$fd")
        (( size > 0 && size <= limit )) || fail 'input exceeds its size bound'
        dd bs=65536 count=$(( (limit + 65535) / 65536 + 1 )) status=none <&"$fd" > "$dest"
        (( $(stat -c %s "$dest") <= limit )) || fail 'input grew beyond its size bound'
    )
}
# No output parent creation: the caller must supply an existing safe parent.
if [[ "$output" != /* ]]; then output="$PWD/$output"; fi
output_name=$(basename -- "$output")
[[ "$output_name" != / && "$output_name" != . && "$output_name" != .. ]] || fail 'unsafe output name'
walk_directory "$(dirname -- "$output")"
exec {parent_fd}< .
parent_identity=$(stat -c '%d:%i' .)
[[ $(stat -c %u .) == "$EUID" ]] || fail 'output parent is not owned by this user'
parent_mode=$(stat -c %a .)
(( (8#$parent_mode & 0022) == 0 )) || fail 'output parent is writable by another user'
check_output() {
    [[ ! -L "$output_name" ]] || fail 'output must not be a symlink'
    if [[ -e "$output_name" ]]; then
        [[ -d "$output_name" ]] || fail 'output is not a directory'
        [[ -z "$(find "$output_name" -mindepth 1 -maxdepth 1 -print -quit)" ]] || fail 'output is not empty'
    fi
}
check_output
work=$(mktemp -d "$PWD/.artifact-verify.XXXXXXXX")
# PWD can retain the descriptor path; make the private work path independent of it.
work=$(realpath -- "$work")
snapshot "$archive" "$work/archive.tar" "$max_archive"
snapshot "$record" "$work/record.json" 65536
snapshot "$signature" "$work/record.sig" 64
snapshot "$public_key" "$work/public.pem" 16384
[[ $(stat -c %s "$work/record.sig") == 64 ]] || fail 'invalid signature length'
openssl pkey -pubin -in "$work/public.pem" -text -noout | grep -q '^ED25519 Public-Key:' || fail 'an Ed25519 public key is required'
openssl pkeyutl -verify -rawin -pubin -inkey "$work/public.pem" -in "$work/record.json" -sigfile "$work/record.sig" >/dev/null 2>&1 || fail 'invalid provenance signature'
hash() { local value; value=$(sha256sum -- "$1"); printf '%s' "${value%% *}"; }
archive_digest=$(hash "$work/archive.tar")
jq -e --arg repo "$repository" --arg workflow "$workflow_path" --arg ref "$workflow_ref" --arg sha "$source_sha" --arg target "$target" --arg digest "$archive_digest" '
    type == "object" and
    (keys == (["version","repository","workflow_path","workflow_ref","source_sha","target","archive_sha256","binary_sha256","attested_subjects","gh_attestation_verified","attested_at"] | sort)) and
    .version == 1 and .repository == $repo and .workflow_path == $workflow and
    .workflow_ref == $ref and .source_sha == $sha and .target == $target and
    .archive_sha256 == $digest and (.binary_sha256 | type == "string" and test("^[0-9a-f]{64}$")) and
    .gh_attestation_verified == true and .attested_subjects == ["verdaxis-read","archive"]
' "$work/record.json" >/dev/null || fail 'provenance bindings do not match'

# Read only plain USTAR headers. GNU tar hides extension records in its listing;
# reject those (including sparse/PAX/GNU long-name records) before invoking it.
# This also rejects compressed archives without letting tar auto-decompress them.
archive_size=$(stat -c %s "$work/archive.tar")
(( archive_size % 512 == 0 )) || fail 'invalid archive length'
offset=0 count=0 total=0
names=()
while (( offset + 512 <= archive_size )); do
    dd if="$work/archive.tar" of="$work/header" bs=512 skip=$((offset / 512)) count=1 status=none
    if [[ -z "$(tr -d '\000' < "$work/header")" ]]; then
        (( archive_size - offset >= 1024 )) || fail 'missing archive terminator'
        [[ $(dd if="$work/archive.tar" bs=512 skip=$((offset / 512)) status=none | tr -d '\000' | wc -c) == 0 ]] || fail 'data after archive terminator'
        break
    fi
    (( ++count <= 64 )) || fail 'too many archive members'
    magic=$(dd if="$work/header" bs=1 skip=257 count=6 status=none | od -An -tx1 | tr -d ' \n')
    [[ "$magic" == 757374617200 ]] || fail 'only plain USTAR is accepted'
    kind=$(dd if="$work/header" bs=1 skip=156 count=1 status=none | od -An -tx1 | tr -d ' \n')
    [[ "$kind" == 30 || "$kind" == 00 ]] || fail 'nonregular archive member'
    name=$(dd if="$work/header" bs=1 count=100 status=none | tr -d '\000')
    prefix=$(dd if="$work/header" bs=1 skip=345 count=155 status=none | tr -d '\000')
    if [[ -n "$prefix" ]]; then name="$prefix/$name"; fi
    [[ "$name" == verdaxis-read || "$name" == artifact-metadata.json || "$name" == SHA256SUMS || "$name" =~ ^data/[A-Za-z0-9._-]+$ ]] || fail 'unexpected archive member path'
    [[ "$name" != data/. && "$name" != data/.. ]] || fail 'unsafe data member'
    for previous in "${names[@]}"; do [[ "$previous" != "$name" ]] || fail 'duplicate archive member'; done
    names+=("$name")
    size=$(dd if="$work/header" bs=1 skip=124 count=12 status=none | tr -d '\000 ')
    [[ "$size" =~ ^[0-7]{1,11}$ ]] || fail 'invalid member size'
    size=$((8#$size))
    (( size <= max_member )) || fail 'oversized member'
    total=$((total + size))
    (( total <= max_archive )) || fail 'oversized member total'
    offset=$((offset + 512 + ((size + 511) / 512) * 512))
    (( offset <= archive_size )) || fail 'truncated archive'
done
(( count >= 3 && offset + 1024 <= archive_size )) || fail 'incomplete archive'
printf '%s\n' "${names[@]}" > "$work/raw-names"
timeout 30 tar --list --file "$work/archive.tar" --quoting-style=escape > "$work/tar-names"
cmp -s "$work/raw-names" "$work/tar-names" || fail 'archive parser disagreement'
mkdir -m 700 "$work/tree"
cd "$work/tree"
timeout 30 tar --extract --file "$work/archive.tar" --no-same-owner --no-same-permissions --keep-old-files --no-overwrite-dir
# No member may redirect a subsequent read, including after extraction returns.
for name in "${names[@]}"; do
    [[ ! -L "$name" && -f "$name" && $(stat -c %h -- "$name") == 1 ]] || fail 'extracted member is not a private regular file'
done
[[ ! -L data && -d data ]] || fail 'unsafe extracted data directory'
metadata=artifact-metadata.json
jq -e --arg sha "$source_sha" --arg target "$target" '
    def fields($names): type == "object" and keys == ($names | sort);
    def matches($pattern): type == "string" and test($pattern);
    def digest: matches("^[0-9a-f]{64}$");
    def revision: matches("^[a-z0-9_]+$");
    fields(["version","release_sha","target","libc_min","toolchain","lockfile_sha256","workflow_path","binary","policy","revision_allowlist","files","runtime_hosts"]) and
    .version == 1 and .release_sha == $sha and .target == $target and .libc_min == "2.35" and
    (.toolchain | matches("^[0-9]+\\.[0-9]+\\.[0-9]+$")) and (.lockfile_sha256 | digest) and
    .workflow_path == ".github/workflows/rust-read-ci.yml" and
    (.binary | fields(["path","sha256"]) and .path == "verdaxis-read" and (.sha256 | digest)) and
    (.policy | fields(["source_sha","policy_path","policy_sha256","manifest_path","manifest_sha256"]) and
        (.source_sha | matches("^[0-9a-f]{40}$")) and .policy_path == "data/market-policy.json" and
        .manifest_path == "data/market-policy.manifest.json" and (.policy_sha256 | digest) and (.manifest_sha256 | digest)) and
    (.revision_allowlist | fields(["path","sha256","revisions"]) and .path == "data/allowed-revisions.json" and
        (.sha256 | digest) and (.revisions | type == "array" and length > 0 and all(.[]; revision))) and
    (.files | type == "array" and length > 0 and all(.[]; fields(["path","sha256"]) and
        (.path | matches("^data/[A-Za-z0-9._-]+$")) and (.sha256 | digest)) and
        ([.[].path] | length == (unique | length))) and
    (.runtime_hosts | fields(["staging","production"]) and all(.[];
        fields(["arch","glibc"]) and .arch == "x86_64" and (.glibc | matches("^[0-9]+\\.[0-9]+$"))))
' "$metadata" >/dev/null || fail 'invalid metadata schema or build identity'
{ printf '%s\n' verdaxis-read artifact-metadata.json SHA256SUMS; jq -r '.files[].path' "$metadata"; } | sort > "$work/expected"
sort "$work/raw-names" > "$work/actual"
cmp -s "$work/expected" "$work/actual" || fail 'member set differs from metadata'
binary_digest=$(hash verdaxis-read)
[[ "$binary_digest" == "$(jq -r .binary.sha256 "$metadata")" && "$binary_digest" == "$(jq -r .binary_sha256 "$work/record.json")" ]] || fail 'binary digest mismatch'
# Parse sums ourselves: sha256sum -c alone accepts duplicate or incomplete lists.
: > "$work/sum-names"
while IFS= read -r line || [[ -n "$line" ]]; do
    [[ -n "$line" ]] || continue
    [[ "$line" =~ ^([0-9a-f]{64})\ \ (verdaxis-read|artifact-metadata\.json|data/[A-Za-z0-9._-]+)$ ]] || fail 'invalid checksum entry'
    sum=${BASH_REMATCH[1]} name=${BASH_REMATCH[2]}
    [[ ! -L "$name" && -f "$name" && "$(hash "$name")" == "$sum" ]] || fail 'member checksum mismatch'
    printf '%s\n' "$name" >> "$work/sum-names"
done < SHA256SUMS
sort "$work/sum-names" > "$work/sorted-sums"
grep -v '^SHA256SUMS$' "$work/expected" > "$work/expected-sums"
cmp -s "$work/sorted-sums" "$work/expected-sums" || fail 'checksum coverage mismatch'
while IFS=$'\t' read -r name sum; do
    [[ "$(hash "$name")" == "$sum" ]] || fail 'metadata file digest mismatch'
done < <(jq -r '.files[] | [.path,.sha256] | @tsv' "$metadata")
for binding in 'policy.policy_path policy.policy_sha256' 'policy.manifest_path policy.manifest_sha256' 'revision_allowlist.path revision_allowlist.sha256'; do
    read -r path_field digest_field <<< "$binding"
    name=$(jq -r ".$path_field" "$metadata")
    [[ -f "$name" && ! -L "$name" && "$(hash "$name")" == "$(jq -r ".$digest_field" "$metadata")" ]] || fail 'data binding mismatch'
done
jq -e --slurpfile meta "$metadata" '.source_sha == $meta[0].policy.source_sha and .policy_sha256 == $meta[0].policy.policy_sha256' data/market-policy.manifest.json >/dev/null || fail 'policy manifest binding mismatch'
jq -e --slurpfile meta "$metadata" '. == $meta[0].revision_allowlist.revisions' data/allowed-revisions.json >/dev/null || fail 'revision allowlist binding mismatch'
# Recheck the complete tree, then normalize modes rather than trust archive modes.
[[ $(find . -type f | wc -l) == "$count" && -z "$(find . ! -type f ! -type d -print -quit)" ]] || fail 'extraction tree changed'
for name in "${names[@]}"; do chmod 644 -- "$name"; done
chmod 755 verdaxis-read data
cd -- "/proc/self/fd/$parent_fd"
[[ "$(stat -c '%d:%i' .)" == "$parent_identity" && "$(stat -c '%d:%i' -- "$(dirname -- "$output")")" == "$parent_identity" ]] || fail 'output parent changed'
check_output
# GNU mv -T replaces an absent or empty directory in one rename. It refuses a
# nonempty destination; it never merges candidate files into user data.
mv --no-target-directory -- "$work/tree" "$output_name"
printf 'verified verdaxis-read sha256=%s from release %s into %s\n' "$binary_digest" "$source_sha" "$output"
