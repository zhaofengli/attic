#!/usr/bin/env bash
source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

>&2 echo "Caching dev shell"
nix print-dev-env "${base}#" >"${cached_shell}"
