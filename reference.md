# StateMCP tool reference

This reference describes the 30 public tools advertised by `tools/list`.
The input schemas below come from `describe({"mode":"full"})`. They are also
generated from typed Rust inputs and validated by the SDK and service. Public MCP calls use the names below without an
`action` argument.

## MCP transport

Stdio and Streamable HTTP use the official Rust MCP SDK (`rmcp`). The SDK
handles protocol negotiation, initialization, sessions, and JSON-RPC envelopes.
Tool routing and parameter schemas use rmcp’s `#[tool]`, `#[tool_router]`, and
`#[tool_handler]` macros. Current-protocol tool lists include `ttlMs: 0` and
`cacheScope: "public"`; the fixed catalog contains no user data.

The HTTP endpoint is `/mcp`; clients must accept both `application/json` and
`text/event-stream`. Protocol versions using sessions send the returned
`Mcp-Session-Id` on subsequent requests.

A tool result contains one MCP text block with the application result encoded
as JSON. Parse `content[0].text` to recover objects, arrays, or scalar values.
Application failures set `isError: true` and encode `{error: {code, message}}`
in that block. Unknown tools are protocol errors. `structuredContent` is omitted.
The SDK rejects malformed tool parameters with `isError: true` and diagnostic
text before execution. Application failures retain the JSON error envelope.
The direct CLI and Python helpers return the application JSON value directly.

Use `--auth-bearer TOKEN` to require a shared bearer token over HTTP. This grants
owner access, not per-user permissions. Host and Origin checks match the bound
address (plus localhost for loopback). Logs go to stderr and support `RUST_LOG`.

## Common rules

- Arguments are JSON objects. Unknown fields are rejected.
- `namespace` selects a namespace by name or stable UUID.
- Resource names contain 1–128 bytes without slashes or control characters.
- File paths are absolute virtual POSIX paths. Parent traversal is forbidden.
- `expected_revision` is an optional string guarding the namespace revision;
  `expected_version` guards a published function version. Stale values fail
  with `CONFLICT`.
- Direct mutations include the committed `revision` when they publish a new
  namespace revision. Nested mutations run inside the root transaction.
- A root operation and all nested calls commit together. Failure rolls back
  their staged effects. Concurrent writes may return `CONFLICT`; retry the
  complete operation.
- `idempotency_key` applies only to root `call` and `execute` operations.
  Repeating an identical committed request replays its result; a changed
  request with the same key fails. Receipts persist until maintenance removes them.

Namespace results include `description` as a string or null. Create accepts an
optional description. Update preserves an omitted description and clears it with
`null`; name is optional so descriptions can be updated alone. Copy inherits the
source description unless a string or null is supplied. Existing stores load
without migration, with null descriptions for existing namespaces.

## Tool index

| Tool | Required arguments | Optional arguments |
| --- | --- | --- |
| `namespace.create` | `name` | `description` |
| `namespace.list` | — | — |
| `namespace.get` | `namespace` | — |
| `namespace.update` | `namespace` | `name`, `description`, `expected_revision` |
| `namespace.copy` | `namespace`, `name` | `description`, `expected_revision` |
| `namespace.delete` | `namespace` | `expected_revision` |
| `fs.read` | `namespace`, `path` | — |
| `fs.stat` | `namespace`, `path` | — |
| `fs.list` | `namespace` | `path` |
| `fs.write` | `namespace`, `path` | `base64`, `expected_revision`, `text` |
| `fs.append` | `namespace`, `path` | `base64`, `expected_revision`, `text` |
| `fs.move` | `namespace`, `path`, `destination` | `expected_revision` |
| `fs.copy` | `namespace`, `path`, `destination` | `expected_revision` |
| `fs.delete` | `namespace`, `path` | `expected_revision`, `recursive` |
| `db.list` | `namespace` | — |
| `db.create` | `namespace`, `database` | `expected_revision` |
| `db.drop` | `namespace`, `database` | `expected_revision` |
| `db.query` | `namespace`, `database`, `sql` | `params` |
| `db.execute` | `namespace`, `database`, `sql` | `expected_revision`, `params` |
| `db.inspect` | `namespace`, `database` | — |
| `db.migrations` | `namespace`, `database` | — |
| `db.migrate` | `namespace`, `database`, `migrations` | `expected_revision` |
| `function.list` | `namespace` | — |
| `function.get` | `namespace`, `name` | `expected_version` |
| `function.remove` | `namespace`, `name` | `expected_revision`, `expected_version` |
| `function.declare` | `namespace`, `name`, `file`, `symbol` | `description`, `expected_revision`, `expected_version`, `input_schema`, `output_schema` |
| `function.update` | `namespace`, `name`, `file`, `symbol` | `description`, `expected_revision`, `expected_version`, `input_schema`, `output_schema` |
| `call` | `namespace`, `function` | `arguments`, `expected_version`, `idempotency_key` |
| `execute` | `script` | `idempotency_key`, `inputs`, `namespace` |
| `describe` | — | `mode` |
| `describe` | `tool` | — |
| `describe` | `namespace` | `function` |

For `fs.write` and `fs.append`, supply exactly one of `text` or `base64`.
For `describe`, choose one row's argument shape; selectors cannot be mixed.

## Results

These are application results before the MCP transport envelope. Mutation
results may additionally include `revision` as described above.

| Tools | Result |
| --- | --- |
| `namespace.create`, `namespace.get`, `namespace.update`, `namespace.copy` | `{id, name, description, revision, files, databases, functions}`; the last three fields are counts. Nested create/update/copy omit `revision`. |
| `namespace.list` | `{namespaces: [namespace metadata]}` |
| `namespace.delete` | `{deleted: true, id}` |
| `fs.read` | `{path, kind: "file", size, text}` or `{path, kind: "file", size, base64}` for non-UTF-8 bytes. |
| `fs.stat` | `{path, kind: "file", size, hash}` or `{path, kind: "directory"}`. |
| `fs.list` | `{entries: [{path, kind}]}` |
| `fs.write`, `fs.append` | `{path, size}` |
| `fs.copy`, `fs.move` | `{path}`; path is the destination. |
| `fs.delete` | `{deleted: integer}`; number of files removed. |
| `db.create` | `{name, created: true}` |
| `db.list` | `{databases: [{name, id, migrations}]}` |
| `db.drop` | `{dropped: true}` |
| `db.query`, `db.execute` | `{columns: [string], rows: [[value]], rows_affected: integer}` |
| `db.migrations` | `{migrations: [{id, checksum}]}` |
| `db.migrate` | `{applied: integer, migrations: [{id, checksum}]}` |
| `db.inspect` | `{name, id, snapshot, staged, schema_fingerprint, migrations, schema, tables}`; includes columns, indexes, and foreign keys. |
| `function.declare`, `function.update` | `{name, published: true, version}` |
| `function.get` | Full declaration: name, file, symbol, schemas, version, pinned source, module sources, source hash, and ABI version; description if supplied. |
| `function.list` | `{functions: [declaration metadata]}`; omits source, source hash, database IDs, and ABI version. |
| `function.remove` | `{removed: true}` |
| `call` | The function's JSON return value. |
| `execute` | `{value, stdout}`; value is the script's final expression. |
| `describe` | Overview, runtime guide, schemas, README, tool schema, or endpoint contracts, depending on selectors. |

## Tool specifications

Each input schema is the complete schema advertised by the server. Field types,
required arguments, alternatives, and nested grant shapes are specified here.

### `namespace.create`

Create an empty namespace with an optional description for files, SQLite databases, and published functions.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "description": {
      "type": [
        "string",
        "null"
      ]
    },
    "name": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "name"
  ]
}
```

### `namespace.list`

Primary entry point for discovering what is available in StateMCP. List namespaces with their names, descriptions, stable IDs, revisions, and resource counts. Then use function.list to discover published functions in the relevant namespace.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "additionalProperties": false
}
```

### `namespace.get`

Get a namespace’s name, description, stable ID, revision, and resource counts.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `namespace.update`

Update a namespace name or description while preserving its stable ID and contents. Omitted description is preserved; null clears it.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "description": {
      "type": [
        "string",
        "null"
      ]
    },
    "expected_revision": {
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `namespace.copy`

Copy a namespace and its contents to a new name. Subsequent changes are independent.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "description": {
      "type": [
        "string",
        "null"
      ]
    },
    "expected_revision": {
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "name",
    "namespace"
  ]
}
```

### `namespace.delete`

Delete a namespace and its contents from active use. Its name remains reserved.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `fs.read`

Read a virtual file as UTF-8 text or base64 for binary data.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace",
    "path"
  ]
}
```

### `fs.stat`

Get a virtual file’s metadata or check whether a virtual directory exists.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace",
    "path"
  ]
}
```

### `fs.list`

List files and directories directly beneath a virtual path.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `fs.write`

Create or replace a virtual file. Supply either text or base64.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "anyOf": [
    {
      "$ref": "#/$defs/FileTextInput"
    },
    {
      "$ref": "#/$defs/FileBase64Input"
    }
  ],
  "$defs": {
    "FileBase64Input": {
      "type": "object",
      "properties": {
        "base64": {
          "type": "string"
        },
        "expected_revision": {
          "type": "string"
        },
        "namespace": {
          "type": "string"
        },
        "path": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "base64",
        "namespace",
        "path"
      ]
    },
    "FileTextInput": {
      "type": "object",
      "properties": {
        "expected_revision": {
          "type": "string"
        },
        "namespace": {
          "type": "string"
        },
        "path": {
          "type": "string"
        },
        "text": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "namespace",
        "path",
        "text"
      ]
    }
  }
}
```

### `fs.append`

Append text or base64-encoded bytes to a virtual file.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "anyOf": [
    {
      "$ref": "#/$defs/FileTextInput"
    },
    {
      "$ref": "#/$defs/FileBase64Input"
    }
  ],
  "$defs": {
    "FileBase64Input": {
      "type": "object",
      "properties": {
        "base64": {
          "type": "string"
        },
        "expected_revision": {
          "type": "string"
        },
        "namespace": {
          "type": "string"
        },
        "path": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "base64",
        "namespace",
        "path"
      ]
    },
    "FileTextInput": {
      "type": "object",
      "properties": {
        "expected_revision": {
          "type": "string"
        },
        "namespace": {
          "type": "string"
        },
        "path": {
          "type": "string"
        },
        "text": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "namespace",
        "path",
        "text"
      ]
    }
  }
}
```

### `fs.move`

Move a virtual file to another path in the same namespace.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "destination": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "destination",
    "namespace",
    "path"
  ]
}
```

### `fs.copy`

Copy a virtual file to another path in the same namespace.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "destination": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "destination",
    "namespace",
    "path"
  ]
}
```

### `fs.delete`

Delete a virtual file, or delete a directory’s contents with recursive=true.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "path": {
      "type": "string"
    },
    "recursive": {
      "type": "boolean"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace",
    "path"
  ]
}
```

### `db.list`

List the named SQLite databases in a namespace.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `db.create`

Create an empty named SQLite database in a namespace.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace"
  ]
}
```

### `db.drop`

Drop a named SQLite database. Remove function declarations that reference it first.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace"
  ]
}
```

### `db.query`

Run one read-only SQL statement with optional bound parameters. Returns columns, rows, and rows_affected.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "params": {
      "type": "array",
      "items": {
        "$ref": "#/$defs/SqlCell"
      }
    },
    "sql": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace",
    "sql"
  ],
  "$defs": {
    "BinaryCell": {
      "type": "object",
      "properties": {
        "$base64": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "$base64"
      ]
    },
    "IntegerCell": {
      "type": "object",
      "properties": {
        "$integer": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "$integer"
      ]
    },
    "SqlCell": {
      "description": "SQLite has a fixed set of cell types; tagged values preserve binary data and large integers.",
      "anyOf": [
        {
          "type": "null"
        },
        {
          "type": "boolean"
        },
        {
          "type": "integer",
          "format": "int64"
        },
        {
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        {
          "type": "number",
          "format": "double"
        },
        {
          "type": "string"
        },
        {
          "$ref": "#/$defs/BinaryCell"
        },
        {
          "$ref": "#/$defs/IntegerCell"
        }
      ]
    }
  }
}
```

### `db.execute`

Run one SQL statement with optional bound parameters, including writes and RETURNING. Returns columns, rows, and rows_affected.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "params": {
      "type": "array",
      "items": {
        "$ref": "#/$defs/SqlCell"
      }
    },
    "sql": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace",
    "sql"
  ],
  "$defs": {
    "BinaryCell": {
      "type": "object",
      "properties": {
        "$base64": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "$base64"
      ]
    },
    "IntegerCell": {
      "type": "object",
      "properties": {
        "$integer": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "$integer"
      ]
    },
    "SqlCell": {
      "description": "SQLite has a fixed set of cell types; tagged values preserve binary data and large integers.",
      "anyOf": [
        {
          "type": "null"
        },
        {
          "type": "boolean"
        },
        {
          "type": "integer",
          "format": "int64"
        },
        {
          "type": "integer",
          "format": "uint64",
          "minimum": 0
        },
        {
          "type": "number",
          "format": "double"
        },
        {
          "type": "string"
        },
        {
          "$ref": "#/$defs/BinaryCell"
        },
        {
          "$ref": "#/$defs/IntegerCell"
        }
      ]
    }
  }
}
```

### `db.inspect`

Inspect a database’s tables, columns, indexes, foreign keys, schema, migration history, and storage metadata.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace"
  ]
}
```

### `db.migrations`

Get the ordered IDs and checksums of applied database migrations.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "namespace"
  ]
}
```

### `db.migrate`

Apply an ordered batch of SQL migrations atomically. Previously applied IDs must match their stored checksums and order.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "database": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "migrations": {
      "type": "array",
      "items": {
        "$ref": "#/$defs/MigrationInput"
      }
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "database",
    "migrations",
    "namespace"
  ],
  "$defs": {
    "MigrationInput": {
      "type": "object",
      "properties": {
        "id": {
          "type": "string"
        },
        "sql": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "id",
        "sql"
      ]
    }
  }
}
```

### `function.list`

List published functions in the required namespace, selected by name or stable UUID. Returns names, versions, input/output contracts without source code. Use function.get with namespace and name for a full declaration.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "namespace"
  ]
}
```

### `function.get`

Get one published function by namespace and name. Returns its pinned Python source, symbol, input/output schemas, version, and diagnostic metadata. Supply expected_version to reject a stale lookup. Editing the source file does not change the published function until it is declared or updated again.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "expected_version": {
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "name",
    "namespace"
  ]
}
```

### `function.remove`

Remove a published function by namespace and name. Its source file and data remain available. Optional expected_version and expected_revision reject changes made since the function or namespace was inspected.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "expected_revision": {
      "type": "string"
    },
    "expected_version": {
      "type": "string"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "name",
    "namespace"
  ]
}
```

### `function.declare`

Publish a Python function by specifying namespace, name, file, and symbol. First write the source with fs.write and create any databases it needs. Arguments supplied to call become keyword arguments to the Python symbol; its return value must be JSON-compatible. Input/output schemas validate calls. Functions have full access to their own namespace and cannot access other namespaces. Source bytes are pinned at publication. Returns name, version, and published status, plus the committed revision for direct calls.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "description": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "expected_version": {
      "type": "string"
    },
    "file": {
      "type": "string"
    },
    "input_schema": {
      "$ref": "#/$defs/EndpointSchema"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "output_schema": {
      "$ref": "#/$defs/EndpointSchema"
    },
    "symbol": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "file",
    "name",
    "namespace",
    "symbol"
  ],
  "$defs": {
    "EndpointSchema": {
      "anyOf": [
        {
          "type": "object",
          "additionalProperties": true
        },
        {
          "type": "boolean"
        }
      ]
    }
  }
}
```

### `function.update`

Publish a new function declaration for namespace and name using the current contents of file and its Python symbol. Supply the complete schemas. The function has full access within its own namespace only. Use expected_version to reject a stale update. Returns the published function version; later source-file edits do not affect this version.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "description": {
      "type": "string"
    },
    "expected_revision": {
      "type": "string"
    },
    "expected_version": {
      "type": "string"
    },
    "file": {
      "type": "string"
    },
    "input_schema": {
      "$ref": "#/$defs/EndpointSchema"
    },
    "name": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    },
    "output_schema": {
      "$ref": "#/$defs/EndpointSchema"
    },
    "symbol": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "file",
    "name",
    "namespace",
    "symbol"
  ],
  "$defs": {
    "EndpointSchema": {
      "anyOf": [
        {
          "type": "object",
          "additionalProperties": true
        },
        {
          "type": "boolean"
        }
      ]
    }
  }
}
```

### `call`

Invoke a published function using namespace, function, and an arguments object matching its input schema. Returns the function's JSON value directly. The function has full access within its own namespace only; nested calls and writes share one transaction and failures roll back changes. expected_version guards against calling changed code. Reuse an idempotency_key only with the identical request to replay a completed result without repeating writes.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "arguments": {
      "type": "object",
      "additionalProperties": true
    },
    "expected_version": {
      "type": "string"
    },
    "function": {
      "type": "string"
    },
    "idempotency_key": {
      "type": "string"
    },
    "namespace": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "function",
    "namespace"
  ]
}
```

### `execute`

Run a Python script with owner access in one transaction. The inputs argument is available as the Python variable inputs; the final expression and printed output are returned as {value, stdout}. Set namespace to use db_query(database, sql, params=[]), db_execute(database, sql, params=[]), db_inspect(database), read_text(path), and write_text(path, text). SQL helpers return {columns, rows, rows_affected}. Use mcp(name, arguments={}) for any tool or call(namespace, function, arguments={}) for a published function. Failures roll back all nested writes. Imports load Python files and packages from the selected namespace, with per-run module caching. Published functions use pinned module sources. Reuse an idempotency_key only for an identical request.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "properties": {
    "idempotency_key": {
      "type": "string"
    },
    "inputs": true,
    "namespace": {
      "type": "string"
    },
    "script": {
      "type": "string"
    }
  },
  "additionalProperties": false,
  "required": [
    "script"
  ]
}
```

### `describe`

Get a compact API overview, mode=runtime for a Python authoring guide, or mode=full for all schemas, or mode=readme for the bundled documentation. Select tool for one tool’s schema, or namespace and optional function for endpoint contracts.

```json
{
  "$schema": "https://json-schema.org/draft/2020-12/schema",
  "type": "object",
  "anyOf": [
    {
      "$ref": "#/$defs/DiscoveryOverviewInput"
    },
    {
      "$ref": "#/$defs/DiscoveryToolInput"
    },
    {
      "$ref": "#/$defs/DiscoveryNamespaceInput"
    }
  ],
  "$defs": {
    "DiscoveryMode": {
      "type": "string",
      "enum": [
        "overview",
        "full",
        "runtime",
        "readme"
      ]
    },
    "DiscoveryNamespaceInput": {
      "type": "object",
      "properties": {
        "function": {
          "type": "string"
        },
        "namespace": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "namespace"
      ]
    },
    "DiscoveryOverviewInput": {
      "type": "object",
      "properties": {
        "mode": {
          "$ref": "#/$defs/DiscoveryMode"
        }
      },
      "additionalProperties": false
    },
    "DiscoveryToolInput": {
      "type": "object",
      "properties": {
        "tool": {
          "type": "string"
        }
      },
      "additionalProperties": false,
      "required": [
        "tool"
      ]
    }
  }
}
```

## Writing tools with Python

Published functions receive `call.arguments` as keyword arguments and must
return JSON-compatible values. `execute` scripts receive `inputs` as a Python
variable; their final expression and captured print output become `{value, stdout}`.
Helpers and imports use the function's namespace or `execute.namespace`.
Module initialization cannot call host APIs.

| Python helper | Result |
| --- | --- |
| `mcp(name, arguments={})` | The selected state tool result; namespace isolation still applies. |
| `call(namespace, function, arguments={})` | Endpoint JSON value. |
| `db_query(database, sql, params=[])` | {columns: [string], rows: [[value]], rows_affected: integer}; one read-only statement. |
| `db_execute(database, sql, params=[])` | {columns: [string], rows: [[value]], rows_affected: integer}; one statement, supports RETURNING. |
| `db_inspect(database)` | Database name, id, snapshot, staged flag, schema_fingerprint, migrations, schema query result, and tables with columns/indexes/foreign_keys. |
| `read_text(path)` | UTF-8 string. |
| `write_text(path, text)` | File write metadata. |

### Python imports

Write `/api.py` and `/helpers.py` in the same namespace, then use ordinary imports:

```python
import helpers
from helpers import format_issue as format_item

def endpoint(title):
    return format_item(title)
```

The entry file's directory is searched first, followed by the namespace root.
Packages use `__init__.py`; namespace packages, dotted imports, relative imports,
and `from module import *` are supported. Wildcard imports respect `__all__`.
Modules have separate globals and execute once per interpreter session; repeated
and circular imports share the same module object. Missing modules raise
`ModuleNotFoundError`. Importing cannot access another namespace or host files.
Monty's built-in standard-library modules remain available; installing third-party
packages is not supported.

Published functions use the Python sources captured at publication, including
imports inside function bodies. Update the declaration to pick up helper edits.
`execute` uses the current transaction's files. Imported helpers can use the same
host APIs as their caller; publication initialization still forbids host effects.
The complete Python source snapshot is capped at the runtime source limit (1 MiB
by default). Imported globals reset on each tool invocation.

### Namespace scope and publication

Published functions can read and write every file and database in their namespace,
create or drop databases, publish functions, and call any function in that namespace.
There are no per-resource grants. `self` resolves to the current namespace and
follows namespace renames and copies. Explicit names and UUIDs must resolve to
that same namespace. Cross-namespace calls and state access are rejected.

Nested `execute` scripts inherit the function's namespace scope; they cannot
obtain owner access. Functions cannot create, copy, or list namespaces. Root MCP
calls and root scripts have owner access across namespaces.

Default `input_schema` is `{"type":"object"}`; default `output_schema` is `true`.
Schemas may be objects or booleans. References must be local fragments;
remote/file references and `$id` resources are unsupported.

Publication pins the entry file and a snapshot of the namespace's Python files.
Editing a helper alone does not change a published function: call
`function.update` to publish its current sources. Updates take the
complete declaration.
Runtime ABI is 1; unsupported ABI versions fail with `ABI_MISMATCH`.

### SQL and storage

`db.query` accepts one read-only statement. `db.execute` accepts one statement,
including writes and `RETURNING`. `params` defaults to `[]` and supports JSON
scalars, `{"$base64":"..."}` for blobs, and `{"$integer":"..."}` for signed
64-bit integers. Rows are arrays in the order of `columns`.

`db.migrate` applies an ordered batch of `{id, sql}` migrations atomically.
Previously applied IDs must match stored checksums and order. Each named database
is a separate SQLite file; application SQL cannot access the private catalog.

`db.inspect.snapshot` is an opaque backing database file identity, not a
concurrency token or content hash. `staged: true` reports pending writes in the
current invocation. Namespace copies use independent files. There is no
historical read/restore API using this identity.

### Working example

Make these MCP tool calls in order. The final call returns `[1, "hello"]`.

**`namespace.create`**

```json
{
  "name": "demo"
}
```

**`db.create`**

```json
{
  "database": "app",
  "namespace": "demo"
}
```

**`db.execute`**

```json
{
  "database": "app",
  "namespace": "demo",
  "sql": "CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT)"
}
```

**`fs.write`**

```json
{
  "namespace": "demo",
  "path": "/api.py",
  "text": "def add(text):\n    return db_execute('app', 'INSERT INTO notes(text) VALUES (?) RETURNING id, text', [text])['rows'][0]\n"
}
```

**`function.declare`**

```json
{
  "file": "/api.py",
  "input_schema": {
    "additionalProperties": false,
    "properties": {
      "text": {
        "type": "string"
      }
    },
    "required": [
      "text"
    ],
    "type": "object"
  },
  "name": "add",
  "namespace": "demo",
  "symbol": "add"
}
```

**`call`**

```json
{
  "arguments": {
    "text": "hello"
  },
  "function": "add",
  "namespace": "demo"
}
```

## CLI and live discovery

The CLI uses the same 30 tool names as MCP. Each command accepts its own named
flags or `--json`, followed by a required data directory.

```sh
statemcp cli db.query ./data --namespace demo --database app --sql 'SELECT * FROM notes'
statemcp cli db.query ./data --json '{"namespace":"demo","database":"app","sql":"SELECT * FROM notes"}'
```


Use `describe({"tool":"db.query"})` for one exact tool schema,
`describe({"namespace":"demo","function":"add"})` for an endpoint contract,
and `describe({"mode":"runtime"})` for the Python authoring guide.

The canonical schemas and runtime guide are in
[schemas.rs](crates/state-core/src/schemas.rs). Storage behavior is documented
in [state-domain](crates/state-domain/README.md).
