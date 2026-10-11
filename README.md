# StateMCP: Let Your Agent Make Its Own Tools

StateMCP is an MCP server that lets your agent create SQLite databases and write
Python tools that use them—all through MCP. The tools and their data persist,
so your agent can reuse and extend what it builds in later conversations.

Over HTTP, multiple chat clients can share the same tools and state. That enables
ad hoc swarming. Beware—or be excited about that prospect.

## Use cases

### 1. A Reddit for your agent swarm

Have your agents build a shared forum with posts, comments, topics, and votes.
They can post findings, discuss approaches, and surface useful results across
clients connected to the same HTTP server.

### 2. One shared memory for all your sessions

Have your agent build tools to save and retrieve preferences, project context,
and decisions. Every session connected to the same store can access that memory
and update it as work progresses.

### 3. A software factory inside your chat

Why use Jira/Linear when your agents can build their own issue tracker inside
StateMCP? Start with issues, projects, and assignments. Add dependencies,
priorities, review queues, and release tracking as your workflow grows.

Your agents can build the tools they use to organize and coordinate software
delivery, with shared SQLite state across sessions. The issue tracker is just
the start: ask for the next feature, and they can build that too.

## Quick start

```sh
cargo build --release

# Run locally over stdio
target/release/statemcp stdio ./data

# Or serve multiple clients over HTTP
target/release/statemcp http ./data --bind 127.0.0.1:8000

# Serve under a prefix on both IPv4 and IPv6 loopback interfaces
target/release/statemcp http ./data --public-url https://example.com/statemcp \
  --bind 127.0.0.1:8000 --bind '[::1]:8000' --auth-bearer YOUR_TOKEN
```

For stdio, configure your client to launch the binary with `stdio` and an absolute
path to your data directory. For HTTP, the endpoint is `http://127.0.0.1:8000/mcp`.
Use the same data directory to keep your tools and data across restarts.

Repeat `--bind` to listen on multiple addresses with shared state and MCP sessions.
`--public-url https://example.com/statemcp` exposes `/statemcp/mcp/` and
`/statemcp/health`, and reports external URLs using that origin and prefix.
Use `--public-url /statemcp` for relative URLs. Query strings, fragments,
credentials, traversal, and ambiguous path segments are rejected at startup.
The legacy `--http-prefix` option remains available and conflicts with `--public-url`.
Trailing slashes are normalized before routing. Without a
prefix, the routes are `/mcp` (or `/mcp/`) and `/health`. Health returns
`{"status":"ok"}` without bearer authentication; MCP requires the token when
`--auth-bearer` is configured. All routes check the request Host and Origin against
the listening addresses. When using a reverse proxy, forward a permitted upstream
Host and preserve authentication and streaming.

StateMCP uses the official Rust MCP SDK for stdio and Streamable HTTP.
Logs go to stderr; set `RUST_LOG=debug` for more detail.

## Ubuntu server installation

Copy `script.sh` and the Linux binary to an Ubuntu 22.04+ host running systemd:

```sh
./script.sh /path/to/statemcp
```

The executable installer uses sudo as needed. It installs Nginx, OpenSSL, and
systemd configuration; creates a dedicated service user and persistent data
directory; generates a self-signed TLS certificate; enables both services; and
checks the local health endpoint. Nginx listens on IPv4 and IPv6 ports 80 and 443,
redirects HTTP to HTTPS, and forwards `/statemcp/` to StateMCP's IPv4/IPv6 loopback
listeners. The server exposes `/statemcp/mcp` with bearer authentication and
`/statemcp/health` without bearer authentication, normalizing trailing slashes.

Defaults reproduce the `micro` deployment: hostname `micro.yiblet.com` and bearer
token `chernoff-bound`. Override them using `STATEMCP_HOSTNAME` and
`STATEMCP_BEARER_TOKEN`. Reruns preserve data, an existing credential unless
explicitly overridden, and a valid certificate matching the hostname. The token
is stored root-only at `/etc/statemcp/bearer-token`; the service loads it through
systemd credentials. Certificate renewal is manual: rerun the installer near
expiry to generate a replacement.

Allow TCP 80 and 443 in the cloud firewall and point DNS at the host. With a
Cloudflare-proxied record, use TLS mode **Full** for the self-signed origin
certificate. The script configures an already-active UFW firewall but does not
create cloud firewall rules or change Cloudflare settings. It replaces the
Ubuntu default Nginx site, so use it on a host dedicated to this deployment.
For image preparation without starting services, set `STATEMCP_START_SERVICES=0`.

## What your agent can do

- Create SQLite databases, define tables, and run queries.
- Write and publish Python functions that read and update those databases.
- Discover and call the functions it creates through `describe` and `call`.
- Extend existing tools as your needs change.

The agent can call `describe({"mode":"runtime"})` for tool-writing instructions
and `describe({"mode":"full"})` for the complete tool schemas.

See the [todo tools](examples/todos.py) and [example setup](examples/setup.json),
or run the demo:

```sh
python3 examples/demo.py --binary target/release/statemcp --data-dir ./demo-data
```

Before writing Python scripts, functions, or handlers, read MCP resource
`statemcp://guides/python`. The [Python guide](resources/python.md) explains the
execution model, available state helpers, namespace scope, imports, transactions,
and return values needed to write working code.

Inspect everything advertised over MCP without running the demo or changing
application data:

```sh
cargo build
python3 examples/introspect.py > /tmp/statemcp-introspection.jsonl
# Optionally inspect published functions in an existing store:
python3 examples/introspect.py --data-dir ./data
```

The script prints initialization, tool/resource catalogs, resource contents, all
`describe` modes, and namespace function contracts, then stops the stdio server.
It uses only the Python standard library and defaults to a disposable empty store.

## How it works

StateMCP comes with three primitives:

- **A virtual file system** to store the Python code your agent writes.
- **Monty**, an embedded Python interpreter, to run that code.
- **SQLite** to store data and query it with SQL.

Your agent combines them through MCP: create a database, define its tables,
write Python functions that use it, and publish those functions as tools it can
discover and call. The tools and data persist in your StateMCP store.

Python files can import siblings with `import helpers` or `from helpers import name`.
Publish again after editing a helper to update a tool.

Published functions can use every file, database, and function in their own
namespace. They can build new tools there, but cannot access other namespaces.

That gives your agent the building blocks for shared memory, forums, issue
trackers, and whatever your workflow needs next. It can update the code and
database schema as those needs change, while other connected agents use the
same tools and state.

See [the tool reference](reference.md) for exact tool names, arguments, results,
and Python helpers.

## Workspace crates and tests

- [state-contract](crates/state-contract/README.md): stable shared error categories.
- [state-fs](crates/state-fs/README.md): blocking physical and in-memory filesystem adapters.
- [state-migrations](crates/state-migrations/README.md): ordered Rust catalog migrations.
- [state-store](crates/state-store/README.md): SQLite overlays, publication, and recovery.
- [state-domain](crates/state-domain/README.md): typed resources and transactions.
- [state-runtime](crates/state-runtime/README.md): Monty execution and framed workers.
- [state-core](crates/state-core/README.md): service operations, authorization, and root budgets.

Unit tests live under each crate's `src/` in `#[cfg(test)]` modules,
including the root package. They use in-memory filesystems, byte buffers,
Tokio duplex streams, and direct HTTP router requests. They do not read or write
host files, bind sockets, or launch child processes. Example fixtures and the
reference schemas are embedded at compile time.

```sh
cargo test --workspace --lib --bins  # in-memory unit tests
cargo test --test e2e                # real I/O end-to-end tests
cargo test --workspace --all-targets # both suites
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all --check
```

The [end-to-end suite](tests/e2e/README.md) lives under `tests/e2e/`, with one
Cargo integration target. It exercises physical filesystem operations,
cross-process locks, real CLI/stdio executables, HTTP sockets and listener
cleanup, worker isolation, and aggregate allocator limits. Real-I/O scenarios
run serially to avoid transient inherited-lock contention during process spawning.

The scripts under [`test/`](test/README.md) remain manual executable checks.

## Application architecture

MCP and CLI adapt `state-contract` inputs to `state-core::Controller` use cases.
Controllers open and finish one transaction; concrete feature services share its
`Context`. Runtime host callbacks invoke those same services with retained
namespace scope and budgets. Source loading and runtime execution have injectable
boundaries for testing. See [state-core](crates/state-core/README.md) for service
composition and typed transactional embedding.

The MCP server receives a shared `BlockingExecutor`; cloned sessions share
admission and execute synchronous application work off the async workers. Existing
stores retain their format and require no migration for this service rewrite.

## HTTP APIs and websites

MCP advertises the resource `statemcp://guides/api-and-sites` for API and website
work. Its description identifies when to read it; use `resources/list` to discover
guides and `resources/read` to load a matching guide before starting that workload.
The [API and sites guide](resources/api-and-sites.md) covers small MCP payloads,
CDN imports, and simple server-rendered HTML, HTMX, and Tailwind patterns.

Use `api.publish` through MCP or `statemcp cli api.publish DATA_DIR --json ...`.
HTTP handlers receive exactly one positional request; routes require no decorators:

```python
def handle(request):
    if request.method == "POST":
        db_execute("app", "INSERT INTO notes(text) VALUES (?)", [request.json["text"]])
    return {"notes": db_query("app", "SELECT text FROM notes")["rows"]}
```

```json
{"namespace":"notes","name":"notes-api","path":"/api/notes","methods":["GET","POST"],"target":{"kind":"function","file":"/web.py","symbol":"handle"}}
```

Write the Python file and create its databases first. Publication pins the current
source and imports. Omit `methods` to handle any HTTP method, including extensions.
Paths accept `{id}` captures and a final `{*path}`; captures are in
`request.view_args`. Use `api.get`, `api.list`, and `api.remove` to manage routes.
On HTTP MCP, publication returns configured public URL patterns, including server
prefixes. Without an origin, URLs are relative to the server. CLI/stdin publication has no listening endpoint and returns empty URLs;
use HTTP MCP `api.get` once serving to discover the location.

Mount static namespace files explicitly:

```json
{"namespace":"notes","name":"site","path":"/{*path}","target":{"kind":"static","directory":"/public","index":"index.html"}}
```

This serves `/public/index.html` at `/notes/` and its assets beneath that URL.
Directories redirect to trailing-slash URLs; binary assets, HEAD and ETags work.
Namespace file updates become visible on the next request. Routes follow namespace
copy/rename/delete; a stable UUID URL alias is also available.

Start `statemcp http DATA_DIR --bind 127.0.0.1:8000`, then call the API with
`curl http://127.0.0.1:8000/notes/api/notes`. Namespace APIs and static websites
are public and do not require a bearer token; `--auth-bearer` protects MCP only.
Host and Origin checks apply to all routes. Handlers have full access to their own
namespace's files, databases, functions and API declarations, with no per-route
permission lists. Cross-namespace access is denied.

Request properties include `body`/`data` bytes, `text`, `json`, `headers`, `args`,
`view_args`, `form`, `files`, `cookies`, `method`, `path`, `full_path`, `url`,
`base_url`, `host`, `scheme`, and content metadata. Query/form/header mappings
support indexing, `get`, `getlist`, and `items(multi=True)`. Upload objects expose
`filename`, `content_type`, and byte `body`. `get_json(force=False, silent=False)`
and `get_data(as_text=False)` are available. Requests are buffered up to 1 MiB;
query strings, headers and form field counts have separate bounds.

Return text or bytes; dict/list values produce JSON. Flask tuples `(body,status)`,
`(body,headers)` and `(body,status,headers)` control status/headers; lists remain JSON.
`Response(body, status=200, headers=..., content_type=...)` is also available.
Repeated response headers are retained. This implements ordinary buffered Flask
return forms; streaming generators and WSGI applications are outside this scope.
A valid returned error status commits writes. Invalid responses and failed host
operations roll back every write in the request's shared transaction.

Existing catalogs migrate automatically to content-addressed source storage.
Namespace history defaults to seven revisions; use `--history-size N` to configure
a positive count. Each namespace can also keep up to seven pinned revisions,
with optional unique names (`namespace.history`, `namespace.pin`, `namespace.unpin`).
Unreferenced content is collected on publication. HTTP routes are optional manifest
metadata; ordinary function formats and declaration hashes are unchanged. Use the
current binary when writing route metadata; older binaries do not understand it.

## Bulk upload and download

HTTP MCP advertises two additional tools; neither is discoverable or callable over
stdio. They return a random `token`, `url`, Unix-seconds `expires_at`, and normalized
`files`. Tokens expire in ten minutes, authorize one operation, and are invalidated
on restart. A failed redemption consumes the token. Redeeming needs no MCP bearer.

```json
{"namespace":"notes","files":["public/image.png","public/style.css"]}
```

Call `fs.upload` with that input, then send multipart file fields with the exact
returned absolute paths (including folders):

```sh
curl --fail "$UPLOAD_URL" \
  -F '/public/image.png=@image.png' -F '/public/style.css=@style.css'
```

Missing, duplicate, or extra fields fail without changing any files. The request
supports **10 MiB total file data** and at most 256 files. Multipart overhead has
its own 256 KiB allowance. Destination paths cannot traverse parents. Files are
staged through async I/O before one atomic storage transaction; binary contents
never pass through JSON/base64. `fs.write`/`fs.append` retain their 8 MiB limit.

Call `fs.download` with literal file paths or glob patterns, and an optional format:

```json
{"namespace":"notes","files":["public/**","/config.json"],"format":"tar"}
```

```sh
curl --fail "$DOWNLOAD_URL" --output files.tar
```

Only `tar` is supported currently; the format field reserves an extensible API.
Selectors must match at least one file each. Glob `*` respects folder boundaries;
`**` spans folders. Selected paths are deduplicated and sorted, up to 256 files.
Tar entries use relative names and preserve folders. File paths resolve when the
token is issued; contents come from one storage snapshot at redemption. Downloads
stream with backpressure rather than buffering an entire archive.

`--public-url` controls transfer and API/site URLs. Python scripts, functions, and
handlers can read `config().public_url` (origin or `None`) and `config().prefix`.
HTTP transfer tools are called directly through MCP, rather than Python `mcp()`.
