# syntax=docker/dockerfile:1

FROM rust:1.95.0-slim-bookworm AS rust
FROM ghcr.io/astral-sh/uv:0.11.26 AS uv

FROM almalinux:8 AS build
RUN dnf install -y gcc make python3.12 && dnf clean all
COPY --from=rust /usr/local/rustup /usr/local/rustup
COPY --from=rust /usr/local/cargo /usr/local/cargo
ENV RUSTUP_HOME=/usr/local/rustup CARGO_HOME=/usr/local/cargo PATH=/usr/local/cargo/bin:$PATH
WORKDIR /src
COPY . .

RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    cargo test --locked
# DuckDB's extensions are downloaded for the tests only; the release tarball holds ours.
RUN --mount=type=cache,target=/usr/local/cargo/registry --mount=type=cache,target=/src/target \
    make build dist PYTHON=python3.12

FROM scratch AS dist
COPY --from=build /src/dist /

FROM python:3.12-slim-bookworm AS test
RUN apt-get update \
 && apt-get install -y --no-install-recommends make \
 && rm -rf /var/lib/apt/lists/*
COPY --from=uv /uv /usr/local/bin/uv
ENV UV_PROJECT_ENVIRONMENT=/opt/venv UV_PYTHON_DOWNLOADS=never UV_LINK_MODE=copy UV_NO_SYNC=1 \
    PYTEST_ADDOPTS="-v -p no:cacheprovider"
WORKDIR /src
COPY pyproject.toml uv.lock .python-version ./
RUN --mount=type=cache,target=/root/.cache/uv \
    uv sync --locked --no-dev --group test
COPY --from=build /src .
RUN useradd --create-home tester
USER tester

CMD ["make", "test"]
