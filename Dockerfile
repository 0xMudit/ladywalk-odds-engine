# ─────────────────────────────────────────────────────────────────────────────
# Stage 1 — Chef planner (dependency caching layer)
# ─────────────────────────────────────────────────────────────────────────────
FROM lukemathwalker/cargo-chef:latest-rust-1.98.1 AS chef
WORKDIR /app

# ─────────────────────────────────────────────────────────────────────────────
# Stage 2 — Planner: generate a recipe file of dependencies
# ─────────────────────────────────────────────────────────────────────────────
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ─────────────────────────────────────────────────────────────────────────────
# Stage 3 — Builder: compile dependencies (cached if recipe unchanged)
# ─────────────────────────────────────────────────────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json

# Build dependencies only — this layer is cached until Cargo.toml changes.
RUN cargo chef cook --release --recipe-path recipe.json

# Now copy source and build the binary
COPY . .

# SQLX_OFFLINE=true so the build works without a live database.
# The .sqlx/ metadata is committed alongside the source.
ENV SQLX_OFFLINE=true

RUN cargo build --release --bin order-entry

# ─────────────────────────────────────────────────────────────────────────────
# Stage 4 — Runtime: minimal image, no build toolchain
# ─────────────────────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# Install only runtime SSL certs (needed for TLS connections to Postgres)
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the compiled binary from the builder stage
COPY --from=builder /app/target/release/order-entry /app/order-entry

# Non-root user for security
RUN useradd --uid 10001 --no-create-home --shell /sbin/nologin appuser
USER appuser

EXPOSE 8080

# Tini as init — forwards signals correctly so SIGTERM triggers graceful shutdown
ENTRYPOINT ["/app/order-entry"]
