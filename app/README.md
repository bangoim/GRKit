# GRKit app

Phone app that updates GRKit on a RICOH GR camera over the camera's Wi-Fi.
Version 1 ships the screens and a demo mode with a fake camera; a later task
adds the real link.

## Requirements

- Rust 1.99.1 (see `rust-toolchain.toml`)
- NSH CLI 0.5.1 (`nsh --version`)
- Xcode (for iOS simulator packaging)

## Commands

```sh
# Unit + headless UI tests
cargo test

# Format and lint
cargo fmt --check
cargo clippy --all-targets -- -D warnings

# Desktop (macOS), demo mode
GRKIT_DEMO=1 cargo run
# or
cargo run -- --demo

# iOS simulator
GRKIT_DEMO=1 nsh run --target ios
nsh package --target ios-simulator
```

### Demo failures

Force an error path in demo mode:

```sh
GRKIT_DEMO=1 GRKIT_DEMO_FAIL=card_full cargo run
# also: refused | not_confirmed | connection_lost
```

### Demo screens (screenshots)

With demo on, jump straight to a design screen (sheet / alert already open).
Mapping lives in `src/store/demo_screen.rs`.

```sh
GRKIT_DEMO=1 GRKIT_DEMO_SCREEN=A5 cargo run
GRKIT_DEMO=1 cargo run -- --demo-screen B3
# or alone (also turns demo on):
cargo run -- --demo-screen C4
```

| Id | Screen |
|----|--------|
| A1 | First launch |
| A2 | Camera nearby sheet |
| A3 | Add a camera sheet |
| A4 | Connecting sheet |
| A5 | Update available |
| A6 | Up to date |
| A7 | Settings |
| B1 | Software Update |
| B2 | Sending (~62%) |
| B3 | Confirm on camera |
| B4 | Restart |
| B5 | Installed |
| C1 | Software Update (roll back) |
| C2 | Roll back alert |
| C3 | Roll back confirm on camera |
| C4 | Card full |
| C5 | Not confirmed |

Unknown ids fall back to the normal demo home.

## Layout

- `src/domain/` — camera model, update state machine, `CameraLink`, `FakeCamera`
- `src/store/` — tabs, sheets, demo wiring (`demo_screen.rs` for screenshot ids)
- `src/shell/` — tab bar and nav stacks
- `src/pages/` — Camera, Settings, Software Update, update sheets
- `src/ui/` — shared chrome
- `locales/app.yml` — English and Simplified Chinese (kept with `src/strings.rs`)
- `assets/` — camera line art, confirm screenshots, GR Snap icon
