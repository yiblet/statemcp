#!/usr/bin/env bash
# Exercise the complete fixed API against an isolated, temporary HTTP server.
set -euo pipefail
script_dir=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)
if [[ ${1:-} == --help ]]; then
    echo "Usage: $0 [OUTPUT.jsonl]"
    echo 'Builds statemcp, starts an isolated localhost server, and records all requests.'
    exit 0
fi
[[ $# -le 1 ]] || { echo "Usage: $0 [OUTPUT.jsonl]" >&2; exit 2; }
for dependency in curl jq cargo; do
    command -v "$dependency" >/dev/null || { echo "Missing dependency: $dependency" >&2; exit 2; }
done
output=${1:-"$script_dir/curl-results.jsonl"}
# Open before starting the server, so a bad output path fails immediately.
: > "$output"
work=$(mktemp -d)
: > "$work/covered.jsonl"
server_pid=''
cleanup() {
    if [[ -n $server_pid ]]; then
        kill "$server_pid" 2>/dev/null || true
        wait "$server_pid" 2>/dev/null || true
    fi
    rm -rf -- "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
"$script_dir/build.sh" >&2
"$script_dir/statemcp" http "$work/data" --bind 127.0.0.1:0 >"$work/server.log" 2>&1 &
server_pid=$!
url=''
for ((attempt=0; attempt<100; attempt++)); do
    url=$(sed -n 's/.*url=\(http[^ ]*\).*/\1/p' "$work/server.log" | head -n 1)
    [[ -z $url ]] || break
    if ! kill -0 "$server_pid" 2>/dev/null; then cat "$work/server.log" >&2; exit 1; fi
    sleep 0.1
done
[[ -n $url ]] || { cat "$work/server.log" >&2; echo 'Server startup timed out' >&2; exit 1; }
session=''
request_id=0
failures=0
count=0
response=null
# expected is success, notification, a tool error code, or rpc:<JSON-RPC code>.
request() {
    local label=$1 payload=$2 expected=${3:-success} status curl_exit=0 passed=false
    local -a headers=(-H 'Content-Type: application/json' -H 'Accept: application/json, text/event-stream' -H 'MCP-Protocol-Version: 2025-06-18')
    [[ -z $session ]] || headers+=(-H "Mcp-Session-Id: $session")
    : > "$work/body"
    : > "$work/headers"
    jq -cn --argjson message "$payload" '{type:"request",message:$message}' >> "$output"
    status=$(curl --silent --show-error --connect-timeout 3 --max-time 15 \
        -D "$work/headers" -o "$work/body" -w '%{http_code}' \
        "${headers[@]}" --data-binary "$payload" "$url" 2>"$work/curl-error") || curl_exit=$?
    # Decode the JSON-RPC response.
    response=$(jq -Rs 'if length == 0 then null else
        try fromjson catch (split("\n") | map(select(startswith("data:")) | ltrimstr("data:") | try fromjson catch empty) | map(select(has("id"))) | first // null)
        end' "$work/body")
    if [[ $curl_exit == 0 ]]; then
        case "$expected" in
            notification) [[ $status == 202 ]] && passed=true ;;
            success) if [[ $status == 200 ]] && jq -e '.result != null and .error == null and (.result.isError != true)' <<<"$response" >/dev/null; then passed=true; fi ;;
            rpc:*) if [[ $status == 200 || $status == 400 ]] && jq -e --arg code "${expected#rpc:}" '.error.code == ($code|tonumber)' <<<"$response" >/dev/null; then passed=true; fi ;;
            *) if [[ $status == 200 ]] && jq -e --arg code "$expected" '.result.isError == true and (.result.content[0].text | fromjson).error.code == $code' <<<"$response" >/dev/null; then passed=true; fi ;;
        esac
    fi
    if [[ $expected != notification ]] && ! jq -e --argjson request "$payload" ' .jsonrpc == "2.0" and .id == $request.id' <<<"$response" >/dev/null; then
        passed=false
    fi
    if [[ $expected == success ]] && [[ $(jq -r .method <<<"$payload") == tools/call ]] && ! jq -e '.result | has("content") and (has("structuredContent") | not)' <<<"$response" >/dev/null; then
        passed=false
    fi
    if [[ $response != null ]]; then
        jq -cn --argjson message "$response" '{type:"response",message:$message}' >> "$output"
    fi
    if [[ $passed == true && $expected == success ]]; then
        jq -c 'select(.method == "tools/call") | .params.name' <<<"$payload" >> "$work/covered.jsonl"
    fi
    count=$((count+1))
    if [[ $passed != true ]]; then
        failures=$((failures+1))
        echo "FAIL: $label (expected $expected, HTTP $status)" >&2
        cat "$work/curl-error" >&2
    fi
}
rpc() {
    local label=$1 method=$2 params=$3 expected=${4:-success}
    request_id=$((request_id+1))
    request "$label" "$(jq -cn --argjson id "$request_id" --arg method "$method" --argjson params "$params" '{jsonrpc:"2.0",id:$id,method:$method,params:$params}')" "$expected"
}
tool() {
    local label=$1 name=$2 arguments=$3 expected=${4:-success}
    rpc "$label" tools/call "$(jq -cn --arg name "$name" --argjson arguments "$arguments" '{name:$name,arguments:$arguments}')" "$expected"
}
# Validate returned data as well as transport status, without adding log metadata.
result() { jq -c '.result.content[0].text | fromjson' <<<"$response"; }
assert_result() {
    local label=$1 predicate=$2
    if ! result | jq -e "$predicate" >/dev/null; then
        failures=$((failures+1))
        echo "FAIL: $label (unexpected result: $(result))" >&2
    fi
}
# MCP session lifecycle and fixed tool discovery.
rpc initialize initialize '{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"curl-all","version":"1"}}'
session=$(awk 'tolower($1)=="mcp-session-id:" {gsub("\r", "", $2); print $2}' "$work/headers")
[[ -n $session ]] || { echo 'No MCP session returned; see JSONL' >&2; exit 1; }
request initialized '{"jsonrpc":"2.0","method":"notifications/initialized"}' notification
rpc ping ping '{}'
rpc tools.list tools/list '{}'
printf '%s\n' "$response" > "$work/tools.json"
tool describe.overview describe '{}'
tool describe.full describe '{"mode":"full"}'
tool describe.runtime describe '{"mode":"runtime"}'
tool describe.readme describe '{"mode":"readme"}'
assert_result readme '.format == "markdown" and (.text | startswith("# StateMCP")) and (.text | contains("reference.md"))'
tool describe.tool describe '{"tool":"execute"}'
tool describe.action describe '{"tool":"db.create"}'

# Namespaces and virtual files, including text and binary variants.
tool namespace.create namespace.create '{"name":"curl-demo"}'
tool namespace.list namespace.list '{}'
tool namespace.get namespace.get '{"namespace":"curl-demo"}'
revision=$(jq -r '.result.content[0].text | fromjson | .revision' <<<"$response")
tool namespace.update namespace.update "$(jq -cn --arg revision "$revision" '{namespace:"curl-demo",name:"curl-renamed",expected_revision:$revision}')"
assert_result namespace.rename ".revision != \"$revision\""
tool fs.write fs.write '{"namespace":"curl-renamed","path":"/notes/message.txt","text":"hello"}'
tool fs.append fs.append '{"namespace":"curl-renamed","path":"/notes/message.txt","text":" world"}'
tool fs.read fs.read '{"namespace":"curl-renamed","path":"/notes/message.txt"}'
assert_result text.read '.text == "hello world" and .hash == null'
tool fs.stat fs.stat '{"namespace":"curl-renamed","path":"/notes/message.txt"}'
assert_result file.stat '(.hash | type) == "string"'
tool fs.list fs.list '{"namespace":"curl-renamed","path":"/notes"}'
tool fs.copy fs.copy '{"namespace":"curl-renamed","path":"/notes/message.txt","destination":"/notes/copy.txt"}'
tool fs.move fs.move '{"namespace":"curl-renamed","path":"/notes/copy.txt","destination":"/notes/moved.txt"}'
tool fs.write.binary fs.write '{"namespace":"curl-renamed","path":"/binary.bin","base64":"/wA="}'
tool fs.append.binary fs.append '{"namespace":"curl-renamed","path":"/binary.bin","base64":"AQ=="}'
tool fs.read.binary fs.read '{"namespace":"curl-renamed","path":"/binary.bin"}'
assert_result binary.read '.base64 == "/wAB" and .size == 3'
tool fs.delete fs.delete '{"namespace":"curl-renamed","path":"/notes/moved.txt"}'

# SQL, parameter binding, migrations and inspection.
tool db.create db.create '{"namespace":"curl-renamed","database":"app"}'
tool db.list db.list '{"namespace":"curl-renamed"}'
tool db.migrate db.migrate '{"namespace":"curl-renamed","database":"app","migrations":[{"id":"001","sql":"CREATE TABLE notes(id INTEGER PRIMARY KEY, text TEXT NOT NULL)"}]}'
tool db.execute db.execute '{"namespace":"curl-renamed","database":"app","sql":"INSERT INTO notes(text) VALUES (?) RETURNING id","params":["from curl"]}'
tool db.query db.query '{"namespace":"curl-renamed","database":"app","sql":"SELECT * FROM notes WHERE id > ?","params":[0]}'
tool db.inspect db.inspect '{"namespace":"curl-renamed","database":"app"}'
tool db.migrations db.migrations '{"namespace":"curl-renamed","database":"app"}'

# Published endpoints, schemas, namespace scope, version guards and idempotency.
source=$'def add(text):\n    return db_execute("app", "INSERT INTO notes(text) VALUES (?) RETURNING id, text", [text])["rows"][0]\n'
tool function.source fs.write "$(jq -cn --arg source "$source" '{namespace:"curl-renamed",path:"/api.py",text:$source}')"
declaration='{"namespace":"curl-renamed","name":"add","file":"/api.py","symbol":"add","input_schema":{"type":"object","properties":{"text":{"type":"string"}},"required":["text"],"additionalProperties":false},"output_schema":{"type":"array"}}'
tool function.declare function.declare "$declaration"
version=$(jq -r '.result.content[0].text | fromjson | .version' <<<"$response")
tool function.get function.get "$(jq -cn --arg version "$version" '{namespace:"curl-renamed",name:"add",expected_version:$version}')"
tool function.list function.list '{"namespace":"curl-renamed"}'
tool describe.namespace describe '{"namespace":"curl-renamed"}'
tool describe.function describe '{"namespace":"curl-renamed","function":"add"}'
call_args=$(jq -cn --arg version "$version" '{namespace:"curl-renamed",function:"add",arguments:{text:"endpoint"},expected_version:$version,idempotency_key:"curl-add"}')
tool call call "$call_args"
call_result=$(result)
assert_result call ' . == [2,"endpoint"]'
tool call.replay call "$call_args"
assert_result call.replay ". == $call_result"
tool function.update function.update "$(jq -c --arg version "$version" '.description="Updated through curl" | .expected_version=$version' <<<"$declaration")"
tool execute execute '{"namespace":"curl-renamed","script":"print(\"curl script\")\n{\"input\": inputs, \"notes\": db_query(\"app\", \"SELECT * FROM notes\")[\"rows\"]}","inputs":{"example":true},"idempotency_key":"curl-script"}'
execute_result=$(result)
tool execute.replay execute '{"namespace":"curl-renamed","script":"print(\"curl script\")\n{\"input\": inputs, \"notes\": db_query(\"app\", \"SELECT * FROM notes\")[\"rows\"]}","inputs":{"example":true},"idempotency_key":"curl-script"}'
assert_result execute.replay ". == $execute_result"
tool namespace.copy namespace.copy '{"namespace":"curl-renamed","name":"curl-copy"}'
tool call.copy call '{"namespace":"curl-copy","function":"add","arguments":{"text":"copied namespace"}}'

tool copy.original db.query '{"namespace":"curl-renamed","database":"app","sql":"SELECT count(*) FROM notes"}'
assert_result copy.original '.rows == [[2]]'
tool copy.fork db.query '{"namespace":"curl-copy","database":"app","sql":"SELECT count(*) FROM notes"}'
assert_result copy.fork '.rows == [[3]]'

# Representative failures (expected errors count as passing requests).
tool error.not_found fs.read '{"namespace":"curl-renamed","path":"/missing"}' NOT_FOUND
tool error.invalid_argument db.query '{"namespace":"curl-renamed"}' INVALID_ARGUMENT
tool error.revision_conflict namespace.update '{"namespace":"curl-renamed","name":"unused","expected_revision":"stale"}' CONFLICT
tool error.version_conflict call '{"namespace":"curl-renamed","function":"add","expected_version":"stale","arguments":{"text":"no"}}' CONFLICT
tool error.schema call '{"namespace":"curl-renamed","function":"add","arguments":{"text":42}}' SCHEMA_VALIDATION
tool error.idempotency call '{"namespace":"curl-renamed","function":"add","arguments":{"text":"different"},"idempotency_key":"curl-add"}' IDEMPOTENCY_MISMATCH
rpc error.unknown_tool tools/call '{"name":"unknown","arguments":{}}' rpc:-32602

tool error.removed_databases function.declare '{"namespace":"curl-renamed","name":"invalid","file":"/api.py","symbol":"add","databases":[]}' INVALID_ARGUMENT
tool error.removed_calls function.declare '{"namespace":"curl-renamed","name":"invalid","file":"/api.py","symbol":"add","calls":[]}' INVALID_ARGUMENT

# Databases can be dropped while functions remain published.
tool db.drop db.drop '{"namespace":"curl-renamed","database":"app"}'
tool function.remove function.remove '{"namespace":"curl-renamed","name":"add"}'
tool fs.delete.recursive fs.delete '{"namespace":"curl-renamed","path":"/notes","recursive":true}'
tool namespace.delete.copy namespace.delete '{"namespace":"curl-copy"}'
tool namespace.delete namespace.delete '{"namespace":"curl-renamed"}'

# Compare exercised operations to live discovery so new API actions cannot be missed silently.
missing=$(jq -n --slurpfile discovery "$work/tools.json" --slurpfile covered "$work/covered.jsonl" '
    [$discovery[0].result.tools[].name] | unique as $required |
    $required - ($covered | unique)')
if [[ $missing != '[]' ]]; then echo "Missing API coverage: $missing" >&2; failures=$((failures+1)); fi
printf 'Recorded %s requests to %s (%s failures)\n' "$count" "$output" "$failures"
[[ $failures == 0 ]]
