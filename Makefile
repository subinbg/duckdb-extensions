include build.env

PYTHON ?= uv run

BUILD = $(PYTHON) scripts/build.py --duckdb-version $(DUCKDB_VERSION) \
  --core-directory $(CORE_DIRECTORY) --custom-directory $(CUSTOM_DIRECTORY) \
  $(CORE_EXTENSIONS:%=--core-extensions %) $(CUSTOM_EXTENSIONS:%=--custom-extensions %)

# DuckDB vMM.mm.pp as the number MMmmpp, which duckdb-rs puts in its releases: 1.20000.x is for v2.0.0.
number = $(shell echo $(1) | awk -F'[v.]' '{ print $$2 * 10000 + $$3 * 100 + $$4 }')
capi_version = $(shell sed -n 's/^capi-version = "\(.*\)"/\1/p' extensions/$(1)/Cargo.toml)

# Cf. Emulated linux/amd64 on Apple Silicon fails DuckDB's signature checks even for official downloads.
ARCH ?= $(if $(filter x86_64 amd64,$(shell uname -m)),amd64,arm64)
DOCKER_BUILD := docker build --platform linux/$(ARCH)
TEST_IMAGE := duckdb-extensions-test:$(ARCH)

.PHONY: help versions build dist test docker-test docker-dist check clean

help:
	@echo "make versions     check that the versions named across the repository fit build.env"
	@echo "make build        download the official extensions and compile ours, as build.env says"
	@echo "make dist         compile ours, and write the release tarball of them"
	@echo "make test         run pytest on what was built, and on the release tarball if there is one"
	@echo "make docker-test  build in containers and run the tests offline (--network none)"
	@echo "make docker-dist  build in containers and write the release tarball"
	@echo "make check        ruff lint + format check, pyrefly strict type check"
	@echo "ARCH=amd64|arm64 overrides the architecture of the containers (default: $(ARCH))"

versions: $(CUSTOM_EXTENSIONS:%=versions-%)
	grep -q '"duckdb==$(DUCKDB_VERSION:v%=%)"' pyproject.toml
	grep -q '^duckdb = .*"=1\.$(call number,$(DUCKDB_VERSION))\.' Cargo.toml
	grep -q '^libduckdb-sys = .*"=1\.$(call number,$(DUCKDB_VERSION))\.' Cargo.toml
	grep -q '^FROM python:$(shell cat .python-version)-' Dockerfile

# The C API version in an extension's footer is the one its entry point asks for, and DuckDB provides it.
versions-%:
	grep -q '"$(call capi_version,$*)"' extensions/$*/src/lib.rs
	test $(call number,$(call capi_version,$*)) -le $(call number,$(DUCKDB_VERSION))

build: versions
	$(BUILD)

# The release tarball holds our extensions only, so DuckDB's are not downloaded for it.
dist: CORE_EXTENSIONS =
dist: versions
	$(BUILD) --dist-directory $(DIST_DIRECTORY)

test:
	CORE_DIRECTORY=$(CORE_DIRECTORY) CUSTOM_DIRECTORY=$(CUSTOM_DIRECTORY) DIST_DIRECTORY=$(DIST_DIRECTORY) uv run pytest

docker-test:
	$(DOCKER_BUILD) --target test -t $(TEST_IMAGE) .
	docker run --rm --network none $(TEST_IMAGE)

docker-dist:
	$(DOCKER_BUILD) --target dist --output type=local,dest=$(DIST_DIRECTORY) .
	@ls -l $(DIST_DIRECTORY)

check: versions
	uv lock --check
	uv run ruff check
	uv run ruff format --check
	uv run pyrefly check

clean:
	rm -rf build dist target
