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
