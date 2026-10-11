# Writing Python in StateMCP

Read this resource before writing or changing Python scripts, published functions,
or HTTP handlers. It explains the working environment and StateMCP helpers needed
for those tasks. Code uses Monty's supported subset of Python 3, not a full CPython
installation. There is no pip installation, arbitrary third-party package access,
or direct host filesystem or network access. Use the supplied helpers for state.

## Choose how the code runs

- **`execute`:** pass a script and optional JSON `inputs`. The script sees `inputs`
  as a variable. Its final expression becomes `value`; printed output is captured
  in `stdout`. The result is `{"value": ..., "stdout": "..."}`. Set `namespace`
  when using database/file helpers or importing namespace Python files.
- **Published function:** write a Python file with `fs.write`, then publish its
  `file` and `symbol` using `function.declare`. `call.arguments` becomes keyword
  arguments to that function. Its return value must be JSON-compatible. Supply
  input/output schemas to establish the contract. `function.update` takes the
  complete declaration and republishes current sources.
- **HTTP handler:** publish a callable with `api.publish`. It receives one
  positional Request and returns buffered HTTP response forms, described below.
  Read `statemcp://guides/api-and-sites` for the website implementation patterns.

Published module initialization and imported module initialization during
publication cannot call state helpers. Put queries, writes, and nested calls
inside functions. Globals and imported modules are fresh on every invocation;
persist shared state in namespace databases or files.

## Available Python helpers

These functions are already available; do not import them from a package.

| Helper | What it does |
| --- | --- |
| `config()` | Returns an attribute object: `.public_url` is the public origin or `None`, and `.prefix` is the route prefix (empty at root). No credentials are exposed. |
| `db_query(database, sql, params=[])` | Runs one read-only SQL statement. |
| `db_execute(database, sql, params=[])` | Runs one SQL statement, including writes and `RETURNING`. |
| `db_inspect(database)` | Returns schema, tables, migrations, and storage metadata. |
| `read_text(path)` | Reads a namespace virtual file as UTF-8 text. |
| `write_text(path, text)` | Creates/replaces a namespace virtual text file; returns metadata. |
| `call(namespace, function, arguments={})` | Invokes a published function; returns its JSON value. |
| `mcp(name, arguments={})` | Invokes a StateMCP tool by its wire name; returns its application result directly. |

Database and text helpers use the function's namespace or `execute.namespace`.
Their paths are virtual namespace paths, not paths on the server's host filesystem.
Use `mcp` for operations without a dedicated helper, such as migrations or binary
file access. Get exact tool arguments from `describe({"tool":"db.migrate"})`
(or the relevant tool name) rather than guessing them.

## SQL inputs and results

Bind values with `?` placeholders and `params`, rather than interpolating values
into SQL. Query and execution results have this shape:

```python
{"columns": ["id", "text"], "rows": [[1, "hello"]], "rows_affected": 0}
```

Rows are lists in column order, not attribute objects. Convert them when useful:

```python
def list_notes():
    result = db_query('app', 'SELECT id, text FROM notes ORDER BY id')
    return [dict(zip(result['columns'], row)) for row in result['rows']]
```

Create the named database and table before invoking this example. Writes can use
`RETURNING`; handle an empty result when a statement may match no records. SQL
parameters accept JSON scalars, `{"$base64": "..."}` for blobs, and
`{"$integer": "..."}` for signed 64-bit integers. Helpers accept one statement;
use `db.migrate` for an ordered batch of schema migrations. Do not issue manual
transaction control statements: StateMCP owns the transaction.

## Namespace authority and nested calls

Published functions and HTTP handlers have full access to their own namespace's
files, databases, functions, and API declarations. They cannot access another
namespace. Root `execute` scripts have owner access; setting `execute.namespace`
chooses default helper/import resolution, not a restricted authority scope.
Nested scripts inherit a published function's restrictions.

Inside a namespace-scoped function, `self` selects that namespace and survives
renames/copies:

```python
def summary():
    return call('self', 'list_notes', {})
```

Root scripts should use an explicit namespace name or UUID for `call`/`mcp`;
`self` is an authority alias for namespace-scoped execution, not a general alias
for `execute.namespace`. If a nested call needs `expected_version` or an
idempotency key, use `mcp('call', {...})` with the full tool arguments; the short
`call` helper accepts only namespace, function, and arguments.

## Imports and publication

Store supporting Python source in the same namespace, for example `/helpers.py`,
and import it normally (`import helpers`, `from helpers import format_item`).
The entry directory is searched first, then the namespace root. Packages,
relative imports, and Monty's built-in standard-library subset are available.
Imports cannot read another namespace, host Python packages, or host files.

Publication pins the entry source and namespace Python source snapshot, including
imports inside function bodies. Editing files alone does not change published
code: republish a function with `function.update`, or a handler with `api.publish`.
`execute` uses current files. Module caches last only for one invocation.

## Transactions, errors, and return values

All nested state calls and writes share the root operation's transaction.
Uncaught Python errors, failed host operations, schema failures, or invalid HTTP
responses roll back staged writes. Host-operation errors terminate execution;
Python `try/except` cannot catch them and continue making changes. Check expected
conditions before writing and raise a Python error for an invalid operation.

Use `expected_revision` for namespace changes and `expected_version` when code
identity matters. Application record revisions are separate invariants enforced
by your own functions. A database `snapshot` is storage identity, not a revision
or restore token. Reuse an idempotency key only with identical request arguments.

Ordinary function/helper results must be JSON-compatible: null/None, booleans,
strings, signed 64-bit integers, finite floats, lists, and string-keyed dicts.
Returned tuples become arrays. Bytes, sets, custom objects, cyclic values,
non-finite floats, and out-of-range integers require explicit JSON encoding.
Source, time, recursion, host-call counts, captured output, and result sizes are
bounded; keep operations small and paginate records. `describe` shows limits.

## HTTP Request and response essentials

- Method and URL: `request.method`, `path`, `full_path`, `url`, `base_url`, `host`, `scheme`.
- Input: `request.args`, `view_args`, `form`, `json`, `headers`, `cookies`, `files`.
- Body: `request.body`/`data` are bytes; `request.text` is text.
- Query/form/header mappings support `get`, `getlist`, indexing, and `items(multi=True)`.
- Parsing helpers: `request.get_json(force=False, silent=False)` and
  `request.get_data(as_text=False)`; uploads expose `filename`, `content_type`, and `body`.

Return a dict/list for JSON, text/bytes for a body, or Flask tuples such as
`(body, status)`, `(body, headers)`, or `(body, status, headers)`. `Response` is
available without an import: `Response(body, status=200, headers=...,
content_type=...)`. HTTP tuples control the response; lists remain JSON.
Streaming/generators and WSGI applications are unsupported.

A valid error-status response still **commits** writes. Returning `(body, 400)`
is not rollback; decide before writing or raise an error when rollback is needed.
Escape user-controlled values when returning HTML.

## Bulk files through HTTP MCP

HTTP MCP advertises `fs.upload` and `fs.download`; stdio and CLI do not expose
these transport tools. They are called directly through MCP, not through Python's
`mcp()` helper. Use an agent's curl access to redeem their URLs. Ordinary Python
HTTP handlers still have the separate 1 MiB request-body bound.

`config()` is available in scripts, published functions and handlers, inside
function bodies. For example, `config().prefix + '/notes/api/items'` builds a
prefix-aware path; prepend `config().public_url or ''` for an external URL.
