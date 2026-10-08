# statemcp API test workspace

Run from this folder:

```sh
./build.sh
./curl-all.sh
```

The server uses the official Rust MCP SDK over stdio and Streamable HTTP.
Tool results contain one text block with JSON in `content[0].text`.
Use the curl script or CLI to exercise the API directly.

Build outputs, the local executable, and persistent `data/` are ignored here.
Re-run `./build.sh` after changing statemcp source.

Direct CLI access to persistent test data:

```sh
./statemcp cli namespace.list ./data
./statemcp cli describe ./data
./statemcp cli execute ./data 'inputs + 1' --inputs 41
```

Each tool is under `./statemcp cli <TOOL> ./data`; use `--help` for its arguments.

## Exercise the HTTP API with curl

```sh
./curl-all.sh                       # writes ./curl-results.jsonl
./curl-all.sh /tmp/statemcp.jsonl    # choose an output file
```

Requires Bash, curl, jq, and Cargo. The script builds the local executable,
starts a server on an automatically assigned localhost port with temporary data,
and stops it and removes that data on exit. It does not use `test/data`.
The output file is overwritten on each run.

The requests cover MCP initialization, the initialized notification, ping,
tool listing, all 34 tools and every advertised action, all discovery modes (including the bundled README),
text/binary file writes, namespace copies, endpoint updates, concurrency tokens,
idempotency replay, namespace isolation, and representative expected errors. This covers the API's
request types, not every possible argument combination or error. Live tool
schemas are checked for missing action coverage at the end. HTTP routes are published,
inspected, listed and removed through MCP. Direct HTTP requests verify captures,
repeated query values, extension methods, committed file writes, invalid-response
rollback, static HTML/index redirects, HEAD, method rejection and route removal.
The temporary server requires a bearer token for MCP; namespace API and static
requests deliberately send no token to verify that published pages are public.

MCP JSONL lines contain a direction and a JSON-RPC message:

```json
{"type":"request","message":{"jsonrpc":"2.0","id":2,"method":"ping","params":{}}}
{"type":"response","message":{"jsonrpc":"2.0","id":2,"result":{}}}
```

Messages appear in wire order. Match requests and responses by `message.id`.
Notifications have a request entry and no response. Responses are decoded into JSON-RPC messages; HTTP metadata and validation details are
excluded from MCP entries. Direct endpoint calls use `http_request` entries with
method, URL and body, followed by `http_response` entries with status and body.
Unexpected errors are reported on stderr and produce a nonzero exit
status. Coverage checks still require successful calls for every advertised tool.
The script also checks matching JSON-RPC IDs, text/binary reads, replay equality,
and independent namespace copies. Tool results are JSON encoded in `message.result.content[0].text`, with objects, arrays, and scalars
returned directly.

```sh
jq -c 'select(.type == "request" and .message.params.name == "db.query")' curl-results.jsonl
jq -c 'select(.type == "response") | .message' curl-results.jsonl
```
