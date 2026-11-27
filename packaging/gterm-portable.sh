#!/usr/bin/env sh
# gTerm portable launcher.
# Keeps configuration, known hosts, logs and the local secret store in
# ./data next to this script instead of the user config directory.
# Configuration is passed as a CLI argument (`--config-dir`), not an
# environment variable: application configuration never comes from the env.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$HERE/gterm" --config-dir "$HERE/data" "$@"
