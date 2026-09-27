FROM debian:13-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends openssh-server \
    && rm -rf /var/lib/apt/lists/* \
    && ssh-keygen -A

COPY entrypoint.sh /usr/local/bin/yshell-sshd-entrypoint
RUN chmod 755 /usr/local/bin/yshell-sshd-entrypoint

EXPOSE 22
ENTRYPOINT ["/usr/local/bin/yshell-sshd-entrypoint"]
