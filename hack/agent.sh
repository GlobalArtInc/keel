agent_pid=""
agent_socket=""
agent_log=""

start_agent() {
  local keel=$1 dir=$2 range=${3:-}
  local login uid gid range_line=""
  case "$keel" in /*) ;; *) keel="$PWD/$keel" ;; esac
  login=$(id -un)
  uid=$(id -u)
  gid=$(id -g)
  agent_socket="$dir/agent.sock"
  agent_log="$dir/agent.log"
  mkdir -p "$dir/bin" "$dir/notes" "$dir/zones" "$dir/named" "$dir/templates" "$dir/cron.d" "$dir/www" "$dir/transfer" "$dir/vhosts"
  : >"$dir/passwd"
  : >"$dir/group"
  printf '%s\n' /bin/sh /bin/bash /usr/sbin/nologin >"$dir/shells"
  cat >"$dir/bin/getent" <<EOF
#!/bin/sh
if [ "\$1" = passwd ] && [ "\$3" = "$login" ]; then
  echo "$login:x:$uid:$gid::$dir:/bin/sh"
  exit 0
fi
EOF
  cat >>"$dir/bin/getent" <<'EOF'
case "$1" in passwd) file=@DIR@/passwd ;; group) file=@DIR@/group ;; *) exit 1 ;; esac
if [ $# -lt 3 ]; then
  cat "$file"
  exit 0
fi
line=$(awk -F: -v k="$3" '$1 == k || $3 == k' "$file")
[ -n "$line" ] || exit 2
printf '%s\n' "$line"
EOF
  cat >"$dir/bin/useradd" <<'EOF'
#!/bin/sh
uid=; gid=; shell=; home=; create=
while [ $# -gt 1 ]; do
  case "$1" in
    --uid) uid=$2; shift 2 ;;
    --gid) gid=$2; shift 2 ;;
    --shell) shell=$2; shift 2 ;;
    --home-dir) home=$2; shift 2 ;;
    --create-home) create=yes; shift ;;
    *) shift ;;
  esac
done
if [ -n "$create" ]; then mkdir -p "$home"; fi
printf '%s:x:%s:%s::%s:%s\n' "$1" "$uid" "$gid" "$home" "$shell" >>@DIR@/passwd
EOF
  cat >"$dir/bin/userdel" <<'EOF'
#!/bin/sh
for last; do :; done
home=$(awk -F: -v k="$last" '$1 == k {print $6}' @DIR@/passwd)
if [ "$1" = --remove ] && [ -n "$home" ]; then rm -rf "$home"; fi
awk -F: -v k="$last" '$1 != k' @DIR@/passwd >@DIR@/passwd.new && mv @DIR@/passwd.new @DIR@/passwd
EOF
  cat >"$dir/bin/usermod" <<'EOF'
#!/bin/sh
shell=
while [ $# -gt 1 ]; do
  case "$1" in
    --shell) shell=$2; shift 2 ;;
    --expiredate) shift 2 ;;
    *) shift ;;
  esac
done
if [ -n "$shell" ]; then
  awk -F: -v OFS=: -v k="$1" -v s="$shell" '$1 == k { $7 = s } { print }' @DIR@/passwd >@DIR@/passwd.new && mv @DIR@/passwd.new @DIR@/passwd
fi
EOF
  cat >"$dir/bin/groupadd" <<'EOF'
#!/bin/sh
printf '%s:x:%s:\n' "$3" "$2" >>@DIR@/group
EOF
  cat >"$dir/bin/groupdel" <<'EOF'
#!/bin/sh
awk -F: -v k="$1" '$1 != k' @DIR@/group >@DIR@/group.new && mv @DIR@/group.new @DIR@/group
EOF
  cat >"$dir/bin/keel" <<'EOF'
#!/bin/sh
echo "$*" >>@DIR@/keel.calls
[ "$1" = agent-exec ] && [ "$2" = --uid ] || exit 126
shift 3
if [ "$1" = --policy ]; then shift 2; fi
shift
exec @KEEL@ "$@"
EOF
  local stub body
  for stub in getent useradd userdel usermod groupadd groupdel keel; do
    body=$(cat "$dir/bin/$stub")
    body=${body//@DIR@/\'$dir\'}
    body=${body//@KEEL@/\'$keel\'}
    printf '%s\n' "$body" >"$dir/bin/$stub"
  done
  cat >"$dir/bin/systemctl" <<EOF
#!/bin/sh
echo "\$@" >>"$dir/systemctl.calls"
if [ "\$1" = is-active ]; then
  echo active
fi
exit 0
EOF
  cat >"$dir/bin/named-checkconf" <<EOF
#!/bin/sh
echo "\$@" >>"$dir/named-checkconf.calls"
exit 0
EOF
  cat >"$dir/bin/nginx" <<EOF
#!/bin/sh
echo "\$@" >>"$dir/nginx.calls"
if grep -qs keel_e2e_refused "$dir"/vhosts/*.conf; then
  echo 'nginx: [emerg] unknown directive "keel_e2e_refused"' >&2
  exit 1
fi
exit 0
EOF
  chmod 0755 "$dir/bin/nginx" "$dir/bin/getent" "$dir/bin/systemctl" "$dir/bin/named-checkconf" "$dir/bin/useradd" "$dir/bin/userdel" "$dir/bin/usermod" "$dir/bin/groupadd" "$dir/bin/groupdel" "$dir/bin/keel"
  if [ -n "$range" ]; then
    range_line="  uid_range: $range"
  fi
  cat >"$dir/agent.yaml" <<EOF
policy:
$range_line
  file_roots: { notes: $dir/notes, zones: $dir/zones, named: $dir/named, www: $dir/www, transfer: $dir/transfer, vhosts: $dir/vhosts }
  services: [named, nginx]
  checks: { named.check: ["$dir/bin/named-checkconf", "-z"], nginx.test: ["$dir/bin/nginx", "-t"] }
  products:
    demo: { user_prefix: dm_, drop_in_prefix: keel-demo-, classes: { note: { root: notes } } }
    dns: { drop_in_prefix: keel-dns- }
    panel: { user_prefix: hp_, drop_in_prefix: keel-panel- }
  tools: { getent: $dir/bin/getent, systemctl: $dir/bin/systemctl, cron_dir: $dir/cron.d, useradd: $dir/bin/useradd, userdel: $dir/bin/userdel, usermod: $dir/bin/usermod, groupadd: $dir/bin/groupadd, groupdel: $dir/bin/groupdel, shells: $dir/shells, keel: $dir/bin/keel }
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
