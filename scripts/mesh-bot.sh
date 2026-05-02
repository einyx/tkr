#!/usr/bin/env bash
# scripts/mesh-bot.sh — populate the live mesh stats with synthetic peers.
#
# Mints a "demo" mesh invite, creates N peers (default 3), has each one
# join the mesh and tail (keeping a WSS connection open), and starts a
# heartbeat loop where peers DM each other on a schedule.
#
# All state lives in /tmp/tkr-mesh-bot/ so it can be wiped cleanly:
#   ./scripts/mesh-bot.sh stop       # kill everything
#   ./scripts/mesh-bot.sh start      # spawn N peers (default 3)
#   ./scripts/mesh-bot.sh status     # check what's running
#
# Bots persist as background processes (nohup). To survive a reboot, wrap
# this in a systemd unit or compose service.

set -uo pipefail

cd "$(dirname "$0")/.."
TKR=${TKR:-$(command -v tkr || echo "$HOME/.cargo/bin/tkr")}
[ -x "$TKR" ] || { echo "error: tkr not found at $TKR (run: cargo install --path crates/tkr --locked)" >&2; exit 1; }

WORK=/tmp/tkr-mesh-bot
PIDFILE=$WORK/pids
LOGDIR=$WORK/logs
OWNERFILE=$WORK/owner.env
INVITEFILE=$WORK/invite.url
PEERS=${PEERS:-3}
BROKER=${TKR_MESH_BROKER:-wss://tkr.prysm.sh/api/v1/mesh/ws}
SLUG=${TKR_MESH_SLUG:-demo}

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

  # 1. owner key (re-used across runs unless wiped).
  if [ ! -f "$OWNERFILE" ]; then
    echo "minting owner key..."
    OWN_PRIV=$("$HOME/.foundry/bin/cast" wallet new --json | grep -oE '0x[0-9a-fA-F]{64}' | head -1)
    [ -n "$OWN_PRIV" ] || { echo "error: cast wallet new failed (foundry installed?)" >&2; exit 1; }
    echo "TKR_PAYMENT_KEY=$OWN_PRIV" > "$OWNERFILE"
    chmod 0600 "$OWNERFILE"
  fi

  # 2. mint a fresh 24h invite each start.
  echo "minting 24h invite..."
  "$TKR" mesh invite-mint \
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

  # 3. for each peer: separate $HOME so tkr's mesh state doesn't collide.
  for i in $(seq 1 "$PEERS"); do
    pdir=$WORK/peer-$i
    mkdir -p "$pdir"

    if [ ! -f "$pdir/.tkr/mesh/$SLUG.json" ]; then
      echo "peer-$i: joining..."
      HOME=$pdir "$TKR" mesh join "$INVITE" --display-name "bot-$i" \
          > "$LOGDIR/peer-$i-join.log" 2>&1
    fi

    addr=$(HOME=$pdir "$TKR" mesh whoami "$SLUG" 2>/dev/null | awk '/^address/ {print $2}')
    pub=$(HOME=$pdir  "$TKR" mesh whoami "$SLUG" 2>/dev/null | awk '/^pubkey/  {print $2}')
    echo "peer-$i  $addr"
    echo "$addr"   > "$pdir/.addr"
    echo "$pub"    > "$pdir/.pub"

    # 4. tail in background — this is what keeps the WSS connection open
    # and ticks "peers online" up by one per tail.
    nohup env HOME=$pdir "$TKR" mesh tail "$SLUG" \
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
TKR=$TKR
SLUG=$SLUG
PEERS=$PEERS
HEARTBEAT_SECS=$HEARTBEAT_SECS
LOGDIR=$LOGDIR
trap 'exit 0' TERM INT
while sleep "\$HEARTBEAT_SECS"; do
  for i in \$(seq 1 \$PEERS); do
    nxt=\$(( (i % PEERS) + 1 ))
    SRC=\$WORK/peer-\$i
    DST=\$WORK/peer-\$nxt
    addr=\$(cat "\$DST/.addr")
    pub=\$(cat "\$DST/.pub")
    HOME=\$SRC "\$TKR" mesh send "\$SLUG" \
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
