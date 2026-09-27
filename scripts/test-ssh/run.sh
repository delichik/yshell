#!/usr/bin/env bash
# Manage the YShell SSH/SFTP test container used by the live smoke tests.
#
# The live tests authenticate with ssh-agent (`AuthMethod::Agent`), so this
# script generates a throwaway key, bakes it into the container's root account
# and starts a local agent when running tests.
set -euo pipefail

if [ -f "$HOME/.cargo/env" ]; then
  # shellcheck disable=SC1091
  . "$HOME/.cargo/env"
fi

script_dir=$(cd "$(dirname "$0")" && pwd)

image="${YSHELL_TEST_SSH_IMAGE:-yshell-test-ssh}"
container="${YSHELL_TEST_SSH_CONTAINER:-yshell-test-ssh}"
port="${YSHELL_TEST_SSH_PORT:-2222}"
key="${YSHELL_TEST_SSH_KEY:-$HOME/.ssh/yshell_test_ed25519}"
test_user="${YSHELL_TEST_SSH_USER:-tester}"
test_password="${YSHELL_TEST_SSH_PASSWORD:-yshell-test-pass}"

usage() {
  cat <<'EOF'
Usage: scripts/test-ssh/run.sh <command>

Commands:
  up       Build the test image and start the container on 127.0.0.1:2222.
  down     Stop and remove the test container.
  status   Show the container status and connection details.
  env      Print the environment variables used by the live smoke tests.
  live     Start ssh-agent and run the live SSH/SFTP smoke tests (container
           must already be running; see `up`).
  test     `up` followed by `live`.
  shell    Open an interactive shell in the test container.

Environment overrides:
  YSHELL_TEST_SSH_PORT      host port (default 2222)
  YSHELL_TEST_SSH_KEY       key path (default ~/.ssh/yshell_test_ed25519)
  YSHELL_TEST_SSH_CONTAINER container name (default yshell-test-ssh)
  YSHELL_TEST_SSH_USER      password-auth user (default tester)
  YSHELL_TEST_SSH_PASSWORD  password-auth password (default yshell-test-pass)
  CARGO_TARGET_DIR          forwarded to cargo if set
EOF
}

ensure_key() {
  if [ ! -f "$key" ]; then
    mkdir -p "$(dirname "$key")"
    ssh-keygen -t ed25519 -N '' -f "$key" -C yshell-test >/dev/null
    echo "generated test key: $key"
  fi
}

container_running() {
  [ "$(docker inspect -f '{{.State.Running}}' "$container" 2>/dev/null || echo false)" = "true" ]
}

cmd_up() {
  ensure_key
  if container_running; then
    echo "container already running: $container"
    return 0
  fi
  echo "building $image ..."
  docker build -t "$image" "$script_dir"
  docker rm -f "$container" >/dev/null 2>&1 || true
  docker run -d --name "$container" \
    -p "127.0.0.1:${port}:22" \
    -e "YSHELL_TEST_SSH_USER=${test_user}" \
    -e "YSHELL_TEST_SSH_PASSWORD=${test_password}" \
    -v "${key}.pub:/tmp/authorized_keys:ro" \
    "$image" >/dev/null
  for _ in $(seq 1 40); do
    if ssh -i "$key" -o StrictHostKeyChecking=no -o UserKnownHostsFile=/dev/null \
        -o BatchMode=yes -o ConnectTimeout=2 -p "$port" root@127.0.0.1 true >/dev/null 2>&1; then
      echo "yshell test ssh ready: root@127.0.0.1:${port} (key: $key)"
      return 0
    fi
    sleep 0.25
  done
  echo "container did not become ready; logs:" >&2
  docker logs "$container" >&2 || true
  exit 1
}

cmd_down() {
  if docker rm -f "$container" >/dev/null 2>&1; then
    echo "removed container: $container"
  else
    echo "container not present: $container"
  fi
}

cmd_status() {
  docker ps -a --filter "name=^/${container}\$" \
    --format 'table {{.Names}}\t{{.Image}}\t{{.Status}}\t{{.Ports}}'
  echo "target: root@127.0.0.1:${port} (key: ${key}.pub, root=prohibit-password)"
  echo "pass  : ${test_user}@127.0.0.1:${port} (password: ${test_password})"
}

cmd_env() {
  echo "export YSHELL_LIVE_SSH_TARGET=root@127.0.0.1:${port}"
  echo "export YSHELL_LIVE_SFTP_TARGET=root@127.0.0.1:${port}"
  echo "export YSHELL_TEST_SSH_PASSWORD=${test_password}"
  echo "# live tests use AuthMethod::Agent; run inside an ssh-agent session:"
  echo "#   eval \$(ssh-agent -s) && ssh-add ${key}"
  echo "# password-auth E2E target: ${test_user}@127.0.0.1:${port} (password from YSHELL_TEST_SSH_PASSWORD)"
}

start_agent() {
  ensure_key
  if [ -n "${SSH_AUTH_SOCK:-}" ] && ssh-add -l >/dev/null 2>&1; then
    echo "reusing existing ssh-agent"
  else
    eval "$(ssh-agent -s)" >/dev/null
    echo "started ssh-agent (pid ${SSH_AGENT_PID})"
  fi
  if ! ssh-add -l 2>/dev/null | grep -q "yshell-test"; then
    ssh-add "$key" >/dev/null 2>&1
  fi
}

run_live_tests() {
  start_agent
  export YSHELL_LIVE_SSH_TARGET="root@127.0.0.1:${port}"
  export YSHELL_LIVE_SFTP_TARGET="root@127.0.0.1:${port}"
  cd "$script_dir/../.."
  # The test selection lives in the build tool so CI and local runs match.
  cargo xtask test --live
}

case "${1:-}" in
  up) cmd_up ;;
  down) cmd_down ;;
  status) cmd_status ;;
  env) cmd_env ;;
  live) run_live_tests ;;
  test)
    cmd_up
    run_live_tests
    ;;
  shell) docker exec -it "$container" bash ;;
  *)
    usage
    exit 2
    ;;
esac
