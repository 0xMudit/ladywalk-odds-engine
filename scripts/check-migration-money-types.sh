#!/usr/bin/env bash
# scripts/check-migration-money-types.sh
#
# Fails if any migration declares a money column as a float, real, double
# precision, or exact-numeric type. Money is always BIGINT minor units (Q01).
#
# Comments and string literals are removed before the scan, because the rule is
# about declared column types and not about prose. A migration that documents
# the rule by naming the forbidden types must not fail its own check, and
# stripping the comments is what lets that documentation exist.
#
# Usage:
#   ./scripts/check-migration-money-types.sh

set -euo pipefail

MIGRATION_DIR="crates/adapters/migrations"

# SQL identifiers that declare a type the money path must never use.
FORBIDDEN_TYPES=(
    'FLOAT'
    'REAL'
    'DOUBLE[[:space:]]+PRECISION'
    'NUMERIC'
    'DECIMAL'
)

# Reduce a migration to its executable statements: drop `--` line comments and
# everything inside single-quoted literals, which is where a CHECK constraint's
# message text lives.
strip_comments_and_literals() {
    sed -e "s/'[^']*'//g" -e 's/--.*$//' "$1"
}

echo "==> Checking migration files for forbidden money column types..."

FAILED=0
for sql_file in "$MIGRATION_DIR"/*.sql; do
    [ -f "$sql_file" ] || continue
    stripped="$(strip_comments_and_literals "$sql_file")"
    for type_name in "${FORBIDDEN_TYPES[@]}"; do
        if grep -qiE "\\b${type_name}\\b" <<<"$stripped"; then
            echo "ERROR: $sql_file declares a column of a forbidden type" \
                 "matching '$type_name'" >&2
            echo "       Money columns must be BIGINT minor units (Q01)." >&2
            FAILED=1
        fi
    done
done

if [[ $FAILED -eq 1 ]]; then
    exit 1
fi

echo "PASS: No forbidden money types declared in migrations."
