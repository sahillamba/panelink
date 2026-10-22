#!/bin/sh
# Local-only unsigned installer for artifacts produced by release_manifest.py.
set -eu
umask 077
install_pending=0

usage() {
    printf '%s\n' \
        'Usage: scripts/install.sh install [--replace-dev]' \
        '       scripts/install.sh upgrade' \
        '       scripts/install.sh rollback' \
        '       scripts/install.sh uninstall [--purge-state --confirm-purge-state]' >&2
    exit 2
}

fail() {
    printf 'panelink installer: %s\n' "$1" >&2
    exit 1
}

cleanup_temporary_paths() {
    for temporary_path in "${tmp_copy-}" "${tmp_record-}" "${previous_tmp-}" "${swap_tmp-}" "${dev_tmp-}"; do
        [ -z "$temporary_path" ] || rm -f "$temporary_path"
    done
    if [ "${dev_previous_moved-0}" -eq 1 ] && [ -L "${dev_previous-}" ]; then
        if [ -L "${dev_command-}" ] && [ "$(readlink "$dev_command")" = panelink-release ]; then
            rm -f "$dev_command"
        fi
        if [ ! -e "${dev_command-}" ] && [ ! -L "${dev_command-}" ]; then
            mv "$dev_previous" "$dev_command"
        fi
    fi
    if [ "${install_pending-0}" -eq 1 ]; then
        if [ "${has_dev_link-0}" -eq 1 ] && [ -L "${dev_command-}" ] && [ "$(readlink "$dev_command")" = panelink-release ]; then
            rm -f "$dev_command"
        fi
        if [ "${dev_previous_moved-0}" -eq 1 ] && [ -L "${dev_previous-}" ] && [ ! -e "${dev_command-}" ] && [ ! -L "${dev_command-}" ]; then
            mv "$dev_previous" "$dev_command"
        fi
        rm -f "${binary-}" "${notices-}" "${installed-}"
    fi
}

trap cleanup_temporary_paths 0
trap 'exit 1' HUP INT TERM

[ "$#" -ge 1 ] || usage
action=$1
shift
replace_dev=0
purge_state=0
confirm_purge_state=0
while [ "$#" -gt 0 ]; do
    case $1 in
        --replace-dev) replace_dev=1 ;;
        --purge-state) purge_state=1 ;;
        --confirm-purge-state) confirm_purge_state=1 ;;
        *) usage ;;
    esac
    shift
done

case $action in
    install|upgrade|rollback|uninstall) ;;
    *) usage ;;
esac

case $action in
install)
        [ "$purge_state" -eq 0 ] && [ "$confirm_purge_state" -eq 0 ] || usage
        ;;
    upgrade|rollback)
        [ "$replace_dev" -eq 0 ] && [ "$purge_state" -eq 0 ] && [ "$confirm_purge_state" -eq 0 ] || usage
        ;;
    uninstall)
        [ "$replace_dev" -eq 0 ] || usage
        if [ "$purge_state" -ne "$confirm_purge_state" ]; then
            fail 'state deletion requires both --purge-state and --confirm-purge-state'
        fi
        ;;
esac

script_dir=$(CDPATH= cd "$(dirname "$0")" && pwd -P)
repo_root=$(CDPATH= cd "$script_dir/.." && pwd -P)
release_dir=$repo_root/release
manifest=$release_dir/MANIFEST.json
source_binary=$release_dir/panelink-release
source_notices=$repo_root/THIRD_PARTY_NOTICES.md
prefix=${PREFIX:-"$HOME/.local"}
case $prefix in
    /*) ;;
    *) fail 'PREFIX must be an absolute path' ;;
esac
[ "$prefix" != / ] || fail 'PREFIX may not be /'
bin_dir=$prefix/bin
share_dir=$prefix/share/panelink
installed=$share_dir/INSTALLED
binary=$bin_dir/panelink-release
previous=$bin_dir/panelink-release.previous
dev_previous=$bin_dir/panelink.previous
notices=$share_dir/THIRD_PARTY_NOTICES.md
dev_command=$bin_dir/panelink

verify_release() {
    [ -f "$manifest" ] && [ ! -L "$manifest" ] || fail 'release/MANIFEST.json is missing or unsafe'
    [ -f "$source_binary" ] && [ ! -L "$source_binary" ] || fail 'release/panelink-release is missing or unsafe'
    [ -f "$source_notices" ] && [ ! -L "$source_notices" ] || fail 'THIRD_PARTY_NOTICES.md is missing or unsafe'
    python3 - "$manifest" "$source_binary" "$source_notices" "$script_dir" <<'PY'
import os
import sys
sys.path.insert(0, sys.argv[4])
import release_manifest

manifest_path, binary_path, notices_path = sys.argv[1:4]
try:
    manifest = release_manifest.load_manifest(__import__("pathlib").Path(manifest_path))
    if manifest.get("binary", {}).get("path") != "release/panelink-release":
        raise ValueError("unexpected manifest binary path")
    expected_binary = release_manifest.manifest_file_digest(manifest, "binary", "sha256")
    expected_notices = release_manifest.manifest_file_digest(manifest, "inputs", "notices_sha256")
    digest = release_manifest.sha256_file
    if digest(__import__("pathlib").Path(binary_path)) != expected_binary:
        raise ValueError("release binary SHA-256 does not match manifest")
    if digest(__import__("pathlib").Path(notices_path)) != expected_notices:
        raise ValueError("third-party notices SHA-256 does not match manifest")
except (OSError, KeyError, TypeError, ValueError) as error:
    print(f"panelink installer: release verification failed: {error}", file=sys.stderr)
    sys.exit(1)
PY
}

load_installed() {
    [ -f "$installed" ] && [ ! -L "$installed" ] || fail 'installation record is missing or unsafe'
    python3 - "$installed" <<'PY'
import sys

allowed = {
    "bin/panelink-release",
    "bin/panelink-release.previous",
    "bin/panelink",
    "bin/panelink.previous",
    "share/panelink/THIRD_PARTY_NOTICES.md",
}
try:
    with open(sys.argv[1], "r", encoding="utf-8") as stream:
        lines = [line.rstrip("\n") for line in stream]
    if not lines or lines[0] != "panelink-install-v1":
        raise ValueError("unknown installation record")
    paths = lines[1:]
    if not paths or len(paths) != len(set(paths)) or any(path not in allowed for path in paths):
        raise ValueError("invalid installation record paths")
    if "bin/panelink-release" not in paths or "share/panelink/THIRD_PARTY_NOTICES.md" not in paths:
        raise ValueError("installation record is incomplete")
    print("|" + "|".join(paths) + "|")
except (OSError, UnicodeError, ValueError) as error:
    print(f"panelink installer: {error}", file=sys.stderr)
    sys.exit(1)
PY
}

is_installed_path() {
    case "$owned_paths" in
        *"|$1|"*) return 0 ;;
        *) return 1 ;;
    esac
}

write_record() {
    record_tmp=$share_dir/INSTALLED.tmp.$$
    [ ! -e "$record_tmp" ] && [ ! -L "$record_tmp" ] || fail 'installation-record temporary path already exists'
    tmp_record=$record_tmp
    {
        printf '%s\n' 'panelink-install-v1'
        printf '%s\n' 'bin/panelink-release'
        printf '%s\n' 'share/panelink/THIRD_PARTY_NOTICES.md'
        if [ "$has_previous" -eq 1 ]; then
            printf '%s\n' 'bin/panelink-release.previous'
        fi
        if [ "$has_dev_link" -eq 1 ]; then
            printf '%s\n' 'bin/panelink'
        fi
        if [ "$has_prior_dev" -eq 1 ]; then
            printf '%s\n' 'bin/panelink.previous'
        fi
    } >"$record_tmp" || fail 'could not write installation record'
    chmod 600 "$record_tmp"
    mv -f "$record_tmp" "$installed"
    tmp_record=
}

assert_regular_or_absent() {
    path=$1
    label=$2
    if [ -e "$path" ] || [ -L "$path" ]; then
        [ -f "$path" ] && [ ! -L "$path" ] || fail "$label exists but is not a regular managed file"
    fi
}

workspace_is_stopped() {
    state_root=${PANELINK_HOME:-"$HOME/Library/Application Support/Panelink"}
    case $state_root in
        /*) ;;
        *) fail 'PANELINK_HOME must be absolute to check workspace status' ;;
    esac
    if [ -L "$state_root" ]; then
        fail 'state root is a symbolic link; cannot safely check workspace status'
    fi
    if [ -e "$state_root" ]; then
        [ -d "$state_root" ] || fail 'state root exists but is not a directory'
        probe_output=
        if probe_output=$(PANELINK_HOME="$state_root" "$binary" panes 2>&1); then
            printf '%s\n' 'panelink installer: a workspace is running; stop it with panelink stop before upgrade/rollback' >&2
            return 1
        fi
        case $probe_output in
            *'no workspace is running'*) ;;
            *)
                printf 'panelink installer: could not confirm workspace is stopped (%s); stop it with panelink stop and retry\n' "$probe_output" >&2
                return 1
                ;;
        esac
    fi
    return 0
}

copy_atomically() {
    source=$1
    destination=$2
    mode=$3
    temporary=$destination.tmp.$$
    [ ! -e "$temporary" ] && [ ! -L "$temporary" ] || fail "temporary path already exists: $temporary"
    tmp_copy=$temporary
    cp -p "$source" "$temporary" || fail "could not stage $destination"
    chmod "$mode" "$temporary"
    mv -f "$temporary" "$destination" || fail "could not install $destination"
    tmp_copy=
}

validate_prefix_separate_from_state() {
    state_root=$1
    python3 - "$prefix" "$state_root" <<'PY'
import os
import sys
prefix = os.path.realpath(sys.argv[1])
state = os.path.realpath(sys.argv[2])
common = os.path.commonpath((prefix, state))
if common in (prefix, state):
    print("panelink installer: PREFIX and Panelink state root must not contain one another", file=sys.stderr)
    sys.exit(1)
PY
}

case $action in
    install)
        [ ! -e "$installed" ] && [ ! -L "$installed" ] || fail 'already installed; use upgrade'
        verify_release
        state_root=${PANELINK_HOME:-"$HOME/Library/Application Support/Panelink"}
        validate_prefix_separate_from_state "$state_root" || exit 1
        mkdir -p "$bin_dir" "$share_dir"
        [ -d "$bin_dir" ] && [ ! -L "$bin_dir" ] || fail 'PREFIX/bin must be a real directory'
        [ -d "$share_dir" ] && [ ! -L "$share_dir" ] || fail 'PREFIX/share/panelink must be a real directory'
        [ ! -e "$binary" ] && [ ! -L "$binary" ] || fail 'refusing to overwrite an untracked panelink-release'
        [ ! -e "$notices" ] && [ ! -L "$notices" ] || fail 'refusing to overwrite untracked third-party notices'
        if [ -e "$dev_command" ] || [ -L "$dev_command" ]; then
            [ "$replace_dev" -eq 1 ] || [ -L "$dev_command" ] || fail 'refusing to install over a non-symlink panelink command; use another PREFIX'
            if [ "$replace_dev" -eq 1 ]; then
                [ -L "$dev_command" ] || fail 'refusing to replace a non-symlink panelink command'
                [ ! -e "$dev_previous" ] && [ ! -L "$dev_previous" ] || fail 'refusing to overwrite panelink.previous'
            fi
        fi
        has_previous=0
        has_dev_link=0
        has_prior_dev=0
        dev_previous_moved=0
        install_pending=1
        dev_tmp=
        record_tmp=
        copy_atomically "$source_binary" "$binary" 755
        copy_atomically "$source_notices" "$notices" 644
        if [ -e "$dev_command" ] || [ -L "$dev_command" ]; then
            if [ "$replace_dev" -eq 1 ]; then
                mv "$dev_command" "$dev_previous"
                dev_previous_moved=1
                has_prior_dev=1
                dev_tmp=$bin_dir/panelink.tmp.$$
                [ ! -e "$dev_tmp" ] && [ ! -L "$dev_tmp" ] || fail 'temporary panelink path already exists'
                ln -s panelink-release "$dev_tmp"
                mv -f "$dev_tmp" "$dev_command"
                has_dev_link=1
            fi
        elif [ "$replace_dev" -eq 1 ]; then
            dev_tmp=$bin_dir/panelink.tmp.$$
            [ ! -e "$dev_tmp" ] && [ ! -L "$dev_tmp" ] || fail 'temporary panelink path already exists'
            ln -s panelink-release "$dev_tmp"
            mv -f "$dev_tmp" "$dev_command"
            has_dev_link=1
        fi
        write_record
        dev_previous_moved=0
        install_pending=0
        printf 'panelink installer: installed %s\n' "$binary"
        ;;
    upgrade)
        owned_paths=$(load_installed)
        verify_release
        is_installed_path bin/panelink-release || fail 'installation does not own the release binary'
        is_installed_path share/panelink/THIRD_PARTY_NOTICES.md || fail 'installation does not own the notices file'
        state_root=${PANELINK_HOME:-"$HOME/Library/Application Support/Panelink"}
        validate_prefix_separate_from_state "$state_root" || exit 1
        assert_regular_or_absent "$binary" 'installed panelink-release'
        [ -f "$binary" ] || fail 'installed panelink-release is missing'
        workspace_is_stopped || exit 1
        if [ -e "$previous" ] || [ -L "$previous" ]; then
            is_installed_path bin/panelink-release.previous || fail 'previous binary exists but is not managed'
            assert_regular_or_absent "$previous" 'previous binary'
        fi
        if is_installed_path bin/panelink.previous; then
            [ -L "$dev_previous" ] || fail 'preserved development launcher is missing or unsafe'
        fi
        [ -d "$bin_dir" ] && [ ! -L "$bin_dir" ] || fail 'PREFIX/bin must be a real directory'
        [ -d "$share_dir" ] && [ ! -L "$share_dir" ] || fail 'PREFIX/share/panelink must be a real directory'
        has_previous=1
        has_dev_link=0
        if is_installed_path bin/panelink; then
            [ -L "$dev_command" ] || fail 'managed panelink command is missing or not a symlink'
            [ "$(readlink "$dev_command")" = panelink-release ] || fail 'managed panelink command was changed'
            has_dev_link=1
        fi
        has_prior_dev=0
        if is_installed_path bin/panelink.previous; then
            [ -L "$dev_previous" ] || fail 'preserved development launcher is missing or unsafe'
            has_prior_dev=1
        fi
        previous_tmp=$bin_dir/panelink-release.previous.tmp.$$
        [ ! -e "$previous_tmp" ] && [ ! -L "$previous_tmp" ] || fail 'previous-binary temporary path already exists'
        previous_tmp_cleanup=$previous_tmp
        previous_tmp=$previous_tmp_cleanup
        dev_tmp=
        record_tmp=
        previous_tmp_for_cleanup=$previous_tmp
        cp -p "$binary" "$previous_tmp" || fail 'could not stage previous binary for rollback'
        chmod 755 "$previous_tmp"
        mv -f "$previous_tmp" "$previous"
        previous_tmp_cleanup=
        previous_tmp=
        copy_atomically "$source_binary" "$binary" 755
        copy_atomically "$source_notices" "$notices" 644
        write_record
        printf 'panelink installer: upgraded %s; previous binary saved as %s\n' "$binary" "$previous"
        ;;
    rollback)
        owned_paths=$(load_installed)
        is_installed_path bin/panelink-release || fail 'installation does not own the release binary'
        is_installed_path bin/panelink-release.previous || fail 'no managed previous binary is available'
        state_root=${PANELINK_HOME:-"$HOME/Library/Application Support/Panelink"}
        validate_prefix_separate_from_state "$state_root" || exit 1
        [ -d "$bin_dir" ] && [ ! -L "$bin_dir" ] || fail 'PREFIX/bin must be a real directory'
        [ -d "$share_dir" ] && [ ! -L "$share_dir" ] || fail 'PREFIX/share/panelink must be a real directory'
        [ -f "$binary" ] && [ ! -L "$binary" ] || fail 'installed release binary is missing or unsafe'
        [ -f "$previous" ] && [ ! -L "$previous" ] || fail 'previous release binary is missing or unsafe'
        workspace_is_stopped || exit 1
        has_previous=1
        has_dev_link=0
        if is_installed_path bin/panelink; then
            [ -L "$dev_command" ] && [ "$(readlink "$dev_command")" = panelink-release ] || fail 'managed panelink command was changed'
            has_dev_link=1
        fi
        has_prior_dev=0
        if is_installed_path bin/panelink.previous; then
            [ -L "$dev_previous" ] || fail 'preserved development launcher is missing or unsafe'
            has_prior_dev=1
        fi
        swap_tmp=$bin_dir/panelink-release.swap.$$
        [ ! -e "$swap_tmp" ] && [ ! -L "$swap_tmp" ] || fail 'rollback temporary path already exists'
        dev_tmp=
        record_tmp=
        swap_tmp_for_cleanup=$swap_tmp
        cp -p "$binary" "$swap_tmp" || fail 'could not stage current binary'
        chmod 755 "$swap_tmp"
        copy_atomically "$previous" "$binary" 755
        mv -f "$swap_tmp" "$previous"
        write_record
        printf 'panelink installer: rolled back %s; prior binary retained as %s\n' "$binary" "$previous"
        ;;
    uninstall)
        owned_paths=$(load_installed)
        state_root=${PANELINK_HOME:-"$HOME/Library/Application Support/Panelink"}
        validate_prefix_separate_from_state "$state_root" || exit 1
        if [ "$purge_state" -eq 1 ]; then
            case $state_root in
                /*) ;;
                *) fail 'PANELINK_HOME must be absolute before state purge' ;;
            esac
            [ "$state_root" != / ] && [ "$state_root" != "$HOME" ] || fail 'refusing unsafe state-root purge path'
            [ ! -L "$state_root" ] || fail 'refusing to purge a symbolic-link state root'
            if [ -e "$state_root" ]; then
                [ -d "$state_root" ] || fail 'state-root purge target is not a directory'
                workspace_is_stopped || exit 1
            fi
        fi
        if is_installed_path bin/panelink-release; then
            assert_regular_or_absent "$binary" 'managed release binary'
        fi
        if is_installed_path bin/panelink-release.previous; then
            assert_regular_or_absent "$previous" 'managed previous binary'
        fi
        if is_installed_path share/panelink/THIRD_PARTY_NOTICES.md; then
            assert_regular_or_absent "$notices" 'managed third-party notices'
        fi
        if is_installed_path bin/panelink; then
            [ -L "$dev_command" ] && [ "$(readlink "$dev_command")" = panelink-release ] || fail 'managed panelink command was changed; refusing removal'
        fi
        if is_installed_path bin/panelink.previous; then
            [ -L "$dev_previous" ] || fail 'preserved development launcher is missing or unsafe'
            [ ! -e "$dev_command" ] || [ -L "$dev_command" ] || fail 'panelink path was replaced by an untracked file'
        fi
        if is_installed_path bin/panelink-release; then
            [ ! -L "$binary" ] || fail 'managed release binary became a symbolic link'
            rm -f "$binary"
        fi
        if is_installed_path bin/panelink-release.previous; then
            [ ! -L "$previous" ] || fail 'managed previous binary became a symbolic link'
            rm -f "$previous"
        fi
        if is_installed_path bin/panelink; then
            rm -f "$dev_command"
        fi
        if is_installed_path bin/panelink.previous; then
            mv "$dev_previous" "$dev_command"
        fi
        if is_installed_path share/panelink/THIRD_PARTY_NOTICES.md; then
            [ ! -L "$notices" ] || fail 'managed notices file became a symbolic link'
            rm -f "$notices"
        fi
        rm -f "$installed"
        if [ "$purge_state" -eq 1 ]; then
            if [ -e "$state_root" ]; then
                rm -rf "$state_root"
            fi
            printf 'panelink installer: uninstalled; confirmed state purge completed for %s\n' "$state_root"
        else
            printf 'panelink installer: uninstalled; Panelink state was preserved\n'
        fi
        ;;
esac
