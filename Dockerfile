# syntax=docker/dockerfile:1
# check=skip=InvalidDefaultArgInFrom
#
# Stages:
#   build-custom  compile every crate under extensions/
#   fetch-core    download official core extensions and verify them against core-extensions.sha256
#   package       add metadata and a signature to custom extensions, then assemble the release tarball
#   dist          the tarball alone, for `docker build --output` (make dist)
#   test          Python dependencies from uv.lock and the tests; run it with --network none (make test)

ARG BUILD_IMAGE RUST_IMAGE PYTHON_IMAGE UV_IMAGE

FROM ${RUST_IMAGE} AS rust-toolchain
FROM ${UV_IMAGE} AS uv

FROM ${BUILD_IMAGE} AS build-custom
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
ARG TARGETARCH DUCKDB_VERSION CORE_EXTENSIONS
WORKDIR /work
COPY core-extensions.sha256 ./
COPY scripts/argpr.py scripts/fetch_core.py ./scripts/
RUN python scripts/fetch_core.py --version "$DUCKDB_VERSION" --platform "linux_${TARGETARCH}" \
      --lock core-extensions.sha256 --out /dist/core $CORE_EXTENSIONS

FROM ${PYTHON_IMAGE} AS package
ARG TARGETARCH DUCKDB_VERSION
RUN apt-get update \
 && apt-get install -y --no-install-recommends openssl \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /work
COPY scripts ./scripts
COPY extensions ./extensions
COPY --from=build-custom /out /build
COPY --from=fetch-core /dist/core /dist/core
RUN --mount=type=secret,id=signing_key,required=true <<'EOF'
set -eu
platform="linux_${TARGETARCH}"
for manifest in extensions/*/Cargo.toml; do
  python scripts/package_extension.py --manifest "$manifest" --library-dir /build \
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
ENV UV_PROJECT_ENVIRONMENT=/opt/venv \
    UV_PYTHON_DOWNLOADS=never \
    UV_LINK_MODE=copy \
    UV_COMPILE_BYTECODE=1
WORKDIR /work
COPY pyproject.toml uv.lock .python-version ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --group test
RUN useradd --create-home tester
COPY --from=package /artifacts /artifacts
COPY tests ./tests
USER tester
ENV PATH=/opt/venv/bin:$PATH \
    DUCKDB_ARTIFACT_DIR=/artifacts

CMD ["python", "-I", "-m", "pytest", "-v", "-p", "no:cacheprovider", "tests"]
