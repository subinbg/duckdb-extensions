include versions.env

# Cf. Emulated linux/amd64 on Apple Silicon fails DuckDB's signature checks even for official downloads.
ARCH ?= $(if $(filter x86_64 amd64,$(shell uname -m)),amd64,arm64)

# Local development key, created on first use.
# In CI, point SIGNING_KEY at a secret instead: only the default path is created, so a missing CI key fails the build.
DEV_KEY := keys/signing-key.pem
SIGNING_KEY ?= $(DEV_KEY)

VERSIONS := $(shell awk -F= '/^[A-Z_]+=/ { print $$1 }' versions.env)
DOCKER_BUILD := docker build --platform linux/$(ARCH) --secret id=signing_key,src=$(SIGNING_KEY) \
  $(foreach name,$(VERSIONS),--build-arg "$(name)=$($(name))")
TEST_IMAGE := duckdb-extensions-test:$(DUCKDB_VERSION)-$(ARCH)

DUCKDB_PIN := duckdb==$(patsubst v%,%,$(DUCKDB_VERSION))
NEO_MINOR := $(shell echo $(DUCKDB_VERSION) | awk -F'[v.]' '{ print $$2 * 10000 + $$3 * 100 + $$4 }')
PYTHON_VERSION := $(shell echo $(PYTHON_IMAGE) | awk -F'[:-]' '{ print $$2 }')

.PHONY: help test dist check pins lock cargo-lock clean

help:
	@echo "make test        build everything and run the loading tests offline (--network none)"
	@echo "make dist        write the release tarball to dist/"
	@echo "make check       version pins, ruff lint + format check, pyrefly strict type check"
	@echo "make pins        check that pyproject.toml, the Cargo.toml files and .python-version match versions.env"
	@echo "make lock        re-pin core extension hashes (after changing versions.env)"
	@echo "make cargo-lock  regenerate Cargo.lock (after changing Rust dependencies)"
	@echo "ARCH=amd64|arm64 overrides the target architecture (default: $(ARCH))"

test: pins $(SIGNING_KEY)
	$(DOCKER_BUILD) --target test -t $(TEST_IMAGE) .
	docker run --rm --network none $(TEST_IMAGE)

dist: pins $(SIGNING_KEY)
	$(DOCKER_BUILD) --target dist --output type=local,dest=dist .
	@ls -l dist

check: pins
	uv lock --check
	uv run ruff check
	uv run ruff format --check
	uv run pyrefly check


# `duckdb-neo` (Rust bindings for DuckDB's C API v2) should have a version compatible to DuckDB target.
# Its format is "x.abcde.y" where "abcde" is MAJOR*10000 + MINOR*100 + PATCH.
# For example, 0.20000.x targets v2.0.0 and 1.10506.x targets v1.5.6.
pins:
	@grep -q '"$(DUCKDB_PIN)"' pyproject.toml || \
	  { echo "pyproject.toml must pin $(DUCKDB_PIN) to match versions.env"; exit 1; }
	@grep -Eq '^duckdb-neo = .*"=0\.$(NEO_MINOR)\.[0-9]+"' Cargo.toml || \
	  { echo "Cargo.toml must pin duckdb-neo =0.$(NEO_MINOR).x to match versions.env"; exit 1; }
	@grep -qx '$(PYTHON_VERSION)' .python-version || \
	  { echo ".python-version must be $(PYTHON_VERSION) to match versions.env"; exit 1; }
	@grep -q '^wasm-bindgen = "=$(WASM_BINDGEN_VERSION)"' extensions/webui/app/Cargo.toml || \
	  { echo "extensions/webui/app/Cargo.toml must pin wasm-bindgen =$(WASM_BINDGEN_VERSION) to match versions.env"; exit 1; }

lock:
	uv run python scripts/fetch_core.py --update-lock --lock core-extensions.sha256 \
	  --version $(DUCKDB_VERSION) --platform linux_amd64 --platform linux_arm64 $(CORE_EXTENSIONS)

cargo-lock:
	docker run --rm -v "$(CURDIR)":/src -w /src $(RUST_IMAGE) cargo generate-lockfile

clean:
	rm -rf dist target

$(DEV_KEY):
	mkdir -p $(dir $@)
	openssl genpkey -algorithm RSA -pkeyopt rsa_keygen_bits:2048 -out $@
	chmod 600 $@
