# AGENT_CORE_NET

Scope: **shared core, network protocol, serialization, and persistent state.** This agent owns the parts of the codebase that both the streamer and the headset client depend on — i.e. the wire format, the sockets layer, the session schema, and the shared logging/primitive types. A change in this scope is almost always a cross-process compatibility change.

## Crates owned by this agent

| Path | Crate | Responsibility |
| --- | --- | --- |
| `alvr/common` | `alvr_common` | Re-exports (`anyhow`, `glam`, `log`, `parking_lot`, `semver`, `settings_schema`), shared primitives (`Pose`, `Fov`, `DeviceMotion`, device-ID constants), `ConnectionState`/`LifecycleState` enums, logging frontend, `c_api` exposed to integrators. |
| `alvr/sockets` | `alvr_sockets` | TCP control socket + stream socket (UDP/TCP, with throttling), mDNS discovery (`_alvr._tcp.local.`), buffer-size tuning. Constants live at the top of `src/lib.rs` (`CONTROL_PORT = 9943`, `KEEPALIVE_INTERVAL`, `KEEPALIVE_TIMEOUT`). |
| `alvr/session` | `alvr_session` | Settings schema (derived via `settings-schema-rs`, pinned git rev), `SessionConfig`/`Settings`, migrations between versions. Note: the user-facing "schema" lives here — there is no separate `alvr/schema` crate in this fork. |
| `alvr/packets` | `alvr_packets` | Every wire type exchanged between streamer and client: `ClientControlPacket`, `ServerControlPacket`, `TrackingData`, `VideoPacketHeader`, `StreamConfigPacket`, etc. The numeric stream IDs (`TRACKING`, `VIDEO`, `AUDIO`, `HAPTICS`, `STATISTICS`) are also here. |
| `alvr/events` | `alvr_events` | The event bus type emitted by the streamer for the dashboard/web UI (`EventType`, `HapticsEvent`, `ButtonEvent`, `AdbEvent`, ...). |

## Coding rules

- Wire and schema compatibility is **load-bearing**. Adding a field to `ClientControlPacket` or `Settings` without a migration breaks every shipped client/streamer pair. Add fields at the end of `serde` structs, prefer `#[serde(default)]`, and add a migration step in `alvr_session` for schema changes.
- Every numeric/literal constant that represents a tunable (timeout, port, buffer size, retry interval) must be a named constant at the top of its file, typed with `Duration`/`Path`/etc. where applicable. See `alvr/sockets/src/lib.rs` for the pattern.
- Logging uses `alvr_common::{info, warn, error, debug}`. Never introduce a second logging crate or `println!` from these crates.
- `c_api.rs` exports are external product surface — never reorder/rename without a deliberate version bump and a `cbindgen` regeneration.
- Use `anyhow::Result` for fallible APIs (re-exported as `alvr_common::anyhow::Result`). For connection-specific failures see `alvr_common::ConResult` / `ConnectionError` and the `con_bail!` macro.

## Running and testing

```
# Type-check the network/protocol crates in isolation
cargo check -p alvr_common
cargo check -p alvr_sockets
cargo check -p alvr_session
cargo check -p alvr_packets
cargo check -p alvr_events

# Standard verification for this scope
cargo test -p alvr_sockets
cargo test -p alvr_session        # the workspace's primary test target; CI runs this on every PR

# Lint the protocol surface
cargo clippy -p alvr_sockets -p alvr_packets -p alvr_session -p alvr_common -- -D warnings
```

Run a full streamer to exercise the protocol against a real client:

```
cargo xtask prepare-deps --platform <windows|linux>     # only if ./deps is empty
cargo xtask run-streamer                                # builds + launches the dashboard/driver
```

For wire-protocol changes, the only realistic integration test is **streamer + client on the same network** — load the streamer with `run-streamer`, build the client with `cargo xtask build-client`, deploy with `adb install -r`, and verify handshake + streaming.

## Where bugs in this scope usually surface

- Streamer logs in `%APPDATA%/ALVR-Launcher/.../session_log.txt` (Windows) or `~/.config/alvr/session_log.txt` (Linux). The streamer also emits structured events the dashboard can subscribe to (`alvr_events::EventType`).
- Client logs over `adb logcat -s ALVR:*` (see `AGENT_ANDROID_CLIENT.md`).
- For mDNS/discovery: confirm both sides advertise on `_alvr._tcp.local.` and that PC and headset share an L2 broadcast domain.

## Before touching this scope

Read `ARCHITECTURE.md` — specifically the "Tracking Flow" and "Video Pipeline" sections — because a change here lands inside the loops described there.
