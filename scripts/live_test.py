#!/usr/bin/env python3
"""Read-only live MCP checks. Never print credentials or upstream payloads."""
import argparse
import json
import os
from pathlib import Path
import queue
import re
import subprocess
import threading

from env_config import load_token

class ToolError(RuntimeError):
    def __init__(self, name, status):
        self.status = status
        suffix = f"HTTP {status}" if status else "tool error; payload omitted"
        super().__init__(f"{name}: {suffix}")


class Session:
    def __init__(self, binary, token, docker_image=None):
        command = (["docker", "run", "--rm", "-i", "--read-only", "--cap-drop=ALL",
                    "--security-opt=no-new-privileges", "--env", "INFINITY_API_TOKEN", docker_image]
                   if docker_image else [str(binary.resolve())])
        self.proc = subprocess.Popen(
            command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
            stderr=subprocess.PIPE, text=True,
            env={**os.environ, "INFINITY_API_TOKEN": token, "RUST_LOG": "warn"},
        )
        self.messages = queue.Queue()
        self.request_id = 0
        threading.Thread(target=self.read_stdout, daemon=True).start()
        # Drain diagnostics without copying them into the test report.
        threading.Thread(target=lambda: self.proc.stderr.read(), daemon=True).start()

    def read_stdout(self):
        for line in self.proc.stdout:
            self.messages.put(line)
        self.messages.put(None)

    def send(self, message):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", **message}) + "\n")
        self.proc.stdin.flush()

    def request(self, method, params):
        self.request_id += 1
        self.send({"id": self.request_id, "method": method, "params": params})
        while True:
            line = self.messages.get(timeout=80)
            if line is None:
                raise RuntimeError("MCP process exited before replying")
            response = json.loads(line)
            if response.get("id") != self.request_id:
                continue
            if "error" in response:
                raise RuntimeError(f"{method}: MCP protocol error (payload omitted)")
            return response["result"]

    def tool(self, name, arguments):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            # Our server emits sanitized API errors, but do not print arbitrary payloads.
            text = " ".join(item.get("text", "") for item in result.get("content", []))
            status = re.search(r"HTTP (\d{3})", text)
            raise ToolError(name, int(status.group(1)) if status else None)
        value = result.get("structuredContent")
        if value is None:
            value = json.loads(result["content"][0]["text"])
        print(f"PASS: {name}", flush=True)
        return value

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
            self.proc.wait()
            raise RuntimeError("MCP process failed to shut down")
        if self.proc.returncode != 0:
            raise RuntimeError("MCP process exited unsuccessfully")


def rows(payload):
    if not isinstance(payload, dict) or not isinstance(payload.get("data"), list):
        raise RuntimeError("Unexpected Infinity list response shape (payload omitted)")
    return payload["data"]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--env-file", type=Path, default=Path(".env"))
    parser.add_argument("--binary", type=Path, default=Path("target/release/start-infinity-mcp"))
    parser.add_argument("--docker", metavar="IMAGE", help="Test a prebuilt container image over stdio")
    args = parser.parse_args()
    session = Session(args.binary, load_token(args.env_file), args.docker)
    try:
        init = session.request("initialize", {
            "protocolVersion": "2025-11-25", "capabilities": {},
            "clientInfo": {"name": "infinity-live-test", "version": "1.0"},
        })
        assert init["serverInfo"]["name"] == "start-infinity-mcp"
        session.send({"method": "notifications/initialized"})
        tools = session.request("tools/list", {})["tools"]
        assert len(tools) == 7 and all(t["annotations"]["readOnlyHint"] for t in tools)
        print("PASS: MCP initialization and seven read-only tool definitions", flush=True)
        workspace_page = session.tool("list_workspaces", {"limit": 5})
        workspaces = rows(workspace_page)
        if not workspaces:
            raise RuntimeError("No accessible workspaces; remaining live checks cannot run")
        # Verify a real cursor independently of board selection.
        first = session.tool("list_workspaces", {"limit": 1})
        if first.get("has_more") and first.get("after"):
            second = session.tool("list_workspaces", {"limit": 1, "after": first["after"]})
            assert rows(second) and rows(second)[0]["id"] != rows(first)[0]["id"]
            print("PASS: live workspace cursor pagination", flush=True)
        else:
            print("SKIP: workspace pagination (only one accessible result)", flush=True)
        sampled_board = False
        sampled_item = False
        comments_count = 0
        restricted_workspaces = 0
        for workspace in workspaces[:3]:
            workspace_id = str(workspace["id"])
            boards = rows(session.tool("list_boards", {"workspace_id": workspace_id, "page": {"limit": 5}}))
            for board in boards[:5]:
                scope = {"workspace_id": workspace_id, "board_id": str(board["id"])}
                rows(session.tool("list_folders", {**scope, "page": {"limit": 2}}))
                schema = session.tool("get_board_schema", {**scope, "page": {"limit": 2}})
                attributes = rows(schema)
                if schema.get("has_more") and schema.get("after"):
                    next_schema = session.tool("get_board_schema", {**scope, "page": {"limit": 2, "after": schema["after"]}})
                    assert rows(next_schema)
                    assert not ({a["id"] for a in attributes} & {a["id"] for a in rows(next_schema)})
                    print("PASS: live attribute cursor pagination", flush=True)
                sampled_board = True
                items = rows(session.tool("list_items", {**scope, "page": {"limit": 2}}))
                if not items:
                    continue
                item = items[0]
                item_scope = {**scope, "item_id": str(item["id"])}
                try:
                    fetched = session.tool("get_item", item_scope)
                    comments = rows(session.tool("list_comments", {**item_scope, "page": {"limit": 2}}))
                except ToolError as error:
                    if error.status != 402:
                        raise
                    restricted_workspaces += 1
                    print("SKIP: item/comment reads in a workspace returning HTTP 402 (subscription restriction)", flush=True)
                    break
                if isinstance(fetched, dict) and isinstance(fetched.get("data"), dict):
                    fetched = fetched["data"]
                assert str(fetched["id"]) == str(item["id"])
                assert isinstance(fetched.get("values"), list)
                expanded = sum(isinstance(v.get("attribute"), dict) for v in fetched["values"])
                print(f"PASS: item identity and values ({expanded} expanded attribute definitions)", flush=True)
                if fetched["values"] and expanded == 0:
                    raise RuntimeError("Item values were returned without expanded attribute definitions")
                if item.get("folder_id"):
                    filtered = rows(session.tool("list_items", {**scope, "folder_id": str(item["folder_id"]), "page": {"limit": 2}}))
                    assert filtered and all(str(v.get("folder_id")) == str(item["folder_id"]) for v in filtered)
                    print("PASS: live folder filtering", flush=True)
                comments_count = len(comments)
                if not comments_count and len(items) > 1:
                    comments = rows(session.tool("list_comments", {**scope, "item_id": str(items[1]["id"]), "page": {"limit": 2}}))
                    comments_count = len(comments)
                if not comments_count:
                    candidates = rows(session.tool("list_items", {**scope, "page": {"limit": 10}}))
                    seen = {str(candidate["id"]) for candidate in items}
                    for candidate in candidates:
                        if str(candidate["id"]) in seen:
                            continue
                        comments = rows(session.tool("list_comments", {**scope, "item_id": str(candidate["id"]), "page": {"limit": 2}}))
                        comments_count = len(comments)
                        if comments_count:
                            break
                expanded_authors = sum(isinstance(c.get("created_by"), dict) for c in comments)
                if comments_count and expanded_authors != comments_count:
                    raise RuntimeError("Comments returned without expanded authors")
                print(f"PASS: comments response shape ({comments_count} sampled comments, {expanded_authors} expanded authors)", flush=True)
                sampled_item = True
                break
            if sampled_item:
                break
        if not sampled_board or not sampled_item:
            raise RuntimeError("No populated board found within the bounded sample; item/comment checks incomplete")
        print("PASS: all seven tools exercised against live Infinity through MCP; no writes performed", flush=True)
        if restricted_workspaces:
            print(f"LIMIT: {restricted_workspaces} sampled workspace(s) returned HTTP 402 for item/comment reads", flush=True)
        if not comments_count:
            print("LIMIT: sampled item has no comments; nonempty comment content/author expansion not verified", flush=True)
    finally:
        session.close()


if __name__ == "__main__":
    try:
        main()
    except Exception as error:
        # Keep traceback and API payloads out of user-facing output.
        print(f"FAIL: {error if isinstance(error, RuntimeError) else type(error).__name__}", flush=True)
        raise SystemExit(1)
