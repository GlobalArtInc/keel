agent_pid=""
agent_socket=""
agent_log=""

start_agent() {
  local keel=$1 dir=$2
  local login uid gid
  login=$(id -un)
  uid=$(id -u)
  gid=$(id -g)
  agent_socket="$dir/agent.sock"
  agent_log="$dir/agent.log"
  mkdir -p "$dir/bin" "$dir/notes" "$dir/templates" "$dir/cron.d"
  cat >"$dir/bin/getent" <<EOF
#!/bin/sh
if [ "\$1" = passwd ] && [ "\$3" = "$login" ]; then
  echo "$login:x:$uid:$gid::$dir:/bin/sh"
  exit 0
fi
exit 2
EOF
  cat >"$dir/bin/systemctl" <<EOF
#!/bin/sh
echo "\$@" >>"$dir/systemctl.calls"
if [ "\$1" = is-active ]; then
  echo active
fi
exit 0
EOF
  chmod 0755 "$dir/bin/getent" "$dir/bin/systemctl"
  cat >"$dir/agent.yaml" <<EOF
policy:
  file_roots: { notes: $dir/notes }
  services: []
  products:
    demo: { user_prefix: dm_, drop_in_prefix: keel-demo-, classes: { note: { root: notes } } }
  tools: { getent: $dir/bin/getent, systemctl: $dir/bin/systemctl, cron_dir: $dir/cron.d }
EOF
  "$keel" privileged --socket "$agent_socket" --policy "$dir/agent.yaml" --user "$login" --templates "$dir/templates" >"$agent_log" 2>&1 &
  agent_pid=$!
  local _
  for _ in $(seq 1 300); do
    if [ -S "$agent_socket" ]; then
      echo "keel privileged ($agent_pid) serves $agent_socket as $login"
      return 0
    fi
    if ! kill -0 "$agent_pid" 2>/dev/null; then
      tail -n 20 "$agent_log" >&2
      agent_pid=""
      return 1
    fi
    sleep 0.1
  done
  tail -n 20 "$agent_log" >&2
  return 1
}

stop_agent() {
  if [ -z "$agent_pid" ]; then
    return 0
  fi
  local pid=$agent_pid status=0
  agent_pid=""
  kill -TERM "$pid" 2>/dev/null || true
  wait "$pid" 2>/dev/null || status=$?
  echo "stopped keel privileged ($pid)"
  return "$status"
}

agent_call() {
  python3 - "$agent_socket" "$1" "$2" <<'EOF'
import json, socket, sys
request = {
    "id": 1,
    "op": sys.argv[2],
    "actor": {"login": "script", "level": "owner"},
    "request_id": "script-1",
    "product": "demo",
    "args": json.loads(sys.argv[3]),
}
client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
client.connect(sys.argv[1])
client.sendall((json.dumps(request) + "\n").encode())
print(client.makefile("r").readline().rstrip("\n"))
EOF
}
