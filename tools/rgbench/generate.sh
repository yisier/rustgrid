#!/usr/bin/env bash
# Generates the RustGrid docs benchmark data set (1M / 10M rows) in the local Docker databases.
# Usage: generate.sh mysql|postgres|clickhouse|oracle|mongo
set -uo pipefail
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
engine="${1:?usage: generate.sh mysql|postgres|clickhouse|oracle|mongo}"
started=$(date +%s)

case "$engine" in
  mysql)
    docker exec -i db-mysql mysql -uroot -pRoot123! --table < "$ROOT/sql/mysql.sql"
    ;;
  postgres)
    docker exec -i db-postgres psql -U postgres -v ON_ERROR_STOP=1 < "$ROOT/sql/postgres.sql"
    ;;
  clickhouse)
    docker exec -i db-clickhouse clickhouse-client --multiquery < "$ROOT/sql/clickhouse.sql"
    ;;
  oracle)
    docker exec -i db-oracle bash -lc "sqlplus -S TEST/Root123!@//localhost:1521/FREEPDB1" < "$ROOT/sql/oracle.sql"
    ;;
  mongo)
    docker exec -i db-mongodb mongosh -u root -p 'Root123!' --authenticationDatabase admin --quiet < "$ROOT/sql/mongo.js"
    ;;
  *)
    echo "unknown engine: $engine" >&2
    exit 2
    ;;
esac
rc=$?
echo "[$engine] exit=$rc elapsed=$(( $(date +%s) - started ))s"
exit $rc
