# The Docker system: these targets run the commands of README.md "Build and test" in containers.
# Any other system with cargo and Python runs those commands directly.

# Cf. Emulated linux/amd64 on Apple Silicon fails DuckDB's signature checks even for official downloads.
ARCH ?= $(if $(filter x86_64 amd64,$(shell uname -m)),amd64,arm64)

DOCKER_BUILD := docker build --platform linux/$(ARCH)
TEST_IMAGE := duckdb-extensions-test:$(ARCH)

.PHONY: help test dist check clean

help:
	@echo "make test   build everything in containers and run the tests offline (--network none)"
	@echo "make dist   write the release tarball to dist/"
	@echo "make check  ruff lint + format check, pyrefly strict type check"
	@echo "ARCH=amd64|arm64 overrides the target architecture (default: $(ARCH))"

test:
	$(DOCKER_BUILD) --target test -t $(TEST_IMAGE) .
	docker run --rm --network none $(TEST_IMAGE)

dist:
	$(DOCKER_BUILD) --target dist --output type=local,dest=dist .
	@ls -l dist

check:
	uv lock --check
	uv run ruff check
	uv run ruff format --check
	uv run pyrefly check

clean:
	rm -rf build dist target
