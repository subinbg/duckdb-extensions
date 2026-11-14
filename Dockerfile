# syntax=docker/dockerfile:1
#
# The Docker system. This file only sets up its two machines, `build` and `test`.
# What they run is not decided here: the last four `RUN` lines of `build` and the `CMD` of `test`
# are commands of README.md "Build, install, test", as written there.
#
#   build  AlmaLinux 8 with cargo and uv; its glibc 2.28 is the floor of DuckDB's own Linux binaries
#   dist   the release tarball alone, for `docker build --output` (make dist)
#   test   Debian with Python, uv and no Rust, as a server that receives the extensions;
#          run it with --network none (make test)

FROM rust:1.95.0-slim-bookworm AS rust
FROM ghcr.io/astral-sh/uv:0.11.26 AS uv

FROM almalinux:8 AS build
RUN dnf install -y gcc && dnf clean all
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo /usr/local/cargo
COPY --from=uv /uv /usr/local/bin/uv
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH \
    UV_PROJECT_ENVIRONMENT=/opt/venv UV_LINK_MODE=copy UV_NO_SYNC=1
WORKDIR /src
COPY pyproject.toml uv.lock .python-version ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --group build
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo test --locked
RUN uv run scripts/fetch_core.py
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    --mount=type=secret,id=signing_key,target=/src/keys/signing-key.pem \
    uv run scripts/build_custom.py
RUN uv run scripts/bundle.py

FROM scratch AS dist
COPY --from=build /src/dist /

FROM python:3.12-slim-bookworm AS test
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_PROJECT_ENVIRONMENT=/opt/venv UV_PYTHON_DOWNLOADS=never UV_LINK_MODE=copy UV_NO_SYNC=1
WORKDIR /src
COPY pyproject.toml uv.lock .python-version ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --group test
COPY scripts scripts
COPY tests tests
COPY --from=build /src/build/repository build/repository
RUN useradd --create-home tester
USER tester

CMD ["uv", "run", "pytest", "-v", "-p", "no:cacheprovider"]
