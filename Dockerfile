# syntax=docker/dockerfile:1
#
# Stages:
#   build-custom  compile every crate under extensions/ (glibc 2.28, like DuckDB's own Linux builds)
#   fetch-core    download official core extensions, verify against core-extensions.sha256
#   package       add metadata + signature to custom extensions, assemble the release tarball
#   dist          the tarball alone, for `docker build --output` (make dist)
#   test          Python dependencies from uv.lock + the tests; run it with --network none (make test)
#
# Versions come from versions.env (Python packages from uv.lock). The signing key is a BuildKit secret:
#   --secret id=signing_key,src=keys/signing-key.pem

ARG RUST_VERSION=1.95.0
ARG UV_VERSION=0.11.26
ARG PYTHON_IMAGE=python:3.12-slim-bookworm

FROM rust:${RUST_VERSION}-slim-bookworm AS rust-toolchain
FROM ghcr.io/astral-sh/uv:${UV_VERSION} AS uv

# AlmaLinux 8 has glibc 2.28, the same floor as DuckDB's linux_amd64/linux_arm64 binaries,
# so the extension runs on any server that can run DuckDB itself.
FROM almalinux:8 AS build-custom
RUN dnf install -y gcc && dnf clean all
COPY --from=rust-toolchain /usr/local/rustup /usr/local/rustup
COPY --from=rust-toolchain /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY extensions ./extensions
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --locked \
 && mkdir -p /out \
 && cp target/release/lib*.so /out/

FROM ${PYTHON_IMAGE} AS fetch-core
ARG TARGETARCH
WORKDIR /work
COPY versions.env core-extensions.sha256 ./
COPY scripts/fetch_core.py ./scripts/
RUN . ./versions.env \
 && python -I scripts/fetch_core.py --version "$DUCKDB_VERSION" --platform "linux_${TARGETARCH}" \
      --lock core-extensions.sha256 --out /dist/core $CORE_EXTENSIONS

FROM ${PYTHON_IMAGE} AS package
ARG TARGETARCH
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY versions.env ./
COPY scripts ./scripts
COPY extensions ./extensions
COPY --from=build-custom /out /build
COPY --from=fetch-core /dist/core /dist/core
RUN --mount=type=secret,id=signing_key,required=true <<'EOF'
set -eu
. ./versions.env
platform="linux_${TARGETARCH}"
for manifest in extensions/*/Cargo.toml; do
  python -I scripts/package_extension.py --manifest "$manifest" --library-dir /build \
    --platform "$platform" --duckdb-version "$DUCKDB_VERSION" \
    --signing-key /run/secrets/signing_key --out /dist/custom
done
openssl pkey -in /run/secrets/signing_key -pubout -out /dist/custom.pub.pem
mkdir -p /artifacts
tarball="duckdb-extensions-${DUCKDB_VERSION}-${platform}.tar.gz"
tar -czf "/artifacts/${tarball}" -C /dist .
cd /artifacts && sha256sum "${tarball}" > "${tarball}.sha256"
tar -tzf "${tarball}"
EOF

FROM scratch AS dist
COPY --from=package /artifacts /

FROM ${PYTHON_IMAGE} AS test
COPY --from=uv /uv /usr/local/bin/uv
# Use the image's Python 3.12 (never download one) and keep the venv outside the work tree.
ENV UV_PROJECT_ENVIRONMENT=/opt/venv \
    UV_PYTHON_DOWNLOADS=never \
    UV_LINK_MODE=copy \
    UV_COMPILE_BYTECODE=1
WORKDIR /work
COPY pyproject.toml uv.lock .python-version ./
# --locked: fail if uv.lock is out of date with pyproject.toml; hashes in uv.lock are verified.
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --group test
RUN useradd --create-home tester
COPY --from=package /artifacts /artifacts
COPY tests ./tests
USER tester
ENV PATH=/opt/venv/bin:$PATH \
    DUCKDB_ARTIFACT_DIR=/artifacts
CMD ["python", "-I", "-m", "pytest", "-v", "-p", "no:cacheprovider", "tests"]
