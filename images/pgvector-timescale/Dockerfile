# gadgetron-pgvector-timescale — Postgres 16 image carrying *both*
# pgvector (vector similarity for the knowledge layer) and TimescaleDB
# (hypertables for host_metrics — see docs/design/phase2/16-server-metrics-timeseries.md).
#
# We base on timescale/timescaledb because:
#   (a) TimescaleDB ships the matching extension ABI against the bundled
#       Postgres major, so version drift across upgrades stays contained
#       in one image owner.
#   (b) timescaledb-tune is preinstalled and the entrypoint runs it on
#       first boot — saves an operator bootstrap step.
#
# pgvector is added on top from the Debian testing archive (Postgres's
# apt repo ships matching postgresql-16-pgvector for pg16).
FROM timescale/timescaledb:latest-pg16

# psql-side tooling helpful for operators poking the DB by hand, plus
# `ca-certificates` so apt-get can reach the PGDG mirror over HTTPS.
# `gosu` is already in the base image.
USER root

RUN set -eux; \
    apk add --no-cache ca-certificates curl gnupg; \
    # The timescale/timescaledb image is Alpine-based, so the Debian
    # pgvector package isn't directly usable. Build pgvector from source
    # against the bundled Postgres headers — 150 KB .so, 5 s compile.
    apk add --no-cache --virtual .build-deps build-base postgresql16-dev git; \
    # pgvector's Makefile emits LLVM bitcode (`.bc`) files for Postgres'
    # JIT, which needs a clang compatible with the Postgres build. On
    # the Alpine base we skip JIT entirely via `with_llvm=no` at `make`
    # time — vector indexes don't rely on JIT, so this is a harmless
    # size/perf trade-off (saves ~150 kB of bitcode per extension).
    git clone --branch v0.8.0 --depth 1 https://github.com/pgvector/pgvector.git /tmp/pgvector; \
    cd /tmp/pgvector; \
    make OPTFLAGS="" with_llvm=no; \
    make with_llvm=no install; \
    cd /; \
    rm -rf /tmp/pgvector; \
    apk del .build-deps

# Sanity check at build time — the extension files must be on disk.
# Actual CREATE EXTENSION runs at first boot / migration time.
RUN test -f /usr/local/lib/postgresql/vector.so \
    && test -f /usr/local/share/postgresql/extension/vector.control

# Label block so `docker image inspect` reveals the provenance.
LABEL org.opencontainers.image.title="gadgetron-pgvector-timescale" \
      org.opencontainers.image.description="Postgres 16 + TimescaleDB (latest) + pgvector 0.8.0" \
      org.opencontainers.image.source="https://github.com/NacBang/gadgetron" \
      org.opencontainers.image.licenses="PostgreSQL AND TSL (TimescaleDB Community)"

# Entrypoint + CMD inherited from base image — no override needed.
