# Wi-Fi service v1 design

Status: revised after Claude Fable 5.2 review, 2026-11-16. The three library policy decisions below are approved. Protocol details and runtime prototypes remain pending. This document does not describe an implemented API.

GRKit is developing its first public Wi-Fi protocol. Replace the current development endpoints and clients together. Do not add legacy adapters or a v2 namespace.

## 1. Goals and boundaries

The phone manages Film Looks, recipes, and module settings during one Wi-Fi connection. Data changes become usable without a camera restart. Software updates still require camera confirmation and a restart.

The service exposes business resources over HTTP. A WebSocket reports resource changes and request results. There are no public generic upload or operation resources.

| Phone application | Camera service and runtime |
| --- | --- |
| Download the official catalog and release index | Read actual card contents and runtime state |
| Store a local library | Validate and persist camera changes |
| Convert LUT files to GRF | Load validated GRF effects |
| Render previews and report fitting limits | Coordinate changes with capture and menu activity |
| Select files and share content | Export looks and recipes |
| Assemble signed software packages | Verify signatures, install, and roll back |

BLE discovery, pairing, wake, and Wi-Fi association remain native phone functions. The HTTP service does not replace them. Remote shutter control, photo transfer, and live view are outside this design.

First delivery covers the protocol foundation, Film Looks, and LUT import. Recipes and module settings use the same contracts in later deliveries. Unimplemented capabilities remain unavailable.

## 2. Existing constraints and evidence

The current server processes connections serially. It exposes status, package, update, rollback, and diagnostics. It does not provide runtime management or WebSocket subscriptions.

The installer builds effect records, menu labels, and icons at startup. Repeating the full installer during capture is not a valid hot-loading implementation.

Existing effects can change their enabled state and order at runtime. An effect that has no loaded record currently needs another start. New and removed records need additional runtime support.

Current limits differ: the runtime loads at most 40 effects and lists at most 64. Effect names contain 1 to 16 printable ASCII characters. Capabilities report these limits separately.

The actual effect library is `GRKIT/EFFECTS`. `SCRIPT/GRKIT/EFFECTS` is the package inbox. `PACKAGE.TXT` describes package selections, not the complete live library.

The camera filesystem does not support ordinary Linux replacement semantics in all cases. Rename over an existing file can fail. On GR IV, Linux deletion can make later new filenames invisible until the next power-on.

Linux also caches negative lookups and can miss files created later by the RTOS. The current service reads directory entries to detect renamed journals. Do not assume desktop filesystem transactions or coherent cross-OS caches.

Resident state files already use the camera filesystem through the RTOS loader. Linux must not directly change those files while the camera can write photos. For new live library management, this design also assigns final card commits to the RTOS coordinator.

The proposed library manifest records membership, content generations, and deletion tombstones under `GRKIT/`. Delete changes the manifest instead of unlinking the GRF during a session. The manifest and retained content need bounded storage and recovery. Validate their concrete format and cleanup protocol before implementation.

Recipe records currently use 15-bit effect codes. INSTALL resolves record-code collisions by incrementing codes in catalog order. Recipes instead find effects by the original ID hash. Adding or removing an effect can therefore alter persisted references.

Admission must reject a new ID whose effect code collides with another library ID or a reserved native code. Return 409 `effect_code_conflict`. The phone can generate another ID before compiling an imported LUT.

Existing ambiguous libraries remain readable, but affected mutations are blocked. Do not silently change IDs or reassign codes. Inventory reports collisions before the user chooses how to resolve them.

Recipes already have generated stable numeric IDs. Reuse those IDs. An ID-less recipe is readable in inventory but cannot be mutated by index. Runtime ID assignment and its failure case still need a defined recovery path.

Implementation references:

- [`crates/grkit-serve/src/lib.rs`](../crates/grkit-serve/src/lib.rs)
- [`crates/grkit-serve/src/main.rs`](../crates/grkit-serve/src/main.rs)
- [`runtime/installer.c`](../runtime/installer.c)
- [`runtime/effects.h`](../runtime/effects.h)
- [`runtime/install_library.h`](../runtime/install_library.h)
- [`crates/grkit-core/src/card.rs`](../crates/grkit-core/src/card.rs)
- [`crates/grkit-core/src/recipes/model.rs`](../crates/grkit-core/src/recipes/model.rs)
- [`crates/grkit-wasm/src/lib.rs`](../crates/grkit-wasm/src/lib.rs)

GR native API research informed naming: `/props`, `/params/camera`, `/changes`, and action suffixes such as `/cancel`. These conventions do not require copying native form encoding or errors inside HTTP 200 responses.

## 3. Protocol conventions

The base URL remains `http://<camera-address>:8080/v1`. This is a separate service from Ricoh's port 80 API. The event URL is `ws://<camera-address>:8080/v1/changes`.

JSON uses camelCase. Binary responses declare their media type. JSON requests require `application/json`, except binary content routes. Unsupported media types return 415.

Use actual HTTP status codes. Errors use `application/problem+json`. State reads use `Cache-Control: no-cache` with ETags where specified. Request records and diagnostics use `no-store`.

### Identity and revisions

| Field | Meaning |
| --- | --- |
| `deviceId` | Stable identity for the camera, from a verified device identifier |
| `bootId` | New opaque value for each service lifetime, including a service restart |
| `cardId` | Opaque identity for the currently managed card |
| `revision` | Content-derived revision of a resource or coherent collection snapshot |
| `requestId` | Client-generated UUID for one mutation intent |
| `seq` | Global reliable-event sequence within one `bootId` |

Do not use names, menu positions, filenames, or runtime addresses as public identities. Look IDs map to the stable IDs in validated GRF data. Recipe IDs use the existing generated numeric identity, serialized as a string.

The implementation must establish reliable card identity before it accepts mutations. A changed card or unreadable identity prevents replay. A copied identity marker must not silently identify two cards as one.

Resource ETags identify the complete representation at the requested URI. Derive strong ETags from canonical content, not a counter that resets on restart. Configuration and content GET responses return an ETag. Volatile snapshot envelopes use the explicit watermark contract below. PATCH, replacement PUT, and DELETE require `If-Match`. Creation PUT requires `If-None-Match: *`.

A missing required precondition returns 428. A failed precondition returns 412. Changes to a referenced resource are checked separately during validation and again before application.

Collection order writes use the ETag from `GET /params/looks/order` or `GET /recipes/order`. Do not send an ETag from `/props` as the precondition for a different URI.

Global `revision` in `/props` identifies managed configuration content. Runtime menu changes also change affected revisions. Transient progress and battery changes do not invalidate configuration-only ETags.

A response with runtime fields still needs an ETag that covers those fields, or no ETag. It must not reuse a configuration-only strong ETag. The properties envelope and snapshot watermark use `no-store`.

Use a separate configuration representation for Look PATCH. Order and usage information do not belong in that representation. Runtime status and dependencies belong in `/looks/{id}/info` and the library snapshot.

### Mutation admission and deduplication

Each mutation carries `X-GRKit-Request-Id`, `X-GRKit-Boot-Id`, and `X-GRKit-Card-Id`. A software or rollback request also carries its expected software resource precondition.

The server binds a request ID to its domain, device, card, method, URI, preconditions, and content digest. A duplicate intent returns the recorded status. Reuse with different content returns 409 `request_id_conflict`.

Check for an existing request before evaluating stale resource preconditions. Within the same service lifetime, an exact duplicate retrieves its recorded result. A different boot rejects a new or unrecognized old-context mutation.

Look bodies are only 8192 or 8704 bytes in the current format. Receive each bounded body in RAM and validate it before admission to the commit path. Do not create a separate persisted upload stage for GRF.

Use a five-second idle read timeout, matching the current server, and a bounded total transfer deadline. The exact software-upload deadline must be measured on physical Wi-Fi. Reject unsupported lengths before allocation.

An interrupted body never changes live resources. Discard it and let the phone resend the complete body under the same ID and digest. Incomplete receives do not consume durable request records.

Software ZIP uploads still need bounded staging and integrity validation. Their staged data and physical-confirmation journal survive disconnects. A concurrent duplicate upload must not start another writer.

Ordinary request summaries live in a bounded in-memory cache scoped to `bootId`. Active storage transactions persist their recovery intent and digest with the commit journal. Do not write a separate history file for each parameter change.

After service restart, recover active transactions before accepting writes. If no result record survives, return unknown and require resource reconciliation. Do not claim cross-restart exactly-once execution from content equality alone.

Cancellation is a mutation. It has its own request ID and names the target request separately in the path or JSON body. Apply the same header and access checks to every cancellation endpoint.

## 4. Read model

### Global endpoints

| Method | Path | Response |
| --- | --- | --- |
| GET | `/ping` | Service reachability and `bootId` |
| GET | `/props` | Device, software summary, capabilities, limits, storage, runtime state, and active request summaries |
| GET | `/debug/revisions` | Component versions, JSON |
| GET | `/debug/diagnostics` | Bounded diagnostic report, without Wi-Fi credentials |

`/ping` can succeed while the card or runtime is unavailable. `/props` identifies that condition. It never represents an unreadable library as an empty library.

Example `/props` response, with illustrative values:

```json
{
  "deviceId": "camera-opaque-id",
  "bootId": "boot-opaque-id",
  "cardId": "card-opaque-id",
  "revision": "sha256:configuration-digest",
  "camera": {"model": "gr3", "firmwareVersion": "2.21"},
  "grkit": {"version": "1.7.0"},
  "storage": {"state": "ready", "writable": true, "freeBytes": 1073741824},
  "runtime": {"state": "ready", "busyReason": null},
  "writeAccess": {"allowed": true, "reason": null},
  "capabilities": {
    "looks": {"read": true, "write": true, "hotReload": true},
    "recipes": {"read": false, "write": false},
    "moduleSettings": {"read": false, "write": false}
  },
  "limits": {
    "loadedLooks": 40,
    "listedLooks": 64,
    "lookNameAsciiCharacters": 16
  },
  "activeRequests": []
}
```

Advertise `hotReload: true` only after the target runtime supports the full application contract. Capability checks include camera family, firmware, loaded modules, and format support.

Free-space values are advisory. The service checks space again before staging. Limits also report maximum body sizes, supported GRF features, and recipe format versions.

### Film Looks

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/looks` | Complete coherent list, collection revision, and capacity counts |
| GET | `/looks/{id}` | Writable configuration with a configuration ETag |
| GET | `/looks/{id}/info` | Runtime state, source information, and dependency references |
| GET | `/looks/{id}/content` | Exact GRF bytes with content ETag and digest |
| GET | `/params/looks/order` | Ordered stable IDs with an order ETag |
| PUT | `/looks/{id}/content` | Create or replace a Look from a raw GRF body |
| PATCH | `/looks/{id}` | Change name or enabled state |
| DELETE | `/looks/{id}` | Remove a Look |
| PUT | `/params/looks/order` | Replace the order with a complete list of IDs |

The first version returns the whole bounded library. Pagination is unnecessary at current limits.

Every valid GRF ID remains addressable, including `order` and `requests`. Collection ordering lives under `/params/looks/order`. Per-Look request queries live below the Look ID. Recipe IDs are numeric, so recipe collection routes cannot collide with them. Give wildcard routing no priority over a longer, explicit route.

Example configuration from `GET /looks/{id}`:

```json
{
  "id": "lut-example",
  "name": "Example",
  "contentHash": "sha256:...",
  "enabled": true
}
```

`GET /looks/{id}/info` returns loaded state, selection, origin, and dependency references. `/params/looks/order` owns position. The list combines these views into a coherent read-only snapshot.

Enabled permits use. Selected identifies the active Look. Current Look choices have an Off bit and position, but no independent hidden bit. Do not add `hidden` to the v1 Look contract. Recipes retain their existing separate hidden state.

An official-looking filename does not prove official origin. Verify content against an official catalog before labeling it verified. Unknown metadata remains unknown.

Content PUT requires `Content-Length` and `X-GRKit-Content-SHA256`. The GRF ID must match the path ID. The server validates the complete effect, features, size, and camera compatibility before application.

A new Look is enabled and appended to the order. It does not replace the active Look. A custom Look replacement preserves identity, position, and enabled state.

PATCH accepts only documented fields. Omitted fields stay unchanged. Unknown fields and invalid values return 422. Renaming a custom Look updates the compiled GRF and preserves its ID. A rename also changes the content ETag.

Editing a custom cover uses replacement GRF content with the same ID. There is no independent image-upload API in the first delivery.

Official Look names, covers, and content cannot be edited through user mutation endpoints. Return 409 `official_look_read_only` for those edits. Enabling, disabling, ordering, and removal remain available with the normal dependency checks.

To edit an official Look, the phone downloads its GRF and creates a custom copy with a new ID. The copy must pass effect-code collision checks. Existing recipes continue to reference the original Look until explicitly edited.

Official identity must come from trusted catalog or package metadata, not a client-supplied source label. Retain official identity for removed entries so re-adding an ID cannot bypass the editing rule. An unknown or conflicting origin requires resolution before content replacement.

Re-adding a removed official Look accepts only verified official content for that ID. Refreshing official content is a distinct catalog/package reconciliation action, not permission for arbitrary same-ID edits.

A same-name Look with a different ID is a separate resource. A repeated ID with different content is a replacement and requires the existing content ETag.

DELETE rejects dependencies with 409 `dependency_in_use`. Dependencies include recipes and dial sets. If dependency scanning is unavailable or ambiguous, deletion fails closed.

Deleting the selected Look requires `?active=off`. Without this explicit policy, return 409 `active_look`. The runtime switches to Off at the safe commit point.

Replacing a referenced custom Look keeps the reference but changes its appearance. The phone uses `/info` to explain this effect before submission. The server rechecks reference validity before commit.

If informed acknowledgement must bind the exact dependency set, add a dependency-revision precondition before shipping that feature. An unversioned boolean query parameter is not such a precondition.

A library containing duplicate files with the same GRF ID reports `duplicate_id`. The current catalog selects the smallest filename. Block mutations for that ID until the duplicate is resolved. Writing the canonical filename alone cannot guarantee which content loads.

Order PUT accepts `{"ids":["id-a","id-b"]}`. Each library ID occurs exactly once. A missing, duplicate, or unknown ID rejects the whole request.

Capacity validation considers both listed and loaded limits. Do not silently accept a new enabled Look that cannot load. Return required and available capacity in the error.

Multi-file batch import is deferred. A phone can issue separate requests and report each result. Do not claim atomicity across those requests.

### Recipes

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/recipes` | List recipes, compatibility, and order |
| GET | `/recipes/{id}` | Read a complete portable recipe and dependencies |
| PUT | `/recipes/{id}` | Create or replace a recipe from JSON |
| PATCH | `/recipes/{id}` | Edit documented fields, enabled state, or visibility |
| DELETE | `/recipes/{id}` | Remove a recipe, with reference checks |
| GET, PUT | `/recipes/order` | Read or replace recipe order |
| POST | `/recipes/{id}/apply` | Apply a recipe to current camera parameters |

Portable JSON declares a schema identifier, camera family, stable recipe ID, Look ID, adjustments, and named module settings. It does not expose RAM offsets or slot indices.

Native camera parameters use semantic names rather than firmware numeric indices. Unsupported fields produce compatibility errors. The service does not silently omit recipe settings.

Read camera-created recipes as well as imported recipes. Resolve compact effect codes to stable IDs. Ambiguous references return an explicit unresolved dependency and prevent apply or destructive edits.

For sharing, the phone reads the recipe and each referenced GRF with their ETags. It rechecks the dependency revisions before publishing the assembled bundle locally. A conflict restarts the export snapshot.

Original LUT files are not recoverable from GRF content. Sharing a converted Look must not label it as the original LUT.

### Modules and current parameters

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/modules` | Installed modules and runtime availability |
| GET | `/modules/{id}` | Module metadata and setting definitions |
| GET, PATCH | `/params/modules/{id}` | Read or change module values |
| GET, PATCH | `/params/camera` | Read or change the active Look and its adjustments |

Definitions include key, type, allowed values, range, step, unit, dependencies, read-only reason, and application mode. Values use stable symbolic setting names.

PATCH groups related values into one request. The service validates the final combination. Unsupported or read-only values reject the entire request.

A parameter request succeeds only after runtime acknowledgement and persistence. The implementation can coalesce slider input in the phone. It must not claim persistence before the durable state exists.

Recipe apply accepts `expectedRecipeRevision` and `expectedParameterRevision` in its JSON body. These bind the source recipe and target parameters. Do not send the recipe resource ETag as `If-Match` on the separate `/apply` URI. The commit rechecks both resources and dependencies.

Installing executable modules remains part of signed software installation. These endpoints cannot upload code, write arbitrary paths, or execute commands.

## 5. Software update and rollback

| Method | Path | Purpose |
| --- | --- | --- |
| GET | `/update` | Software inventory, available backup, and current or latest update request |
| PUT | `/update` | Receive a raw signed ZIP and prepare an update |
| POST | `/update/cancel` | Cancel the identified update when cancellation is still safe |
| GET | `/rollback` | Backup availability and current or latest rollback request |
| POST | `/rollback` | Prepare rollback to the identified backup |
| POST | `/rollback/cancel` | Cancel the identified rollback when cancellation is still safe |

GET responses expose resource ETags. Update PUT uses the ETag from GET `/update`. Rollback POST uses the ETag from GET `/rollback` and identifies the target backup.

The update ZIP requires length and SHA-256 headers. Official signature and target validation remain mandatory. A checksum is not an authorization substitute.

Software inventory includes package membership and content hashes. The phone must preserve custom library content when preparing an update. The release catalog alone cannot reconstruct imported Looks.

A pending software update or rollback blocks data mutations. Data changes also invalidate software inventory preconditions. Reads and cancellation remain available.

The existing physical camera confirmation remains required. Confirmation and reboot are separate phases. Journal disappearance alone never proves success.

Recovery checks the installed manifest and expected content identity, not only a version string. Two builds can share a version. Rollback checks the expected backup identity.

Data management does not edit signed package code. Software rollback preserves user data. Reject a target that cannot read the current managed-library manifest or other required data formats before staging.

GET `/rollback` distinguishes an existing backup from an allowed rollback. Report `allowed: false` and `reason: "data_format_incompatible"` when the target lacks proven format support. POST `/rollback` repeats this check and returns 409 before writing an installation journal.

Unknown format compatibility also blocks rollback. Do not infer compatibility from the version string alone. Use trusted target format metadata. The phone explains the incompatibility before offering confirmation.

Do not remove the manifest, resurrect deleted Looks, or convert user data to an older format to enable rollback. Such conversion is outside this design.

## 6. Request lifecycle and recovery

Domain-specific query paths retain result recovery without a public task-creation endpoint:

| Domain | Query | Cancellation |
| --- | --- | --- |
| One Look | `GET /looks/{id}/requests/{requestId}` | `POST /looks/{id}/requests/{requestId}/cancel` |
| Look ordering | `GET /params/looks/requests/{requestId}` | `POST /params/looks/requests/{requestId}/cancel` |
| Recipes | `GET /recipes/requests/{requestId}` | `POST /recipes/requests/{requestId}/cancel` |
| Parameters | `GET /params/requests/{requestId}` | `POST /params/requests/{requestId}/cancel` |
| Update | `GET /update?requestId={requestId}` | `POST /update/cancel` with `targetRequestId` |
| Rollback | `GET /rollback?requestId={requestId}` | `POST /rollback/cancel` with `targetRequestId` |

Lookup of a retained per-Look request works after that Look was deleted. It does not require the resource to exist. Parameter changes use synchronous responses when possible, but can await capture safety or persistence. Their result query remains necessary in those cases.

A short request can finish synchronously. Return 201 for resource creation or 200 for a completed edit. Responses include `requestId` and resulting revisions.

An unfinished accepted request returns 202 and `Location` for its domain query path. A 202 response never means that the requested content is active.

```json
{
  "requestId": "6b23bb06-b505-4fa6-bdd4-754018494129",
  "domain": "looks",
  "status": "running",
  "phase": "waitingForIdle",
  "progress": null,
  "cancellable": true,
  "requiredAction": null,
  "result": null,
  "error": null
}
```

| Status | Meaning |
| --- | --- |
| `pending` | Accepted, not executing |
| `running` | Receiving, validating, waiting, applying, or recovering |
| `succeeded` | Durable result and required runtime verification are complete |
| `failed` | Terminal failure with a known resulting state |
| `cancelled` | Cancellation completed before the irreversible boundary |

`phase` supplies detail without adding more lifecycle states. Examples include `receiving`, `validating`, `waitingForIdle`, `applying`, `awaitingConfirmation`, `awaitingRestart`, and `recovering`.

Progress is null when the service cannot measure it. Otherwise return a named stage and completed/total units. Phone upload progress is separate from camera validation and application progress.

Cancellation returns 202 while cancellation proceeds. It returns 409 `cannot_cancel` after the commit boundary. A duplicate cancellation returns the current result.

Once committing starts, finish or recover the transaction. Do not promise cancellation of an in-progress commit. A disconnected client does not cancel work.

Retain active records and a bounded set of terminal summaries in RAM during one service lifetime. Expose limits in `/props`. Never evict an active record to admit another request.

Persist storage recovery intent and software update state, not a second general-purpose request history. Completed ordinary summaries can disappear on restart or cache eviction. Result lookup returns 404 `request_result_unknown` in that case.

The phone never automatically retries an unknown result. Read the actual resource and reconcile the prior intent. An identical desired resource does not prove which request caused it. Require a new user decision when competing edits make the outcome ambiguous.

Deduplication is guaranteed only while the request record exists. Bound admitted IDs per boot and reject new writes when the cache cannot retain the required retry window. Exact capacities and timing remain a measured implementation choice.

## 7. WebSocket changes

`GET /changes` upgrades to WebSocket. The service sends events. Clients send no application commands over this socket. Standard WebSocket Ping/Pong handles connection liveness.

After upgrade, the service registers the subscriber and sends a `hello` message with `bootId`, `cardId`, and the current sequence. Registration precedes the snapshot synchronization window.

```json
{
  "type": "resourceChanged",
  "seq": 104,
  "bootId": "boot-opaque-id",
  "cardId": "card-opaque-id",
  "changed": "looks",
  "revision": "sha256:looks-snapshot-digest",
  "requestId": "6b23bb06-b505-4fa6-bdd4-754018494129",
  "ids": ["lut-example"]
}
```

Resource events identify invalidated resources. They do not carry full binary content. Changes from camera menus have no phone request ID.

Request events use `type: "requestChanged"`. They include the request summary, query URI, domain, and request ID. A failure emits a result event even when no resource changes.

Domains are `looks`, `recipes`, `params`, `modules`, `update`, `rollback`, and `storage`. Card replacement invalidates every card-scoped cache and request context.

### Subscription synchronization

Reliable resource events and terminal request events share one global `seq` within a `bootId`. Assign the sequence and commit the visible snapshot under the same coordinator boundary. Every subscriber receives the same reliable sequence.

Progress events are best effort and carry no `seq`. They can be coalesced or omitted. Never treat a progress event as the only completion record.

Each HTTP snapshot includes `bootId`, `cardId`, and `snapshotSeq` in its envelope. Configuration-only and binary responses carry these values in response headers instead. Their ETag covers representation bytes, not the watermark headers. The snapshot contents and watermark must describe the same coordinator point. A cached 304 response must not invent a later watermark for unchanged cached content. Read synchronization envelopes without caching.

1. Open the socket and wait for `hello`.
2. Buffer reliable events while reading `/props` and visible resource snapshots.
3. For each resource, discard buffered events with `seq <= snapshotSeq` from that resource's snapshot.
4. Apply later invalidations and re-fetch affected resources as needed. Advance the per-resource watermark with each fetched snapshot.

A watermark is not a resource revision. It proves which reliable events the snapshot already includes. The client does not compare opaque hashes to order old and new events.

On reconnect, changed `bootId`, or a reliable sequence gap, read fresh snapshots and active request results. The first delivery does not replay historical events. A live stream can keep changing while synchronization continues. The client must not block the UI waiting for global silence.

Bound each subscriber queue and the number of subscribers. Do not drop reliable events silently. Close an overloaded socket and let the client synchronize again.

A resource snapshot remains authoritative. WebSocket delivery is neither durable nor exactly once. Querying HTTP results remains possible without a socket.

## 8. Hot loading and persistence

The HTTP handler validates framing. It receives a GRF into bounded RAM and a software ZIP into bounded staging storage. The runtime coordinator owns mutation admission, safe application, and acknowledgement.

Use one configuration commit at a time. Receiving a bounded body and validating it do not hold the runtime commit lock. Limit staged requests and return 409 `camera_busy` when capacity is occupied.

The service continues to answer reads, result queries, and cancellation during uploads and runtime waits. Replace the current blocking connection loop with bounded concurrent I/O.

The coordinator handles Looks, recipes, parameters, and software exclusion through the same internal mechanism. This reuse is not a public generic API.

### Required application sequence

1. Validate the request, identity, preconditions, content, capacity, and dependencies.
2. Prepare a complete candidate configuration without modifying active records.
3. Ask the RTOS storage owner to reserve recovery space and persist the transaction intent and required old/new data.
4. Ask the runtime for a safe application window.
5. Recheck preconditions and dependencies under the application guard.
6. Commit durable state and switch runtime references through a recoverable protocol.
7. Verify the acknowledged runtime generation and durable content.
8. Publish resource revisions and the event watermark, cache the terminal result, and notify subscribers.

The RTOS coordinator owns final writes to the managed data library and state. Linux validates data in RAM and requests commits through a bounded control channel. It does not unlink or overwrite live GRF files.

A proposed manifest selects immutable content generations. Logical deletion and replacement update that manifest through the RTOS storage protocol. Old content remains available for rollback until recovery no longer needs it.

Manifest publication, checksums, bounded garbage collection, and cross-OS read coherence require a filesystem prototype. Do not assume that retaining files indefinitely is acceptable. Software staging follows its existing separate journal path and remains exclusive with data mutations.

The storage and runtime steps are not one hardware transaction. The recovery record must distinguish prepared, durable-committed, runtime-applied, and completed phases.

Before durable commit, abort restores the old configuration. After durable commit, recovery converges to the committed configuration. Runtime failure must remain visible until convergence or a verified compensating transaction.

If recovery cannot establish a coherent result, block further mutations and expose `recoveryRequired`. Keep a usable old runtime when possible. Do not report success or a clean rollback without verification.

A boot recovery pass completes before loading the managed library. It must not load partially published content generations. The exact filesystem protocol remains a prototype gate.

### Runtime allocation and safety

Current records, icon chunks, and settings tables are sized at startup. There are no guaranteed spare slots. Current effect setting handles also depend on list positions. Inserting a sorted ID cannot shift handles that running code still uses.

Prototype fixed-capacity record and icon pools with stable slots. Provision settings slots for all listed effects, not only the 40 loaded effects. Keep display order separate from slot identity. Check the global settings-table budget as well as record memory.

Do not free slots until no capture or menu reader can retain a reference. Preallocation avoids allocator churn, but does not make reuse or in-place replacement safe. The prototype must establish quiescence or another proven publication mechanism.

Measure the full budget, including icons, records, curves, labels, settings, and any candidate copy. Do not adopt a partial memory estimate as a target guarantee.


- Do not replace records while capture hooks or image processing use them.
- Do not free menu labels or icons while an open menu can reference them.
- Preserve the active Look by ID, not by its old index.
- Preserve existing parameter values, recipe references, and dial-set references by identity.
- Do not reset unrelated modules or the capture pipeline.
- Do not restart the service, AP, or BLE connection to apply data.

The runtime can wait for capture to finish or ask the user to leave an affected menu. Report the reason and a bounded wait timeout. A timeout before commit leaves old content unchanged.

A replacement of the selected custom Look applies its new rendering only at the safe boundary. Preserve explicit user adjustments when valid. Invalid adjustments cause validation failure rather than silent clipping.

A successful addition is visible in the camera menu and usable for capture during the same connection. An unsupported hot-loading target must report that capability as unavailable.

## 9. Persistence, package imports, and references

The proposed managed manifest under `GRKIT/` is the live library source of truth. Deleting an official Look must remain effective after another startup or a package update.

The implementation must reconcile the package inbox, `IMPORTED.TXT`, package exclusions, and managed library choices. Package exclusion lines currently hide library filenames. Changed inbox hashes can overwrite library content at startup. The new reconciliation must replace this behavior where managed choices exist.

Software packages continue to include official Looks for installation with a card reader. On an unmanaged first installation, the package selections seed the library. On a managed card, the manifest takes precedence over package selections.

Explicit removal tombstones prevent an official Look from returning after startup, update, or rollback. Custom copies and their edits remain unchanged. Preserve explicit enabled states and ordering when reconciling official entries.

Managed additions must not remain hidden by an old package exclusion. Package content cannot overwrite a custom entry merely because filenames match. Conflicting identities stop reconciliation instead of silently replacing content.

The exact policy for new official catalog entries and revised official content remains to be specified. It must respect removal tombstones and user selections. This policy does not permit user edits of an official identity.

Do not edit `PACKAGE.TXT` to represent library changes. Software rollback verifies installed package hashes and would become unavailable. Keep library decisions in `GRKIT/`. Removing one GRF file alone is not a deletion contract.

The same reconciliation must retain custom effects across software update and rollback. Never regenerate the library solely from the online catalog or `PACKAGE.TXT`.

Manual card edits invalidate cached inventory. Scan and validate actual content on startup and before affected mutations. Unsupported or corrupt files appear with a reason where possible.

Recipe and dial-set references require dependency validation before delete or replacement. A reverse index can accelerate lookup, but it must be rebuilt or checked against authoritative records.

When compact runtime codes or reserved native codes collide, reject the conflicting mapping during admission. Do not guess which Look a recipe means. Public IDs remain stable across ordering changes and restarts.

## 10. Errors and access policy

Errors follow RFC 9457. `code` is the stable application discriminator. `detail` is diagnostic text and is not parsed by the phone.

```json
{
  "type": "urn:grkit:problem:dependency-in-use",
  "title": "Look has dependent recipes",
  "status": 409,
  "code": "dependency_in_use",
  "requestId": "6b23bb06-b505-4fa6-bdd4-754018494129",
  "recipeIds": ["recipe-a"],
  "retryable": false
}
```

| HTTP status | Examples |
| --- | --- |
| 400 | Invalid JSON or request framing |
| 403 | Phone changes disabled |
| 404 | Unknown resource or request |
| 409 | Busy writer, dependencies, active Look, request-ID conflict |
| 412 | Stale resource or context precondition |
| 413 | Body exceeds declared limits |
| 415 | Unsupported media type |
| 422 | Invalid effect, parameter value, signature, or incompatible content |
| 428 | Missing required resource precondition |
| 503 | Card or runtime temporarily unavailable |
| 507 | Insufficient storage for staging and recovery |

After HTTP acceptance, failures appear in the request record using the same problem structure. A successful GET of a failed request still returns HTTP 200.

The local camera network is the initial transport trust boundary. The API does not claim that a BLE bond authenticates an HTTP request. There is no account or Internet login.

All writes require a runtime-enforced phone-changes permission. The intended product default is enabled, subject to a final access-policy decision before release. Recheck permission before commit.

Every mutation, including POST actions and cancellation, requires the custom request and context headers. Reject missing headers before processing a body. Reject browser mutations with an `Origin` header. Do not enable wildcard CORS.

WebSocket handshakes do not use CORS preflight. Reject handshakes carrying `Origin`. Native clients must omit it. Validate this behavior with the iOS and Android networking libraries before enabling subscriptions.

Validate `Host` on HTTP and WebSocket handshakes against the local service address and port, using parsed IP literals. Permit the explicit loopback address only in development. Do not accept arbitrary DNS hostnames or trust proxy forwarding headers. This limits DNS rebinding.

These checks reduce browser access. They do not authenticate a hostile native client on the same network. The final local-network trust policy remains a product decision.

Software code requires official signatures. Data requires strict decoding, path containment, content bounds, and reference validation. Uploaded filenames never become arbitrary card paths.

Body lengths, staging bytes, request records, sockets, and WebSocket queues have explicit bounds. Disk cleanup never removes active recovery data. Manifest content reclamation occurs only through the verified storage owner.

## 11. Delivery and validation

### Delivery order

1. Define schemas, identity, revisions, errors, request records, and bounded transport behavior.
2. Prototype runtime and filesystem recovery for one Look on GR III and GR IV.
3. Implement Look inventory, content transfer, CRUD, and WebSocket notifications.
4. Add the phone library and LUT import flow on iOS and Android.
5. Replace current software update clients with the redesigned v1 contracts.
6. Implement recipes and module settings after the shared control path is verified.

Do not modify product code until the design and prototype scope are approved. This document is not approval to implement all listed features.

### Acceptance matrix

| Area | Required checks |
| --- | --- |
| Basic Looks | Import, replace or rename custom Looks, enable, reorder, and delete during one Wi-Fi connection |
| Official Looks | Reject name/cover/content edits, permit custom copies with new IDs, retain original recipe references |
| Rendering | New Look appears in menus and affects a captured photo without reboot |
| Active Look | Unrelated changes preserve selection and adjustments, explicit selected deletion switches to Off |
| Capacity | Separate listed/loaded limits, exact boundary, no silent omission |
| Dependencies | Recipes and dial sets block deletion, hash/native-code collisions and duplicate files are rejected |
| Persistence | Reboot retains edits, removed official Looks do not return, custom Looks survive update and rollback |
| Concurrency | Camera menu edits race with HTTP writes, stale requests do not overwrite them, strong ETags cover their representation |
| Capture safety | Single capture, continuous processing, open menus, and pending parameter saves |
| Networking | Interrupted body, lost response, duplicate request, disconnect during commit, service restart |
| WebSocket | Snapshot watermarks, reconnect, global sequence gaps, progress without sequence, queue overflow, no socket available |
| Cancellation | Before receiving finishes, before commit, after commit boundary, repeated cancellation |
| Storage | Card unavailable, full card, card swap, write error, power loss at each recovery phase |
| Updates | Signature rejection, physical confirmation, timeout, reboot verification, rollback identity, incompatible-format rejection before staging |
| Access | Disabled writes, browser POST, WebSocket Origin, DNS rebinding, malformed bodies, arbitrary paths, resource bounds |

Use unit tests for schemas, revisions, deduplication, and state transitions. Use firmware emulation for runtime and filesystem failure injection. Test AP continuity and capture behavior on physical cameras.

Emulator transport does not prove physical Wi-Fi stability or power behavior. Existing update tests do not prove hot loading.

### Implementation gates

The following choices require evidence before implementation is considered ready:

- Full fixed-capacity memory and settings-table budgets, stable slot mapping, and safe record/icon replacement on GR III and GR IV.
- A runtime guard that covers capture hooks, menu readers, and state persistence.
- RTOS-owned manifest publication, content reclamation, and a recovery protocol supported by the camera filesystem.
- Stable card identity and behavior for cloned or reformatted cards.
- In-memory request retention and total upload deadlines measured on physical Wi-Fi.
- Runtime assignment for ID-less recipes and detection of existing ambiguous effect-code mappings.
- Package-inbox reconciliation and data retention during software rollback.
- Phone-changes enforcement and the final local-network access policy.


### Approved product decisions

The owner approved these decisions on 2026-11-16:

1. Official Look names, covers, and content remain unchanged. Editing creates a custom copy with a new ID. Removal, enablement, and ordering remain available.
2. Software packages continue to carry official Looks. The managed manifest protects deletions, custom edits, and explicit selections during reconciliation.
3. Rollback to a runtime that cannot read the managed manifest is blocked. Preserve user data and explain the incompatibility before staging.

### Product details still open

- Define how new or revised official catalog content enters an existing managed library without changing explicit user selections.
- Define the default phone-changes permission and explain pending-update exclusion in the App.

### Review disposition

Claude Code Fable 5.2 reviewed the initial draft. Source checks confirmed filesystem visibility hazards, code collisions, missing capacity, unsupported Look visibility, and route-name collisions.

This revision adds RTOS storage ownership, a proposed manifest, global event watermarks, configuration-specific representations, and browser request checks. It removes persisted generic request history and GRF upload staging.

Two simplifications are deliberately limited. Runtime fields cannot share a configuration-only strong ETag. Parameter edits can still need asynchronous completion while capture or persistence is pending.

The reviewer did not review this revised draft. No runtime prototypes or new implementation tests were executed for the document.

## 12. Standards references

- [RFC 9110: HTTP Semantics](https://www.rfc-editor.org/rfc/rfc9110.html): methods, conditional requests, and status codes.
- [RFC 9457: Problem Details](https://www.rfc-editor.org/rfc/rfc9457.html): structured HTTP errors.
- [RFC 6455: WebSocket Protocol](https://www.rfc-editor.org/rfc/rfc6455.html): framing and control messages.

The resource and action names are project conventions. HTTP does not require a particular route spelling. Hot-loading and durability guarantees are GRKit requirements, not guarantees provided by HTTP.
