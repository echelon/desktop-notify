# desktop-notify

A local HTTP server for desktop notifications. It currently plays one-shot and
looping sounds; visible desktop notifications are planned for the future.

This Rust workspace starts with
[`agent-notify-server`](crates/agent_notify_server/README.md), copied from ArtCraft.
Additional crates can live under `crates/` and share dependencies declared in the
root `Cargo.toml`.

## Run

```sh
cargo run --locked -p agent-notify-server
```

The server listens on `http://127.0.0.1:43110`. Open that address for the API
reference, or send requests directly:

```sh
curl http://127.0.0.1:43110/alert_beep  # Play a sound once
curl http://127.0.0.1:43110/loop_done   # Repeat the completion sound
curl http://127.0.0.1:43110/stop        # Stop all playback
curl http://127.0.0.1:43110/state       # Inspect playback and configuration
```

Press Ctrl+C to stop the server.

## Configuration

Bundled sounds and the default YAML configuration live in
[`crates/agent_notify_server/config/`](crates/agent_notify_server/config/notify_config.yaml).
Sound paths are resolved relative to the YAML file. The default configuration path
is embedded at build time, so running from another directory works as long as the
source configuration and sounds remain in place. When moving the binary to another
machine, also copy the configuration and sounds and set `NOTIFY_CONFIG_PATH`.

Environment variables:

- `HTTP_BIND_ADDRESS`: listener address (default `127.0.0.1:43110`).
- `NOTIFY_CONFIG_PATH`: path to a custom YAML configuration.
- `RUST_LOG`: logging filter (default `info,actix_web=info`).

See the [server README](crates/agent_notify_server/README.md) for the full API,
loop escalation settings, and agent integration examples.

## Development

```sh
cargo fmt --all --check
cargo check --locked --workspace --all-targets
cargo test --locked --workspace
```

The lockfile retains the server's dependency versions from ArtCraft.

Playback requires an audio output device. Linux builds also need ALSA development
headers and `pkg-config` (for example, `libasound2-dev` and `pkg-config` on Debian or
Ubuntu).
