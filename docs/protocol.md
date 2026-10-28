# Bridge protocol v1

The desktop discovers isolates using getVM/getIsolate and searches extensionRPCs for ext.flutter_inspector.call. Connect with the authenticated VM Service URI; preserve its token path. HTTP(S) URIs normalize to WS(S) and append /ws.

The service extension receives isolateId plus a string payload containing:

    {
      "controller": "opaque-controller-id",
      "method": "hello",
      "params": {}
    }

The extension returns an object with either result or error. hello returns protocol, sessionId, name, and providers (id, kind, name). The desktop requires protocol major 1.

| Method | Parameters / result |
| --- | --- |
| discover | No controller required; returns protocol, sessionId, name, busy. Does not claim or change a lease or expose captured data |
| hello | Acquire/reacquire the controller lease; returns discovery data |
| heartbeat | Renew lease; returns alive |
| detach | Release lease and forward all original held messages |
| events | after sequence; returns cursor, dropped count, events, pending decisions |
| body.read | id, offset; up to 64 KiB in base64, size, complete |
| intercept.rules | enabled, host, path, method, request, response, holdSeconds |
| network.decide | id, action (forward/drop/mock), optional method/url/headers/status/body/bodyBase64 |
| replay | provider Dio ID, Draft fields, `draftId` (tab), `id` (unique send); returns status and the completed exchange with `repeaterId` |
| replay.cancel | provider Dio ID and `id` of the active send |
| preferences.list | provider ID; typed key/value entries |
| preferences.set | provider, key, type, value |
| preferences.remove | provider, key |
| drift.tables | provider; schema objects |
| drift.columns | provider, table; table_xinfo metadata |
| drift.rows | provider, table, offset, filter, order, descending; rows/columns/total |
| drift.query | provider, sql; one SELECT/WITH statement, up to 1,000 rows |
| drift.execute | provider, sql; one explicit mutating statement |
| drift.insert | provider, table, values |
| drift.update | provider, table, identity, values |
| drift.delete | provider, table, identity |
| drift.apply | provider, table, changes; atomic batch, returns changed |
| drift.empty | provider, table, confirm equal to table; delete all rows, returns changed |

A `drift.apply` batch contains 1–200 changes and at most 2 MiB of serialized changes. Each change has `kind`: `insert` with `values` (an empty object uses defaults), `update` with full `identity`, `values` and optional `original`, or `delete` with full `identity`. `original` holds the previous edited-cell values as optimistic concurrency guards. Update/delete operations must affect exactly one row; any validation, constraint or concurrency failure rolls back the whole batch. Generated/unknown columns are rejected. Table-update notifications become visible after commit. The same methods work through built-in adapters and the optional Drift provider.

`drift.empty` preserves schema and follows SQLite foreign-key rules. Its `confirm` parameter must exactly match the selected table name. The desktop presents a confirmation explaining that filters and pagination do not limit this deletion.

Network events carry monotonically increasing sequence, kind=network, and a complete exchange snapshot. Exchange IDs are unique within a bridge session/provider. Body references carry id, size, complete, contentType. Bodies are retrieved after the SDK closes the corresponding capture file.

Headers are arrays of name/value pairs; duplicate header values survive the protocol. Request bodies are transformed application bytes; response bodies are the bytes returned by the underlying adapter, potentially already decompressed. Drift BLOB values are represented as {"base64":"…"}.

States are pending, held_request, held_response, complete, error, dropped, cancelled. Request and response holds share an exchange ID but are sequential. A hold is removed before completing its future; a late duplicate decision fails. Cancellation, expiry, or detach completes each hold at most once.

Only one controller owns an isolate at a time. Hold timeout defaults to 120 seconds, configurable from 1 to 600. Heartbeats renew every 5 seconds; the lease expires after 15 seconds. A missing controller disables new holds and forwards original buffered messages. Hot restart creates a new session rather than resuming decisions.

Native desktop bridges publish discovery.json inside their private flutter-inspector-* temporary directory. Fields: version=1, uri (authenticated loopback HTTP URL), isolate, name, sessionId, pid, updatedAt (Unix milliseconds). The file is atomically replaced every three seconds; descriptors older than fifteen seconds are ignored. Consumers must verify the live discover RPC rather than trusting descriptor metadata. Android discovery obtains the authenticated engine URL from ADB logcat and maps the device port to localhost; it uses the same verification RPC.

HAR preserves original captured text/bytes. Response base64 uses content.encoding. Request base64 uses postData._encoding because HAR 1.2 has no standardized binary request encoding. _inspector contains source, state, error, and body completeness markers.
