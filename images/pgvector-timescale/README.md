# gadgetron-pgvector-timescale

Postgres 16 image carrying both **pgvector** (knowledge layer semantic
search) and **TimescaleDB** (`host_metrics` hypertable, see
`docs/design/phase2/16-server-metrics-timeseries.md`).

## Why one image

Every gadgetron deployment wants both extensions — vector similarity
for the wiki / knowledge stack and hypertables for host telemetry.
Running two Postgres instances doubles credential / backup / migration
burden. A single image with both extensions is the canonical pattern
from both vendors' docs.

## Build

```sh
cd images/pgvector-timescale
docker build -t gadgetron-pgvector-timescale:pg16 .
```

Build time ~30 s on a warm cache, ~2 min cold. pgvector is compiled
from source (v0.8.0) against the bundled Postgres headers with
`with_llvm=no` — no Postgres JIT for vector ops, which is a harmless
trade-off (vector ops aren't JIT-hot).

## Smoke test

```sh
docker run -d --name ts-smoke \
  -e POSTGRES_USER=gadgetron \
  -e POSTGRES_PASSWORD=secret \
  -e POSTGRES_DB=gadgetron_smoke \
  -p 5433:5432 \
  gadgetron-pgvector-timescale:pg16

docker exec ts-smoke psql -U gadgetron -d gadgetron_smoke -c "
  CREATE EXTENSION timescaledb;
  CREATE EXTENSION vector;
  SELECT extname, extversion FROM pg_extension
   WHERE extname IN ('timescaledb', 'vector');
"
```

Expected rows:

```
   extname   | extversion
-------------+------------
 timescaledb | 2.26.3
 vector      | 0.8.0
```

Combined hypertable + vector column works (verified 2026-04-21):

```sql
CREATE TABLE x (
  ts TIMESTAMPTZ NOT NULL,
  embedding VECTOR(3),
  val DOUBLE PRECISION
);
SELECT create_hypertable('x', 'ts');
INSERT INTO x VALUES (NOW(), '[1,2,3]', 42.0);
```

## Ownership

- **Base image**: `timescale/timescaledb:latest-pg16` (Alpine, Timescale
  tracks Postgres major release cadence)
- **pgvector source**: `github.com/pgvector/pgvector` tag `v0.8.0` pinned
- **Rebuild trigger**: bump pgvector tag, or when Postgres major rolls
  (Postgres 17 → update both base tag and pgvector dependency)

A single contributor owns the `docker build + docker push` step for
now — automating via CI workflow is a follow-up tracked in
`docs/ops/postgres-image.md` (pending).

## Licensing

- Postgres: PostgreSQL License (permissive)
- TimescaleDB: Apache-2.0 for Community; extended features under TSL
  (TimescaleDB Community License). `host_metrics` hypertable +
  continuous aggregates land in the Community feature set — no TSL
  code paths used.
- pgvector: PostgreSQL License
