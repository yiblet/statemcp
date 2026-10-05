# StateMCP feedback

Based on creating a persistent chat room with a SQLite database and published
`send_message` and `list_messages` endpoints through the MCP tools.

## Discoverability

The model was straightforward once the tool descriptions and README were read:
namespaces contain virtual files, named SQLite databases, and callable Python
endpoints. Creating a chat room required composing these primitives; there was
no existing chat-room endpoint.

The main discovery gap was the endpoint runtime. Tool discovery explained how
to publish functions, but did not explain enough about the Python host APIs to
write one confidently. The local README supplied `db_execute`, `db_query`, their
result shapes, and the namespace isolation rules.

Recommendation: expose a compact runtime guide through discovery, including a
minimal working endpoint, available host APIs, return shapes, and namespace scope. Avoid
requiring access to the project's local README.

## Response size and repetition

- Function declaration responses repeated the full source file. Declaring two
  functions from the same file returned that same source twice, even though the
  caller had just supplied it. Default to a compact declaration acknowledgment;
  reserve source retrieval for an explicit get or verbose option.
- Results appeared in both text `content` and `structuredContent`, duplicating
  much of the output visible to the agent. If client compatibility requires both,
  consider whether the text representation can be a short summary rather than a
  second serialization of the entire result.
- `state_describe({})` returned every fixed tool's full schema. That is useful for
  complete discovery, but an overview mode would be easier to scan when only the
  conceptual model or a particular operation is needed.
- Source hashes, database UUIDs, snapshot IDs, and ABI versions added noise to
  routine creation responses. These fields are not inherently useless; their
  placement should reflect whether callers can act on them.

## Snapshot IDs: useful for inspection, rarely for routine creation

The README describes immutable snapshots shared by namespace copies, with a
full SQLite file copy on the first subsequent write. A snapshot ID can therefore
be useful to explain storage sharing, trace the exact stored database state in
diagnostics, and investigate copy-on-write or history-retention behavior.

It could also support reproducible reads, historical comparisons, or restoration
if public operations accepted snapshot IDs. The tool contracts inspected during
this task did not expose those operations, so these are potential uses, not
current capabilities established by this exercise.

A snapshot ID is not necessarily a content hash: different IDs must not be
interpreted as proof of different logical data without a documented guarantee.
It also should not be confused with a namespace revision used for optimistic
concurrency checks.

Recommendation: retain snapshot IDs in database inspection or diagnostic output,
with a documented meaning and lifecycle. Omit them from ordinary create/write
acknowledgments unless callers need to pass them into a supported next operation.
If exposed by default, make their actionable purpose clear.

## ABI versions: useful at compatibility boundaries

An ABI version can identify the contract between published code and its runtime:
for example, host API signatures, calling conventions, or result representation.
It can help diagnose why a function declared under one runtime behaves
differently or cannot run after an upgrade. It could also support compatibility
checks when moving persisted functions between installations, if that workflow
is supported.

These are reasons to preserve the metadata, not reasons to repeat it in every
successful declaration response. The observed output reported ABI version 1;
this exercise did not establish what compatibility guarantees that number makes
or how mismatches are handled.

Recommendation: expose the runtime ABI version once in server/runtime discovery.
Include a function's ABI version in detailed inspection when functions can be
pinned to different contracts. Report expected and actual versions in
compatibility errors. Document what changes increment the version, whether older
versions remain supported, and whether migration or redeclaration is required.
If every function necessarily uses the same ABI, repeating it per declaration
adds little information.

## Keep actionable identifiers

Namespace revisions and endpoint versions are useful because callers can pass
them as expected revisions or versions to protect against concurrent changes.
Stable namespace UUIDs also have a concrete role in references that must survive
renames. Source hashes can help verify which source was pinned, but usually
belong in inspection output rather than a routine acknowledgment.

A compact default response should return the resource identity, operation
outcome, and concurrency token needed for the next action. Detailed inspection
should retain the metadata needed for debugging and compatibility analysis.
