# Makefile — convenience targets for ps-payload-rust.
#
# Primary purpose: generate the L1 FFI bindings (crates/ps5-sys) from the SDK.
# The bindings are produced by crates/ps5-sys/build.rs (bindgen) into OUT_DIR;
# `make generate` forces that build script to re-run. See CLAUDE.md.

SHELL := /bin/bash

CARGO     ?= cargo
SYS_CRATE := ps5-sys
SYS_DIR   := crates/ps5-sys
SDK_HDR   := sdk/include/ps5/payload.h

# --- build profile ---------------------------------------------------------
# Default is debug. Pass RELEASE=1 (e.g. `make generate RELEASE=1`) for release,
# or use the *-release aliases. PROFILE_DIR is where cargo drops the artifacts.
RELEASE      ?=
PROFILE_FLAG := $(if $(RELEASE),--release,)
PROFILE_DIR  := $(if $(RELEASE),release,debug)

# --- libclang discovery ----------------------------------------------------
# bindgen needs libclang. Honor a caller-provided LIBCLANG_PATH; otherwise probe
# the usual macOS (Xcode/CLT, Homebrew) and Linux locations. Exported to recipes;
# if nothing is found it stays empty and bindgen falls back to its own search.
ifndef LIBCLANG_PATH
LIBCLANG_DIRS := \
	$(shell xcode-select -p 2>/dev/null)/Toolchains/XcodeDefault.xctoolchain/usr/lib \
	/Library/Developer/CommandLineTools/usr/lib \
	/opt/homebrew/opt/llvm/lib \
	/opt/homebrew/opt/llvm@18/lib \
	/usr/local/opt/llvm/lib \
	/usr/lib/llvm-18/lib \
	/usr/lib/llvm-17/lib \
	/usr/lib
LIBCLANG_PATH := $(firstword $(foreach d,$(LIBCLANG_DIRS),\
	$(if $(strip $(wildcard $(d)/libclang.dylib) $(wildcard $(d)/libclang.so*)),$(d))))
endif
export LIBCLANG_PATH

.DEFAULT_GOAL := help

# --- core targets ----------------------------------------------------------

.PHONY: generate
generate: $(SDK_HDR) ## Generate L1 FFI bindings from the SDK (RELEASE=1 for release)
	@echo ">> generating $(SYS_CRATE) bindings [$(PROFILE_DIR)] (LIBCLANG_PATH=$(LIBCLANG_PATH))"
	touch $(SYS_DIR)/build.rs   # forces the build script (and all bindgen runs) to re-run
	$(CARGO) build -p $(SYS_CRATE) $(PROFILE_FLAG)
	@$(MAKE) --no-print-directory show-bindings RELEASE=$(RELEASE)

.PHONY: generate-release
generate-release: ## Generate L1 FFI bindings using the release profile
	@$(MAKE) --no-print-directory generate RELEASE=1

.PHONY: show-bindings
show-bindings: ## Print path + per-module counts for the generated bindings
	@d=$$(find target/$(PROFILE_DIR) -path '*$(SYS_CRATE)*/out/ps5_bindings.rs' 2>/dev/null | head -1); \
	if [ -z "$$d" ]; then d=$$(find target -path '*$(SYS_CRATE)*/out/ps5_bindings.rs' 2>/dev/null | head -1); fi; \
	if [ -n "$$d" ]; then d=$$(dirname "$$d"); \
		echo "bindings: $$d"; \
		for f in "$$d"/*_bindings.rs; do \
			printf '  %-20s %4s fns, %4s consts, %3s structs\n' "$$(basename "$$f")" \
				"$$(grep -c 'pub fn ' "$$f")" "$$(grep -c 'pub const ' "$$f")" "$$(grep -c 'pub struct ' "$$f")"; \
		done; \
	else \
		echo "no bindings found — run 'make generate'"; \
	fi

.PHONY: print-bindings
print-bindings: ## Dump all generated *_bindings.rs to stdout
	@d=$$(find target/$(PROFILE_DIR) -path '*$(SYS_CRATE)*/out/ps5_bindings.rs' 2>/dev/null | head -1); \
	if [ -z "$$d" ]; then d=$$(find target -path '*$(SYS_CRATE)*/out/ps5_bindings.rs' 2>/dev/null | head -1); fi; \
	if [ -n "$$d" ]; then for f in $$(dirname "$$d")/*_bindings.rs; do echo "// ===== $$f ====="; cat "$$f"; done; \
	else echo "no bindings — run 'make generate'" >&2; exit 1; fi

# --- supporting targets ----------------------------------------------------

$(SDK_HDR):
	@echo ">> SDK headers missing; initializing the submodule"
	git submodule update --init sdk

.PHONY: submodule
submodule: ## Initialize/update the sdk submodule
	git submodule update --init sdk

.PHONY: build
build: ## Build the whole workspace (RELEASE=1 for release)
	$(CARGO) build $(PROFILE_FLAG)

.PHONY: check
check: ## cargo check the workspace (RELEASE=1 for release)
	$(CARGO) check $(PROFILE_FLAG)

.PHONY: clippy
clippy: ## Lint with clippy (RELEASE=1 for release)
	$(CARGO) clippy --all-targets $(PROFILE_FLAG)

.PHONY: fmt
fmt: ## Format the workspace
	$(CARGO) fmt

.PHONY: fmt-check
fmt-check: ## Verify formatting (CI)
	$(CARGO) fmt --check

.PHONY: clean-sys
clean-sys: ## Remove L1 artifacts so the next generate runs from scratch
	$(CARGO) clean -p $(SYS_CRATE)

.PHONY: clean
clean: ## Remove all build artifacts
	$(CARGO) clean

.PHONY: help
help: ## Show this help
	@awk 'BEGIN {FS = ":.*## "; printf "Targets:\n"} \
		/^[a-zA-Z0-9_-]+:.*## / {printf "  \033[36m%-16s\033[0m %s\n", $$1, $$2}' \
		$(MAKEFILE_LIST)
