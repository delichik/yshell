#!/usr/bin/env bash
# Minimal sshd entrypoint for the YShell live SSH/SFTP tests.
set -euo pipefail

authorized_keys_source="${YSHELL_TEST_AUTHORIZED_KEYS:-/tmp/authorized_keys}"
if [ ! -f "$authorized_keys_source" ]; then
  echo "yshell-test-ssh: missing public key at $authorized_keys_source" >&2
  exit 1
fi

# 密码认证测试账号（E2E 密码弹窗 + SFTP 上传/下载都用它；root 仍保持密钥专用）。
test_user="${YSHELL_TEST_SSH_USER:-tester}"
test_password="${YSHELL_TEST_SSH_PASSWORD:-yshell-test-pass}"
if [ -z "$test_user" ] || [ -z "$test_password" ]; then
  echo "yshell-test-ssh: YSHELL_TEST_SSH_USER / YSHELL_TEST_SSH_PASSWORD must not be empty" >&2
  exit 1
fi
if [[ "$test_user" == *:* ]] || [[ "$test_password" == *:* ]]; then
  # chpasswd 的 user:password 行以冒号分隔，密码里带冒号会被截断。
  echo "yshell-test-ssh: test user/password must not contain ':'" >&2
  exit 1
fi

mkdir -p /root/.ssh
install -m 600 "$authorized_keys_source" /root/.ssh/authorized_keys
chmod 700 /root/.ssh

# 非 root 测试账号：可写 home（SFTP 上传/下载目标），同时写入同一把测试公钥
# （方便脚本用 ssh -i 做连通性检查；密码认证才是 E2E 的主角）。
if ! id -u "$test_user" >/dev/null 2>&1; then
  useradd -m -s /bin/bash "$test_user"
fi
test_home=$(getent passwd "$test_user" | cut -d: -f6)
mkdir -p "${test_home}/.ssh"
install -m 600 "$authorized_keys_source" "${test_home}/.ssh/authorized_keys"
chmod 700 "${test_home}/.ssh"
chown -R "${test_user}:${test_user}" "${test_home}/.ssh"
echo "${test_user}:${test_password}" | chpasswd

cat > /etc/ssh/sshd_config.d/99-yshell-test.conf <<'EOF'
PermitRootLogin prohibit-password
PubkeyAuthentication yes
PasswordAuthentication yes
KbdInteractiveAuthentication yes
UsePAM yes
Subsystem sftp internal-sftp
EOF

mkdir -p /run/sshd
exec /usr/sbin/sshd -D -e
