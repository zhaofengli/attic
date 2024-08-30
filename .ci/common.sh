# Use as:
#
# source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

set -euo pipefail
base="$(readlink -f $(dirname "${BASH_SOURCE[0]}")/..)"
cached_shell="${base}/.ci/cached-shell"
