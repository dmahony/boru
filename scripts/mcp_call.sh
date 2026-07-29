#!/bin/bash
# mcp_call.sh <host> <mcp_port> <method> [params]
# Make a JSON-RPC call to a boru MCP server through SSH
set -e
HOST="$1"
PORT="$2"
METHOD="$3"
PARAMS="${4:-{}}"
PAYLOAD="{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$METHOD\",\"params\":$PARAMS}"
ssh -o ConnectTimeout=5 -o BatchMode=yes "$HOST" "echo '$PAYLOAD' | timeout 5 nc -w 3 127.0.0.1 $PORT" 2>/dev/null
