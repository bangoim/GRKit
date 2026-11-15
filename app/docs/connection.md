# Camera connection

The real iOS connection path is BLE discovery → encrypted Wi-Fi credentials →
camera wake / AP startup → Wi-Fi authorization and association → GRKit status and
package verification. Demo mode uses `FakeCamera`; real updates and rollback use
the [camera update protocol](updates.md).

## Ownership and cancellation

`AppState` permits one connection task at a time. Its generation fences UI writes;
an abort handle drops the operation when the user cancels. A new task cannot start
until the previous task has unwound. Native BLE cleanup and scan resumption live
in drop guards, including failures after BLE setup.

ASK joins have separate request ids. A late completion can only settle its own
request. Cancelling a system Wi-Fi request does not cancel the underlying iOS
association: NetworkExtension exposes no cancellation API. An outstanding ASK
picker blocks another picker until it closes. Cancellation never authorizes a
fallback join through a second API.

## Behavior shared with GR Snap

- Saved, powered-on cameras have prompt priority. An unknown camera must be the
  only eligible nearby candidate. Proximity enters at -60 dBm and leaves below
  -70 dBm; unknown RSSI cannot establish proximity.
- Dismissal persists across scan updates and resets on an observed power-off.
- Scanning pauses before BLE connection. Pending connections are cancelled before
  retrying, and GATT waits have fixed deadlines.
- AP-ready notifications are latched even when they arrive before write completion.
  AP startup is best effort once credentials are available; a BLE loss at that
  point does not prove Wi-Fi will fail.
- ASK consent precedes Wi-Fi association. Association races native completion
  against a valid GRKit HTTP response and has a foreground-time budget. Explicit
  refusal is terminal; transient native failure still permits HTTP verification.
- BLE loss is a hint, not proof of Wi-Fi loss. While idle, HTTP is checked every
  five seconds and on BLE disconnect; two failed probes mark the link offline.
  Update sheets defer monitoring. A powered-on advertisement can then trigger
  one reconnect attempt.

GRKit uses `/v1/status` and `/v1/package` on port 8080 instead of GR Snap's photo
API verification. Each HTTP request has a three-second deadline, redirects are
disabled, and requests ask caches not to reuse responses. Status retries have a
30-second budget. A camera API response on port 80 distinguishes missing GRKit
from an unreachable network; a mounting card is reported separately.

## Remaining differences and hardware checks

This is not a port of every GR Snap connection feature. GRKit still stores one
camera identity, without a saved passphrase. It does not yet expose saved-camera
paging, manual SSID entry, or existing-network detection. It also lacks GR Snap's
forced reassociation / subnet-conflict diagnostics and whole-setup pairing retry
budget. Discovery does not expire silent advertisements or synthesize sightings
from already-connected peripherals.

Reference modules in the sibling GR Snap checkout: `features/connection/flows.ts`,
`nearby.ts`, `ap-warmup.ts`, `services/camera/ble.ts`, `wifi.ts`, and
`connection-lifecycle.ts`. Keep GRKit-specific service checks separate from photo,
playback, and geotagging session behavior.

Before release, verify on GR III / IIIx and GR IV hardware: first pairing, declined
ASK consent, saved authorization, AP startup with BLE loss, rapid cancel/retry,
background/foreground during a join, camera power-off/on, and missing or mounting
SD card. Simulator and headless tests do not validate radio or system-dialog timing.
