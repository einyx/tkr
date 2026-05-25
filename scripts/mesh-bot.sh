#!/usr/bin/env bash
# scripts/mesh-bot.sh — populate the live mesh stats with synthetic peers.
#
# Mints a "demo" mesh invite, creates N peers (default 3), has each one
# join the mesh and tail (keeping a WSS connection open), and starts a
# heartbeat loop where peers DM each other on a schedule.
#
# All state lives in /tmp/jkr-mesh-bot/ so it can be wiped cleanly:
#   ./scripts/mesh-bot.sh stop       # kill everything
#   ./scripts/mesh-bot.sh start      # spawn N peers (default 3)
#   ./scripts/mesh-bot.sh status     # check what's running
#
# Bots persist as background processes (nohup). To survive a reboot, wrap
# this in a systemd unit or compose service.

set -uo pipefail

cd "$(dirname "$0")/.."
JKR=${JKR:-$(command -v jkr || echo "$HOME/.cargo/bin/jkr")}
[ -x "$JKR" ] || { echo "error: jkr not found at $JKR (run: cargo install --path crates/jkr --locked)" >&2; exit 1; }

WORK=/tmp/jkr-mesh-bot
PIDFILE=$WORK/pids
LOGDIR=$WORK/logs
OWNERFILE=$WORK/owner.env
INVITEFILE=$WORK/invite.url
PEERS=${PEERS:-3}
SLUG=${JKR_MESH_SLUG:-demo}

# LOCAL=1 ./scripts/mesh-bot.sh start  → point everything at the local
# jkr-server (http://127.0.0.1:4000). Useful for self-contained demos and
# for iterating without affecting the public broker.
if [ "${LOCAL:-0}" = "1" ]; then
  : "${JKR_MESH_BROKER:=ws://127.0.0.1:4000/api/v1/mesh/ws}"
  : "${JKR_MESH_LOGIN_HOST:=http://127.0.0.1:4000}"
fi
BROKER=${JKR_MESH_BROKER:-wss://tkr.prysm.sh/api/v1/mesh/ws}

cmd_status() {
  if [ ! -f "$PIDFILE" ]; then
    echo "no bots running (no $PIDFILE)"
    return 0
  fi
  echo "tracked pids:"
  while read -r pid; do
    if kill -0 "$pid" 2>/dev/null; then
      echo "  $pid  RUNNING  $(ps -p "$pid" -o args= 2>/dev/null | head -c 100)"
    else
      echo "  $pid  DEAD"
    fi
  done < "$PIDFILE"
  echo
  echo "ls $LOGDIR/"
  ls -la "$LOGDIR" 2>/dev/null || echo "  (no logs yet)"
}

cmd_stop() {
  if [ -f "$PIDFILE" ]; then
    while read -r pid; do
      kill "$pid" 2>/dev/null || true
    done < "$PIDFILE"
    sleep 1
    while read -r pid; do
      kill -9 "$pid" 2>/dev/null || true
    done < "$PIDFILE"
    rm -f "$PIDFILE"
    echo "stopped"
  else
    echo "(no $PIDFILE to read)"
  fi
}

cmd_start() {
  mkdir -p "$WORK" "$LOGDIR"
  : > "$PIDFILE"

  # 0. authenticate. jkr-server now gates POST /join and GET /ws behind
  # an authenticated session; jkr-mesh's enrollment + Client::connect
  # both honor JKR_MESH_WS_COOKIE which is sent as `jkr_session=<v>`.
  # Login once, extract the cookie value, export for subsequent calls.
  if [ -z "${JKR_MESH_WS_COOKIE:-}" ]; then
    PW=""
    if [ -n "${JKR_ADMIN_PASSWORD:-}" ]; then
      PW="$JKR_ADMIN_PASSWORD"
    elif [ -f jkr-server.env ]; then
      PW=$(grep '^JKR_ADMIN_PASSWORD=' jkr-server.env | cut -d= -f2- || true)
    fi
    if [ -z "$PW" ]; then
      echo "error: no password — set JKR_ADMIN_PASSWORD or place it in jkr-server.env" >&2
      exit 1
    fi
    LOGIN_HOST=${JKR_MESH_LOGIN_HOST:-https://tkr.prysm.sh}
    JAR=$(mktemp); trap "rm -f $JAR" EXIT
    HTTP=$(curl -sS -o /dev/null -w '%{http_code}' -c "$JAR" \
      -X POST "$LOGIN_HOST/api/auth/login" \
      -H 'content-type: application/json' \
      -d "{\"password\":\"$PW\"}")
    if [ "$HTTP" != "200" ]; then
      echo "error: login to $LOGIN_HOST returned HTTP $HTTP — wrong password?" >&2
      exit 1
    fi
    JKR_MESH_WS_COOKIE=$(awk '/jkr_session/ {print $7}' "$JAR" | head -1)
    if [ -z "$JKR_MESH_WS_COOKIE" ]; then
      echo "error: login succeeded but no jkr_session cookie in jar" >&2
      exit 1
    fi
    export JKR_MESH_WS_COOKIE
    echo "authenticated against $LOGIN_HOST"
  fi

  # 1. owner key (re-used across runs unless wiped).
  if [ ! -f "$OWNERFILE" ]; then
    echo "minting owner key..."
    OWN_PRIV=$("$HOME/.foundry/bin/cast" wallet new --json | grep -oE '0x[0-9a-fA-F]{64}' | head -1)
    [ -n "$OWN_PRIV" ] || { echo "error: cast wallet new failed (foundry installed?)" >&2; exit 1; }
    echo "JKR_PAYMENT_KEY=$OWN_PRIV" > "$OWNERFILE"
    chmod 0600 "$OWNERFILE"
  fi

  # 2. mint a fresh 24h invite each start.
  echo "minting 24h invite..."
  "$JKR" mesh invite-mint \
      --slug "$SLUG" \
      --broker-url "$BROKER" \
      --owner-key-file "$OWNERFILE" \
      --ttl-hours 24 2>/dev/null \
      | grep -oE '^https://[^ ]+' | head -1 > "$INVITEFILE"
  INVITE=$(cat "$INVITEFILE")
  if [ -z "$INVITE" ]; then
    echo "error: invite-mint produced no URL" >&2
    exit 1
  fi

  # 3. for each peer: separate $HOME so jkr's mesh state doesn't collide.
  for i in $(seq 1 "$PEERS"); do
    pdir=$WORK/peer-$i
    mkdir -p "$pdir"

    if [ ! -f "$pdir/.jkr/mesh/$SLUG.json" ]; then
      echo "peer-$i: joining..."
      HOME=$pdir "$JKR" mesh join "$INVITE" --display-name "bot-$i" \
          > "$LOGDIR/peer-$i-join.log" 2>&1
    fi

    addr=$(HOME=$pdir "$JKR" mesh whoami "$SLUG" 2>/dev/null | awk '/^address/ {print $2}')
    pub=$(HOME=$pdir  "$JKR" mesh whoami "$SLUG" 2>/dev/null | awk '/^pubkey/  {print $2}')
    echo "peer-$i  $addr"
    echo "$addr"   > "$pdir/.addr"
    echo "$pub"    > "$pdir/.pub"

    # 4. tail in background — this is what keeps the WSS connection open
    # and ticks "peers online" up by one per tail.
    # --reconnect keeps the WSS link up across broker restarts; without it
    # a single blip drops the bot to "enrolled but not connected" forever.
    nohup env HOME=$pdir "$JKR" mesh tail "$SLUG" --reconnect \
        >> "$LOGDIR/peer-$i-tail.log" 2>&1 &
    echo $! >> "$PIDFILE"
  done

  # 5. heartbeat loop: every $HEARTBEAT_SECS, peer-1 → peer-2 → peer-3 → 1.
  HEARTBEAT_SECS=${HEARTBEAT_SECS:-45}
  HEARTBEAT_SCRIPT=$WORK/heartbeat.sh
  cat > "$HEARTBEAT_SCRIPT" <<EOF
#!/usr/bin/env bash
set -uo pipefail
WORK=$WORK
JKR=$JKR
SLUG=$SLUG
PEERS=$PEERS
HEARTBEAT_SECS=$HEARTBEAT_SECS
LOGDIR=$LOGDIR
export JKR_MESH_WS_COOKIE='$JKR_MESH_WS_COOKIE'
trap 'exit 0' TERM INT
while sleep "\$HEARTBEAT_SECS"; do
  for i in \$(seq 1 \$PEERS); do
    nxt=\$(( (i % PEERS) + 1 ))
    SRC=\$WORK/peer-\$i
    DST=\$WORK/peer-\$nxt
    addr=\$(cat "\$DST/.addr")
    pub=\$(cat "\$DST/.pub")
    HOME=\$SRC "\$JKR" mesh send "\$SLUG" \
        --to "\$addr" --recipient-pubkey "\$pub" \
        "[bot-\$i] heartbeat \$(date -u +%H:%M:%S)" \
        >> "\$LOGDIR/heartbeat.log" 2>&1
  done
done
EOF
  chmod +x "$HEARTBEAT_SCRIPT"
  nohup "$HEARTBEAT_SCRIPT" >> "$LOGDIR/heartbeat.log" 2>&1 &
  echo $! >> "$PIDFILE"

  echo
  echo "✓ $PEERS peers tailing + heartbeat every ${HEARTBEAT_SECS}s"
  echo "  invite: $INVITE"
  echo "  pids:   $(wc -l < "$PIDFILE") running"
  echo "  logs:   $LOGDIR"
  echo
  echo "  status:  ./scripts/mesh-bot.sh status"
  echo "  stop:    ./scripts/mesh-bot.sh stop"
}

case "${1:-start}" in
  start)  cmd_start  ;;
  stop)   cmd_stop   ;;
  status) cmd_status ;;
  restart) cmd_stop; sleep 1; cmd_start ;;
  *) echo "usage: $0 {start|stop|status|restart}" >&2; exit 1 ;;
esac
