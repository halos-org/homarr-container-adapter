# homarr-container-adapter - Agent Instructions

## Repository Purpose

Rust service that bridges app definitions with the Homarr dashboard. Two main functions:
1. First-boot setup: Complete Homarr onboarding with HaLOS branding
2. App registry sync: Sync apps from `/etc/halos/webapps.d/*.toml` to Homarr

## Key Files

- `src/main.rs` - CLI entry point with setup/sync/status commands
- `src/homarr.rs` - Homarr tRPC API client
- `src/registry.rs` - App registry loader (TOML files)
- `src/branding.rs` - Parse branding.toml from halos-homarr-branding
- `src/state.rs` - Persistent state (JSON)
- `src/config.rs` - Adapter configuration
- `docs/SPEC.md` - Functional requirements
- `docs/ARCHITECTURE.md` - System design

## Technical Notes

### Homarr API
- Uses **tRPC**, not REST
- All mutations require JSON wrapper: `{"json": {...}}`
- Session-based auth (cookies), not API keys for mutations
- Onboarding flow: start → user → settings → finish

### App Registry
Apps are defined in `/etc/halos/webapps.d/*.toml` files with:
- `name` (required) - Display name
- `url` (required) - App URL (validated)
- `description`, `icon_url`, `category` (optional)
- `[layout]` section for position/size: `priority`, `width`, `height`, `x_offset`, `y_offset`

Priority ranges: 00-09 (system), 10-29 (core), 30-49 (marine), 50-69 (user), 70-99 (external)

Note: If only one of `x_offset`/`y_offset` is specified, both are auto-calculated.

### Dependencies
- `reqwest` with cookies for HTTP
- `tokio` async runtime
- `clap` for CLI

## Build Commands

```bash
cargo build              # Debug build
cargo build --release    # Release build
cargo test               # Run tests
cargo clippy             # Lint
```

## Packaging

Native Debian package built with cargo-deb. See `debian/` directory.

## Related Repos

- `halos-homarr-branding` - Provides `/etc/halos-homarr-branding/branding.toml`
- `halos-core-containers` - Defines homarr-container
