#!/usr/bin/env bash
# Run one Quartus 13.0sp1 tool with no network, in the current directory.
#
#   tools/quartus.sh quartus_cpf -c a.sof a.rbf
#
# Tools chipsmith doesn't know by name (quartus_cdb, ...) go through
# quartus_sh: `tools/quartus.sh quartus_sh --tcl_eval exec quartus_cdb ...`.
set -euo pipefail

# The oracle is a 2013 binary that has no business talking to anything. Re-exec
# inside a fresh user+net namespace, where only a downed loopback exists.
# chipsmith's own bubblewrap sandbox nests inside it fine.
if [[ "${ORACLE_NETNS:-}" != 1 ]]; then
    exec env ORACLE_NETNS=1 unshare --user --map-current-user --net "$0" "$@"
fi
if awk 'NR > 2 && $1 != "lo:" { found = 1 } END { exit !found }' /proc/net/dev; then
    echo "quartus.sh: refusing to run, a network interface is visible" >&2
    exit 1
fi

exec chipsmith run --backend quartus-ii-13 --version 13.0sp1 "$1" -- "${@:2}"
