#!/usr/bin/env python3
"""Exercise the real stdio executable without credentials or external requests."""
import json
import argparse
import os
from pathlib import Path
import queue
import subprocess
import threading


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", nargs="?", type=Path, default=Path("target/debug/start-infinity-mcp"))
    parser.add_argument("--docker", metavar="IMAGE", help="Test a prebuilt container image over stdio")
    args = parser.parse_args()
    command = (["docker", "run", "--rm", "-i", "--read-only", "--cap-drop=ALL",
                "--security-opt=no-new-privileges", "--env", "INFINITY_API_TOKEN", args.docker]
               if args.docker else [str(args.binary.resolve())])
    missing_token_env = {key: value for key, value in os.environ.items()
                         if key != "INFINITY_API_TOKEN"}
    startup = subprocess.run(command, env=missing_token_env, capture_output=True,
                             text=True, timeout=5)
    assert startup.returncode != 0
    assert startup.stdout == ""
    assert "INFINITY_API_TOKEN" in startup.stderr
    env = {**os.environ, "INFINITY_API_TOKEN": "smoke-test-dummy-token"}
    # Capture output without logging any inherited user configuration.
    proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                            stderr=subprocess.PIPE, text=True, env=env)
    messages = queue.Queue()

    def read_messages():
        for line in proc.stdout:
            messages.put(line)

    threading.Thread(target=read_messages, daemon=True).start()

    def send(message):
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
        proc.stdin.flush()

    def request(request_id, method, params):
        send({"id": request_id, "method": method, "params": params})
        for _ in range(20):
            response = json.loads(messages.get(timeout=10))
            if response.get("id") == request_id:
                return response
        raise AssertionError("No matching response")

    try:
        response = request(1, "initialize", {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "infinity-smoke-test", "version": "1.0"},
        })
        assert response["result"]["serverInfo"]["name"] == "start-infinity-mcp"
        send({"method": "notifications/initialized"})
        response = request(2, "tools/list", {})
        tools = response["result"]["tools"]
        assert {tool["name"] for tool in tools} == {
            "list_workspaces", "list_boards", "list_folders", "get_board_schema",
            "list_items", "get_item", "list_comments",
        }
        assert all(tool["annotations"]["readOnlyHint"] for tool in tools)
        assert all(tool["inputSchema"]["type"] == "object" for tool in tools)
        # Invalid limits fail before any HTTP request, even with a dummy token.
        response = request(3, "tools/call", {
            "name": "list_workspaces", "arguments": {"limit": 101},
        })
        assert response["result"]["isError"] is True
        assert "between 1 and 100" in response["result"]["content"][0]["text"]
        response = request(4, "tools/call", {
            "name": "get_item", "arguments": {
                "workspace_id": "669", "board_id": "../escape", "item_id": "test",
            },
        })
        assert response["result"]["isError"] is True
        response = request(5, "tools/call", {"name": "get_item", "arguments": {}})
        assert "error" in response or response.get("result", {}).get("isError")
    finally:
        proc.stdin.close()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()
            proc.wait()
            raise AssertionError("Server did not exit after stdin closed")
        proc.stdout.close()
        proc.stderr.close()
    assert proc.returncode == 0, f"Unexpected server exit: {proc.returncode}"
    print("PASS: credential startup check, stdio initialization, seven read-only tools, validation and clean shutdown")


if __name__ == "__main__":
    main()
