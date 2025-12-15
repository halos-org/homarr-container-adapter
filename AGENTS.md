# homarr-container-adapter - Agent Instructions

## Repository Purpose

Rust service that bridges Docker containers with the Homarr dashboard. Two main functions:
1. First-boot setup: Complete Homarr onboarding with HaLOS branding
2. Container sync: Auto-discover containers with `homarr.*` labels

## Key Files

- `src/main.rs` - CLI entry point with setup/sync/status commands
- `src/homarr.rs` - Homarr tRPC API client
- `src/docker.rs` - Docker container discovery (bollard)
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

### Docker Labels
Containers opt-in with `homarr.enable=true` plus:
- `homarr.name` (required)
- `homarr.url` (required)
- `homarr.description`, `homarr.icon`, `homarr.category` (optional)

### Dependencies
- `reqwest` with cookies for HTTP
- `bollard` for Docker API
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
