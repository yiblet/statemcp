#!/usr/bin/env python3
"""A Python stdlib MCP client; endpoint execution happens in Rust/Monty."""
import argparse
import json
from pathlib import Path
import subprocess


class Client:
    def __init__(self, binary, data_dir):
        self.process = subprocess.Popen(
            [str(binary), "--data-dir", str(data_dir)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True,
        )
        self.next_id = 0
        self.request("initialize", {
            "protocolVersion": "2025-06-18", "capabilities": {},
            "clientInfo": {"name": "state-demo", "version": "1"},
        })
        self.send({"jsonrpc": "2.0", "method": "notifications/initialized"})

    def send(self, message):
        self.process.stdin.write(json.dumps(message) + "\n")
        self.process.stdin.flush()

    def request(self, method, params):
        self.next_id += 1
        self.send({"jsonrpc": "2.0", "id": self.next_id, "method": method, "params": params})
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError("server exited without a response")
        response = json.loads(line)
        if "error" in response:
            raise RuntimeError(response["error"])
        return response["result"]

    def tool(self, name, arguments):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        if result.get("isError"):
            raise RuntimeError(result)
        return result["structuredContent"]

    def call(self, namespace, function, arguments=None):
        result = self.tool("state_call", {
            "namespace": namespace, "function": function, "arguments": arguments or {},
        })
        # MCP structuredContent must be an object, so non-object tool values are wrapped.
        return result.get("value", result)

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=10)
        self.process.stdout.close()
        if self.process.returncode:
            raise RuntimeError(f"server exit: {self.process.returncode}")


def setup(client):
    directory = Path(__file__).resolve().parent
    existing = {item["name"] for item in client.tool("state_namespace", {"action": "list"})["namespaces"]}
    for fixture in json.loads((directory / "setup.json").read_text()):
        namespace = fixture["namespace"]
        if namespace in existing:
            continue
        # TODO(setup): make installation one atomic script and support fixture upgrades.
        client.tool("state_namespace", {"action": "create", "name": namespace})
        client.tool("state_db", {"action": "create", "namespace": namespace, "database": "app"})
        client.tool("state_db", {"action": "migrate", "namespace": namespace, "database": "app", "migrations": [{"id": "initial", "sql": fixture["sql"]}]})
        client.tool("state_fs", {"action": "write", "namespace": namespace, "path": "/api.py", "text": (directory / fixture["file"]).read_text()})
        for endpoint in fixture["endpoints"]:
            client.tool("state_function", dict(endpoint, action="declare", namespace=namespace, file="/api.py"))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/state-mcp"))
    parser.add_argument("--data-dir", type=Path, default=Path(".state-mcp-demo"))
    args = parser.parse_args()
    first = Client(args.binary.resolve(), args.data_dir.resolve())
    second = None
    try:
        setup(first)
        print("note:", first.call("notes", "add", {"text": "A persistent note"}))
        todo = first.call("todos", "add", {"text": "Try State MCP"})
        print("completed todo:", first.call("todos", "complete", {"id": todo[0]}))
        second = Client(args.binary.resolve(), args.data_dir.resolve())
        message = first.call("chat", "post", {"room": "demo", "sender": "model-a", "text": "Hello model-b", "request_id": "demo-first-message"})
        print("second client polls:", second.call("chat", "poll", {"room": "demo", "after": message[0] - 1}))
        print("retry returns same message:", first.call("chat", "post", {"room": "demo", "sender": "model-a", "text": "Hello model-b", "request_id": "demo-first-message"}))
    finally:
        if second is not None:
            second.close()
        first.close()


if __name__ == "__main__":
    main()
