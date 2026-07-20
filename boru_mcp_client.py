#!/usr/bin/env python3
"""Simple JSON-RPC over TCP client for the Boru MCP diagnostic server."""
import json
import socket
import sys

def boru_mcp_call(host, port, method, params=None):
    """Send a JSON-RPC request to the Boru MCP server and return the result."""
    request = {
        "jsonrpc": "2.0",
        "method": method,
        "params": params or {},
        "id": 1,
    }
    
    sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
    sock.settimeout(10)
    try:
        sock.connect((host, port))
        
        # Send request
        payload = json.dumps(request) + "\n"
        sock.sendall(payload.encode("utf-8"))
        
        # Read response
        data = b""
        while True:
            chunk = sock.recv(4096)
            if not chunk:
                break
            data += chunk
            if b"\n" in data:
                break
        
        response = json.loads(data.decode("utf-8").strip())
        
        if "error" in response and response["error"]:
            return {"error": response["error"]}
        return {"result": response.get("result")}
    except socket.timeout:
        return {"error": f"Timeout connecting to {host}:{port}"}
    except ConnectionRefusedError:
        return {"error": f"Connection refused to {host}:{port}"}
    except Exception as e:
        return {"error": str(e)}
    finally:
        sock.close()

def print_result(label, data):
    """Pretty-print a result."""
    print(f"\n{'='*60}")
    print(f"  {label}")
    print(f"{'='*60}")
    if "error" in data and data["error"]:
        print(f"  ERROR: {data['error']}")
    else:
        result = data.get("result", {})
        print(json.dumps(result, indent=2, default=str))

if __name__ == "__main__":
    if len(sys.argv) < 3:
        print("Usage: boru_mcp_client.py <host> <port> [<method> [<params_json>]]")
        print("  or:  boru_mcp_client.py <host> <port> --all")
        sys.exit(1)
    
    host = sys.argv[1]
    port = int(sys.argv[2])
    
    if len(sys.argv) >= 4 and sys.argv[3] == "--all":
        # Run discovery test
        print_result("PING", boru_mcp_call(host, port, "boru_ping"))
        print_result("NODE STATUS", boru_mcp_call(host, port, "boru_get_node_status"))
        print_result("ROOM STATUS", boru_mcp_call(host, port, "boru_get_room_status"))
        print_result("DISCOVERY EVENTS", boru_mcp_call(host, port, "boru_get_discovery_events"))
    else:
        method = sys.argv[3] if len(sys.argv) >= 3 else "boru_ping"
        params = json.loads(sys.argv[4]) if len(sys.argv) >= 5 else {}
        print_result(f"{method}({params})", boru_mcp_call(host, port, method, params))
