#!/usr/bin/env bash
# An unprivileged sshd on 127.0.0.1:2222 with a throwaway host key and one test key.
set -euo pipefail

root=$(cd "$(dirname "$0")/.." && pwd)
local="$root/.local"
config="$local/sshd_config"
pidfile="$local/sshd.pid"

stop() {
    [ -f "$pidfile" ] && kill "$(cat "$pidfile")" 2>/dev/null || true
    rm -f "$pidfile"
}

case "${1:-start}" in
stop)
    stop
    echo "stopped"
    exit 0
    ;;
start) ;;
*)
    echo "usage: $0 [start|stop]" >&2
    exit 2
    ;;
esac

if [ -f "$pidfile" ] && kill -0 "$(cat "$pidfile")" 2>/dev/null; then
    echo "already listening on 2222 (pid $(cat "$pidfile"))"
    exit 0
fi

mkdir -p "$local"
[ -f "$local/ssh_host_ed25519_key" ] ||
    ssh-keygen -q -t ed25519 -N '' -C sesh-local-host -f "$local/ssh_host_ed25519_key"
[ -f "$local/testkey" ] || ssh-keygen -q -t ed25519 -N '' -C sesh-test -f "$local/testkey"
chmod 600 "$local/ssh_host_ed25519_key" "$local/testkey"

cat > "$config" <<CONF
Port 2222
ListenAddress 127.0.0.1
ListenAddress ::1
HostKey $local/ssh_host_ed25519_key
PidFile $pidfile
AuthorizedKeysFile $local/testkey.pub
AllowUsers $USER
PasswordAuthentication no
KbdInteractiveAuthentication no
UsePAM no
StrictModes no
PrintMotd no
LogLevel VERBOSE
CONF

/usr/sbin/sshd -D -e -f "$config" > "$local/sshd.log" 2>&1 &
echo $! > "$pidfile"
sleep 1
kill -0 "$(cat "$pidfile")" 2>/dev/null || {
    echo "sshd died:" >&2
    cat "$local/sshd.log" >&2
    rm -f "$pidfile"
    exit 1
}
echo "listening on 127.0.0.1:2222 as $USER with $local/testkey"
