#!/usr/bin/env python3
"""Print StateMCP's MCP introspection responses as JSONL, then exit."""

import argparse
import json
from pathlib import Path
from tempfile import TemporaryDirectory

from demo import Client


class IntrospectionClient(Client):
    """Reuse the stdlib stdio client and include initialization in the output."""

    def request(self, method, params):
        result = super().request(method, params)
        print(json.dumps({"method": method, "params": params, "result": result}))
        return result


def catalog(client, method, key):
    """Read every catalog page without assuming a fixed resource or tool list."""
    params = {}
    while True:
        result = client.request(method, params)
        yield from result[key]
        cursor = result.get("nextCursor")
        if cursor is None:
            return
        params = {"cursor": cursor}


def introspect(binary, data_dir):
    client = IntrospectionClient(binary, data_dir)
    try:
        for _tool in catalog(client, "tools/list", "tools"):
            pass
        for _template in catalog(client, "resources/templates/list", "resourceTemplates"):
            pass
        for resource in catalog(client, "resources/list", "resources"):
            client.request("resources/read", {"uri": resource["uri"]})
        for mode in ["overview", "runtime", "full", "readme"]:
            client.tool("describe", {"mode": mode})
        namespaces = client.tool("namespace.list", {})
        for namespace in namespaces["namespaces"]:
            selector = namespace["id"]
            client.tool("function.list", {"namespace": selector})
            client.tool("describe", {"namespace": selector})
    finally:
        client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, default=Path("target/debug/statemcp"))
    parser.add_argument(
        "--data-dir", type=Path,
        help="Inspect an existing store; defaults to a disposable empty store.",
    )
    args = parser.parse_args()
    binary = args.binary.resolve()
    if args.data_dir is not None:
        introspect(binary, args.data_dir.resolve())
    else:
        with TemporaryDirectory(prefix="statemcp-introspection-") as directory:
            introspect(binary, Path(directory))


if __name__ == "__main__":
    main()
