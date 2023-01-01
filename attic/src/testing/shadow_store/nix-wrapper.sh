#!/usr/bin/env bash
export NIX_CONF_DIR="%store_root%/etc/nix"
export NIX_USER_CONF_FILE=""
export NIX_REMOTE=""

exec %command% --store "%store_root%" "$@"
