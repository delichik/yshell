#!/usr/bin/env sh
# YShell portable launcher.
# Keeps configuration, known hosts, logs and the local secret store in
# ./data next to this script instead of the user config directory.
# Configuration is passed as a CLI argument (`--config-dir`), not an
# environment variable: application configuration never comes from the env.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
exec "$HERE/yshell" --config-dir "$HERE/data" "$@"
