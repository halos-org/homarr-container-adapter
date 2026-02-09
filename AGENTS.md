# homarr-container-adapter - Agent Instructions

## CRITICAL: Build Commands

> **YOU MUST USE THE RUN SCRIPT FOR ALL BUILD AND DEPLOY OPERATIONS.**
>
> Do NOT run `cargo build` or `docker` commands directly.
> Always use `./run <command>` instead.

```bash
# Cross-compile for ARM64 (Raspberry Pi) - THIS IS THE MAIN BUILD COMMAND
./run build-arm64

# Build and deploy to test server in one step
./run deploy-build

# Deploy existing ARM64 build to test server
./run deploy

# View all available commands
./run help
```

**Why this matters:**
- The target system is ARM64 (Raspberry Pi), not x86_64
- Direct `cargo build` creates unusable binaries for the target
- The run script handles Docker-based cross-compilation automatically
- Deploy commands handle the full workflow: build, copy, install, restart

## Repository Purpose

Rust service that bridges app definitions with the Homarr dashboard. Two main functions:
1. First-boot setup: Complete Homarr onboarding with HaLOS branding
2. App registry sync: Sync apps from `/etc/halos/webapps.d/*.toml` to Homarr

## Key Files

- `src/main.rs` - CLI entry point with setup/sync/status/watch commands
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
- `bollard` for Docker event streaming (watch mode)

## Development Commands

**ALWAYS use the run script:**

```bash
./run build              # Debug build (native, for local testing only)
./run build-arm64        # Cross-compile for ARM64 (USE THIS FOR DEPLOYMENT)
./run test               # Run tests
./run lint               # Run fmt-check + clippy
./run deploy-build       # Build ARM64 and deploy to halos.local
./run logs-follow        # Follow service logs on test server
./run help               # Show all available commands
```

## Versioning

**IMPORTANT**: Always use `./run bumpversion` to change versions. Never edit version files manually.

```bash
./run bumpversion patch    # Bump patch version (0.3.0 -> 0.3.1)
./run bumpversion minor    # Bump minor version (0.3.0 -> 0.4.0)
./run bumpversion major    # Bump major version (0.3.0 -> 1.0.0)
```

The command automatically commits the version change. Just push afterwards:
```bash
git push
```

**Note:** The working directory must be clean before bumping. This ensures atomic, isolated version commits.

**Version files kept in sync by bumpversion:**
- `VERSION` - Canonical source, read by CI
- `Cargo.toml` - Rust package version

**Note:** `debian/changelog` is generated dynamically by CI from the VERSION file.

**CI Enforcement**: PRs that change package-affecting files must include a VERSION bump or CI will fail. Changes to docs, tests, CI config, and dev tooling are automatically excluded.

## Packaging

Native Debian package built with cargo-deb. See `debian/` directory.

## Related Repos

- `halos-homarr-branding` - Provides `/etc/halos-homarr-branding/branding.toml`
- `halos-core-containers` - Defines homarr-container
