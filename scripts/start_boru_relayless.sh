#!/bin/bash
# Start boru headless via screen, with no-relay to avoid endpoint.online() hang
set -e
HOST=$1
NAME=$2
MCP_PORT=$3
DATA_DIR=${4:-/tmp/boru_data_${NAME}}

ssh -o ConnectTimeout=10 -o BatchMode=yes "$HOST" "screen -dmS boru_${NAME} bash -c '
pkill -9 -f boru 2>/dev/null; pkill -9 -f Xvfb 2>/dev/null; sleep 1
rm -rf ${DATA_DIR} 2>/dev/null
xvfb-run -a -e /tmp/xvfb_${NAME}.log /home/dan/boru --no-relay --mcp --enable-gui-test-actions --mcp-bind 127.0.0.1:${MCP_PORT} --name ${NAME} --data-dir ${DATA_DIR} open > /tmp/boru_${NAME}.log 2>&1
'"
echo "Dispatched $NAME"

# Wait for MCP port
for i in $(seq 1 30); do
  sleep 1
  READY=$(ssh -o ConnectTimeout=3 -o BatchMode=yes "$HOST" "ss -tlnp 2>/dev/null | grep -q ${MCP_PORT} && echo yes || echo no" 2>/dev/null)
  if [ "$READY" = "yes" ]; then
    echo "$NAME MCP ready after ${i}s (port ${MCP_PORT})"
    exit 0
  fi
done
echo "WARNING: $NAME MCP not ready after 30s"
exit 1
