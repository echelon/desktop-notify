# Shared notification types

Inherit the root rules, including **two-space Rust indentation**. This small
Serde crate is the wire contract shared by the server and desktop app; keep it
independent of HTTP, audio, Tauri, filesystem discovery, and process control.

## Compatibility and validation

- `Notification` is shared by both Rust consumers. Preserve the serialized names
  and defaults so older title/message-only notifications still deserialize.
- `Origin` and `SessionContext` are optional metadata. Every field inside them is
  optional; preserve `serde(default)`, omitted empty values, and their existing
  unknown-field rejection. Keep notification kinds compatible with the server,
  frontend, and Python hook.
- Keep validation here when both consumers need it. Distinguish character limits
  from byte limits: context limits count characters, origin string limits count
  bytes. Keep Python's truncation limits aligned with the wire contract.
- Origin strings must be bounded, nonblank, and free of control characters.
  Validate bundle identifiers, process IDs, absolute tmux sockets, `%<digits>`
  pane IDs, and `/dev/` TTY paths before any platform operation.
- Keep terminal alias normalization in `Origin::bundle_id`; avoid scattering
  inconsistent aliases through the Rust callers.

`Agent` and `TaskState` are wire enums. `Agent` keeps a `#[serde(other)]`
`Unknown` variant so a newer producer's value cannot break an older consumer.

## Context merge contract

`SessionContext::apply` treats omitted/null fields as unchanged, supplied strings
as updates, and whitespace-only strings as explicit clears. A changed `cwd`
clears the old repository name/description unless the patch supplies replacements.
Validation permits newlines and tabs in context text, but no other control
characters. The server decides whether a session is eligible to inherit context;
the types crate must not add global state or cross-session caching.

When adding a field, review serialization/defaults, validation, merge/clear
behavior, Python collection, UI rendering, and API documentation together.

## Verification

Run `cargo test --locked -p notify-types` for type/validation changes and
`cargo test --locked --workspace` when the shared contract changes. Keep legacy
JSON, partial metadata, invalid hints, and context merge behavior covered in the
type tests and server notification tests.
