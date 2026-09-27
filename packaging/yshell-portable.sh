#!/usr/bin/env sh
# YShell portable launcher.
# Keeps configuration, known hosts, logs and the local secret store in
# ./data next to this script instead of the user config directory.
set -eu

HERE=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
YSHELL_CONFIG_DIR="$HERE/data"
export YSHELL_CONFIG_DIR
exec "$HERE/yshell" "$@"
