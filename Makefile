include versions.env

# Build for the host's architecture by default (emulated linux/amd64 on Apple Silicon
# fails DuckDB's signature checks even for official downloads, so test natively).
HOST_ARCH := $(shell uname -m)
ARCH ?= $(if $(filter x86_64 amd64,$(HOST_ARCH)),amd64,arm64)
DOCKER_PLATFORM := linux/$(ARCH)

DEV_KEY := keys/signing-key.pem
SIGNING_KEY ?= $(DEV_KEY)
TEST_IMAGE := duckdb-extensions-test:$(DUCKDB_VERSION)-$(ARCH)
CORE_LIST := $(subst ",,$(CORE_EXTENSIONS))
DUCKDB_PIN := duckdb==$(patsubst v%,%,$(DUCKDB_VERSION))
# duckdb-neo in Cargo.toml: Rust bindings for DuckDB's C API v2. The version encodes the DuckDB release it is
# built against (0.20000.x = v2.0.0) and must match DUCKDB_VERSION in versions.env. The middle number is
# MAJOR*10000 + MINOR*100 + PATCH, as in the duckdb and libduckdb-sys crates (1.10506.x = v1.5.6).
NEO_MINOR := $(shell echo $(DUCKDB_VERSION) | awk -F'[v.]' '{ print $$2 * 10000 + $$3 * 100 + $$4 }')
DOCKER_BUILD := docker build --platform $(DOCKER_PLATFORM) \
  --build-arg RUST_VERSION=$(RUST_VERSION) --build-arg UV_VERSION=$(UV_VERSION) \
  --secret id=signing_key,src=$(SIGNING_KEY)

.PHONY: help test dist check lock cargo-lock clean

help:
	@echo "make test        build everything and run the loading tests offline (--network none)"
	@echo "make dist        write the release tarball to dist/"
	@echo "make check       version pins vs versions.env, ruff lint + format check, pyrefly strict type check"
	@echo "make lock        re-pin core extension hashes (after changing versions.env)"
	@echo "make cargo-lock  regenerate Cargo.lock (after changing Rust dependencies)"
	@echo "ARCH=amd64|arm64 overrides the target architecture (default: $(ARCH))"

# Local development key, created on first use. In CI, point SIGNING_KEY at a secret instead;
# only the default path is auto-created, so a missing CI key fails the build.
$(DEV_KEY):
	mkdir -p $(dir $@)
	openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out $@
	chmod 600 $@

test: $(SIGNING_KEY)
	$(DOCKER_BUILD) --target test -t $(TEST_IMAGE) .
	docker run --rm --network none $(TEST_IMAGE)

dist: $(SIGNING_KEY)
	$(DOCKER_BUILD) --target dist --output type=local,dest=dist .
	@ls -l dist

check:
	@grep -q '"$(DUCKDB_PIN)"' pyproject.toml || \
	  { echo "pyproject.toml must pin $(DUCKDB_PIN) to match versions.env"; exit 1; }
	@grep -Eq '^duckdb-neo = .*"=0\.$(NEO_MINOR)\.[0-9]+"' Cargo.toml || \
	  { echo "Cargo.toml must pin duckdb-neo =0.$(NEO_MINOR).x to match versions.env"; exit 1; }
	uv lock --check
	uv run ruff check
	uv run ruff format --check
	uv run pyrefly check

lock:
	uv run python scripts/fetch_core.py --update-lock --lock core-extensions.sha256 \
	  --version $(DUCKDB_VERSION) --platform linux_amd64 --platform linux_arm64 $(CORE_LIST)

cargo-lock:
	docker run --rm -v "$(CURDIR)":/src -w /src rust:$(RUST_VERSION)-slim-bookworm cargo generate-lockfile

clean:
	rm -rf dist target
