# syntax=docker/dockerfile:1

################################################################################

ARG RUST_VERSION=1.82.0

FROM rust:${RUST_VERSION}-slim-bullseye AS build

WORKDIR /app

# Install the required dependencies for the build
# RUN apt-get update && apt-get install -y \
#     build-essential \
#     pkg-config \
#     libssl-dev

# Leverage a cache mount to /usr/local/cargo/registry/
# for downloaded dependencies and a cache mount to /app/target/ for 
# compiled dependencies which will speed up subsequent builds.
# Leverage a bind mount to the src directory to avoid having to copy the
# source code into the container. Once built, copy the executable to an
# output directory before the cache mounted /app/target is unmounted.
RUN --mount=type=bind,source=src,target=src \
    --mount=type=bind,source=Cargo.toml,target=Cargo.toml \
    --mount=type=bind,source=Cargo.lock,target=Cargo.lock \
    --mount=type=cache,target=/app/target/ \
    --mount=type=cache,target=/usr/local/cargo/registry/ \
    <<EOF
set -e
cargo build --locked --release --bin testflow
cp ./target/release/testflow /bin/testflow
EOF

################################################################################

FROM debian:bullseye-slim AS final


COPY --from=build /bin/testflow /bin/testflow

ENTRYPOINT ["/bin/testflow"]