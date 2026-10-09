#!/usr/bin/env bash
# scripts/check-domain-deps.sh
#
# Fails if the `domain` crate depends on any infrastructure crate, directly or
# transitively:
#
#   adapters  application  sqlx  redis  axum  tower  reqwest  hyper  actix
#
# `crates/domain/tests/dependency_purity.rs` checks the direct dependency list.
# This script checks the *resolved graph*, which is the stronger claim: it also
# catches a crate that is not itself infrastructure but that drags one in.
#
# Usage:
#   ./scripts/check-domain-deps.sh
#
# Called by CI. Requires a resolvable workspace, so run it after `cargo build`
# or at least `cargo metadata`.

set -euo pipefail

FORBIDDEN_DEPS=(
    adapters
    application
    sqlx
    redis
    axum
    tower
    reqwest
    hyper
    actix
)

echo "==> Checking the domain crate's resolved dependency graph..."

# `cargo metadata` is run without --no-deps so that the full resolve graph is
# present; --no-deps reports only the workspace's direct edges, which cannot
# answer the transitive question.
#
# The metadata is written to a file rather than piped, because the analysis
# below is a here-document and a here-document owns stdin. Passing the path as
# an argument also keeps this working no matter how the two are combined.
METADATA_FILE=$(mktemp)
trap 'rm -f "$METADATA_FILE"' EXIT

if ! cargo metadata --format-version 1 --all-features >"$METADATA_FILE" 2>/dev/null; then
    echo "ERROR: cargo metadata failed; the workspace does not resolve." >&2
    exit 1
fi

# The here-document delimiter is quoted, so nothing inside it is expanded by
# the shell. An inline `python3 -c "..."` string is not safe here: a backtick
# in a Python comment silently becomes a command substitution, and the
# resulting 240KB of cargo output is passed to execve as one argument.
REACHABLE=$(python3 - "$METADATA_FILE" <<'PYTHON'
import json
import sys

with open(sys.argv[1], encoding="utf-8") as handle:
    meta = json.load(handle)

packages = meta["packages"]
by_id = {package["id"]: package for package in packages}

domain_id = next(
    (package["id"] for package in packages if package["name"] == "domain"),
    None,
)
if domain_id is None:
    sys.exit("no package named 'domain' in the workspace")

# Breadth-first walk of the resolved graph from domain.
nodes = {node["id"]: node for node in meta.get("resolve", {}).get("nodes", [])}
seen = {domain_id}
queue = [domain_id]
while queue:
    current = queue.pop()
    for dependency in nodes.get(current, {}).get("dependencies", []):
        if dependency not in seen:
            seen.add(dependency)
            queue.append(dependency)

# Several package ids can share a name when a crate is present at two
# semver-incompatible versions, so the names are collected into a set.
names = set()
for package_id in seen:
    name = by_id.get(package_id, {}).get("name")
    if name and name != "domain":
        names.add(name)

for name in sorted(names):
    print(name)
PYTHON
)

if [[ -z "$REACHABLE" ]]; then
    echo "FAIL: domain resolves to no packages at all, which means the" >&2
    echo "      dependency check ran against nothing." >&2
    exit 1
fi

echo "    reachable from domain:"
echo "$REACHABLE" | sed 's/^/      /'

FAILED=0
for dep in "${FORBIDDEN_DEPS[@]}"; do
    if echo "$REACHABLE" | grep -qx "$dep"; then
        echo "ERROR: domain's dependency graph contains: $dep" >&2
        FAILED=1
    fi
done

if [[ $FAILED -eq 1 ]]; then
    echo "" >&2
    echo "FAIL: The domain crate must remain infrastructure-free." >&2
    echo "      Removing the dependency that reaches it is the fix; allowlisting" >&2
    echo "      it here would make the guard a comment." >&2
    exit 1
fi

echo "PASS: domain's dependency graph contains no infrastructure crate."
