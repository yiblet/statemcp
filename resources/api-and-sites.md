# HTTP APIs and sites in StateMCP

Read this resource before creating or editing HTTP routes, Python handlers,
HTML pages, or static sites. Use the primitives creatively to satisfy the task
with a small implementation. Reuse existing namespace data and functions.

## Keep MCP payloads small

MCP is a control and authoring interface, not a bulk asset transport. Avoid large
request and response payloads: generated bundles, library source, base64 images,
fonts, embedded data dumps, or full dependency trees. Write concise application
source and templates with `fs.write`. Split application files by purpose and edit
only what changed; do not work around payload limits by sending a library in chunks.
Return compact acknowledgments and inspect selected files or paginated records.
Let browsers load pages, assets, and data through HTTP routes rather than putting
them into MCP results. Keep list APIs paginated and HTML fragments focused.

## Avoid N+1 state calls

HTTP handlers share a fixed operation count limit with every nested function
and state call. As a result, all endpoints should aim for a fixed number of queries
per request, independent of the number of records processed or returned.
Batch SQL reads and aggregates, reuse fetched data, and
paginate results rather than issuing queries or function calls per record.
Exceeding the budget rolls back the request and returns `LIMIT_EXCEEDED` as HTTP
504, even when execution is fast. See `statemcp://guides/python` for the budget
contract and `describe` for the current `limits.root_calls`.

## Let the browser load dependencies from CDNs

When a browser library or stylesheet helps, reference its CDN URL directly with
`<script src="...">`, `<link rel="stylesheet" href="...">`, or an ES module import.
Prefer version-pinned URLs. Do not download and re-upload a library, inline its
source into HTML, or introduce npm, bundling, or packaging merely to use it.
Keep namespace files for the application's own small handlers, templates, CSS,
and JavaScript. Use self-hosted dependencies or a build step when the user's
requirements, offline operation, or deployment constraints actually need them.

## Simple patterns that work

- **Server-rendered HTML and forms:** Python handlers receive a request and return
  HTML or JSON. Namespace SQLite and existing published functions own the state
  and rules. Ordinary forms or a little `fetch` code are often enough.
- **HTMX:** load it from a CDN and return small HTML fragments from handlers.
  `hx-get` loads a view; `hx-post` submits an edit; `hx-target` chooses the element
  to replace. This supports search, editors, and partial updates without a SPA
  framework or client-side copy of domain logic. See [HTMX docs](https://htmx.org/docs/).
- **Tailwind:** utility classes are a concise styling option. The browser CDN
  provides a buildless path for prototypes and small tools. Tailwind documents
  Play CDN as development-only; for production requirements choose a small plain
  stylesheet or compiled CSS instead of assuming the browser compiler is suitable.
  See [Tailwind Play CDN](https://tailwindcss.com/docs/installation/play-cdn).
- **Small native scripts:** use browser APIs for a dialog, focus management, or a
  simple JSON interaction. Add a framework only when the interaction needs it.

For example, HTMX can fetch a server-rendered fragment without packaging a library:

```html
<script src="https://cdn.jsdelivr.net/npm/htmx.org@2.0.11/dist/htmx.min.js"></script>
<button hx-get="api/items" hx-target="#items">Refresh</button>
<div id="items"></div>
```

Use a current, compatible, version-pinned CDN release when authoring a new site;
the example is a pattern, not a requirement to keep that particular release.

## Publish and connect the pieces

1. Discover existing namespace functions before writing new rules.
2. Write small Python handlers and application files with `fs.write`.
3. Publish HTTP handlers with `api.publish`, selecting methods and a namespace-relative
   path. Return HTML fragments for HTMX or compact JSON for browser scripts.
4. For static pages, mount only their directory with a static `api.publish` target.
5. Use publication URLs, including any HTTP server prefix, for requests and links.
   Relative URLs work when deliberately resolved against the page's location.
6. Verify view and edit flows, escaping, validation failures, revisions, and rollback.

Handlers receive one positional Request. Query values are in `request.args`, form
values in `request.form`, route captures in `request.view_args`, and JSON in
`request.json`. Escape user-controlled text in HTML. Keep concurrency guards and
transactional business rules in existing functions. Handler sources and imports
are pinned at publication; republish after changing them. Static files remain live.
Use `describe({"tool":"api.publish"})` for the exact schema and
`describe({"mode":"readme"})` for the detailed HTTP contract.

## Bulk assets and external URLs

Use HTTP MCP `fs.upload` for assets instead of large base64 MCP payloads. Pass
`namespace` and `files` (literal paths); send a multipart POST to the returned URL
with file field names exactly matching the returned normalized paths. Multiple
files commit together. The request accepts up to 10 MiB of file data. `fs.download`
accepts exact paths or glob selectors and streams a tar archive from its GET URL.
Both URLs are single-use and expire after ten minutes; failed redemption consumes
the token. These tools are unavailable in stdio mode.

Configure `--public-url https://example.com/statemcp`, or `--public-url /statemcp`
for relative URLs. Publication URLs include this prefix and optional origin.
Python can inspect `config().public_url` and `config().prefix` to construct links.
