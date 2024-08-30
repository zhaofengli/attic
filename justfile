set positional-arguments

here := env_var_or_default("JUST_INVOCATION_DIR", invocation_directory())
base := `pwd`

#@echo "here: {{ here }}"
#@echo "base: {{ base }}"

# List available targets
list:
	@just --list --unsorted

# Run a command with an alternative Nix version
with-nix version *command:
	set -e; \
		hook="$(jq -e -r '.[$version].shellHook' --arg version "{{ version }}" < "$NIX_VERSIONS" || (>&2 echo "Version {{ version }} doesn't exist"; exit 1))"; \
		eval "$hook"; \
		CARGO_TARGET_DIR="{{ base }}/target/nix-{{ version }}" \
		{{ command }}

# (CI) Build WebAssembly crates
ci-build-wasm:
	#!/usr/bin/env bash
	set -euxo pipefail

	# https://github.com/rust-lang/rust/issues/122357
	export RUST_MIN_STACK=16777216

	pushd attic
	cargo build --target wasm32-unknown-unknown --no-default-features -F chunking -F stream
	popd
	pushd token
	cargo build --target wasm32-unknown-unknown
	popd

# (CI) Run unit tests
ci-unit-tests matrix:
	#!/usr/bin/env bash
	set -euxo pipefail

	system=$(nix-instantiate --eval -E 'builtins.currentSystem')
	tests=$(nix build .#internalMatrix."$system".\"{{ matrix }}\".attic-tests --no-link --print-out-paths -L)
	find "$tests/bin" -exec {} \;
