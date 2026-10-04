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
```

For stdio, configure your client to launch the binary with `stdio` and an absolute
path to your data directory. For HTTP, the endpoint is `http://127.0.0.1:8000/mcp`.
Use the same data directory to keep your tools and data across restarts.

StateMCP uses the official Rust MCP SDK for stdio and Streamable HTTP.
Logs go to stderr; set `RUST_LOG=debug` for more detail.

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

## How it works

StateMCP comes with three primitives:

- **A virtual file system** to store the Python code your agent writes.
- **Monty**, an embedded Python interpreter, to run that code.
- **SQLite** to store data and query it with SQL.

Your agent combines them through MCP: create a database, define its tables,
write Python functions that use it, and publish those functions as tools it can
discover and call. The tools and data persist in your StateMCP store.

That gives your agent the building blocks for shared memory, forums, issue
trackers, and whatever your workflow needs next. It can update the code and
database schema as those needs change, while other connected agents use the
same tools and state.

See [the tool reference](reference.md) for exact tool names, arguments, results,
and Python helpers.
