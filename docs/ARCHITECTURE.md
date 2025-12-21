# homarr-container-adapter Architecture

## System Context

```
┌─────────────────────────────────────────────────────────────────┐
│                        HaLOS System                              │
│                                                                  │
│  ┌──────────────────┐    ┌──────────────────────────────────┐  │
│  │  Docker Daemon   │    │  homarr-container-adapter        │  │
│  │                  │◄───│                                   │  │
│  │  - Containers    │    │  - First-boot setup              │  │
│  │  - Labels        │    │  - Container discovery           │  │
│  └──────────────────┘    │  - Homarr sync                   │  │
│          ▲               └──────────────────────────────────┘  │
│          │                         │                            │
│          │                         ▼                            │
│  ┌───────┴──────────┐    ┌──────────────────────────────────┐  │
│  │  Marine Apps     │    │  Homarr Dashboard                │  │
│  │  - Signal K      │    │  (localhost:7575)                │  │
│  │  - Grafana       │    │                                   │  │
│  │  - InfluxDB      │    │  ┌─────────┐ ┌─────────┐        │  │
│  └──────────────────┘    │  │ Cockpit │ │Signal K │ ...    │  │
│                          │  └─────────┘ └─────────┘        │  │
│                          └──────────────────────────────────┘  │
│                                                                  │
│  ┌──────────────────┐    ┌──────────────────────────────────┐  │
│  │ halos-homarr-    │    │  State File                      │  │
│  │ branding         │    │  /var/lib/homarr-container-      │  │
│  │                  │    │  adapter/state.json              │  │
│  │ - branding.toml  │    │  - api_key (permanent)           │  │
│  │ - bootstrap-api- │    │  - first_boot_completed          │  │
│  │   key            │    │  - discovered_apps               │  │
│  │ - db-seed.sqlite │    └──────────────────────────────────┘  │
│  │ - logo.svg       │                                          │
│  └──────────────────┘                                          │
└─────────────────────────────────────────────────────────────────┘
```

## Module Structure

```
src/
├── main.rs        # CLI entry point, command dispatch
├── config.rs      # Adapter configuration loading
├── branding.rs    # Branding configuration types
├── homarr.rs      # Homarr API client
├── docker.rs      # Docker container discovery
├── state.rs       # Persistent state management
└── error.rs       # Error types
```

### Module Responsibilities

#### main.rs
- CLI argument parsing (clap)
- Command dispatch (setup, sync, status)
- Logging initialization
- Error handling and exit codes

#### config.rs
- Load adapter configuration from TOML
- Provide defaults for optional settings
- Path resolution

#### branding.rs
- Parse branding.toml from halos-homarr-branding
- Type definitions for identity, theme, credentials, board config
- Validation of branding settings

#### homarr.rs
- HTTP client with API key authentication
- tRPC API wrapper functions
- API key rotation (bootstrap → permanent)
- Onboarding flow automation
- Board and app management

#### docker.rs
- Docker API client (bollard)
- Container listing and filtering
- Label parsing for homarr.* namespace

#### state.rs
- JSON state persistence
- First-boot completion tracking
- API key storage (permanent key after rotation)
- Removed apps tracking
- Sync timestamp management

#### error.rs
- Custom error types
- Error conversion traits
- Result type alias

## Data Flow

### First-Boot Setup Flow

```
┌─────────┐     ┌──────────┐     ┌────────┐     ┌───────┐
│ systemd │────►│ adapter  │────►│ Homarr │────►│ State │
│ service │     │ (setup)  │     │  API   │     │ file  │
└─────────┘     └──────────┘     └────────┘     └───────┘
                     │
                     ▼
              ┌──────────────┐
              │  branding +  │
              │ bootstrap-   │
              │ api-key      │
              └──────────────┘

1. Load branding configuration
2. Check if permanent API key exists in state
3. If no permanent key (first boot):
   a. Read bootstrap API key from halos-homarr-branding package
   b. Use bootstrap key to create new permanent API key
   c. Delete bootstrap key from Homarr
   d. Store permanent key in state
4. Check onboarding status (should be complete from seed database)
5. If onboarding not complete:
   a. Complete onboarding wizard
   b. Configure settings
6. Create/update board with branding
7. Sync Authelia credentials if needed
8. Mark first_boot_completed = true
9. Save state
```

### Container Sync Flow

```
┌────────┐     ┌──────────┐     ┌────────┐     ┌───────┐
│ Docker │────►│ adapter  │────►│ Homarr │────►│ State │
│ daemon │     │  (sync)  │     │  API   │     │ file  │
└────────┘     └──────────┘     └────────┘     └───────┘

1. Query Docker for running containers
2. Filter containers with homarr.enable=true
3. Parse homarr.* labels
4. For each discovered app:
   a. Check if in removed_apps (skip if yes)
   b. Check if already in Homarr (skip if yes)
   c. Create app in Homarr
   d. Add to board
   e. Record in discovered_apps
5. Update last_sync timestamp
6. Save state
```

## Configuration Hierarchy

```
/etc/homarr-container-adapter/config.toml  (adapter config)
         │
         └──► branding_file ──► /etc/halos-homarr-branding/branding.toml
         │
         └──► state_file ──► /var/lib/homarr-container-adapter/state.json
```

## Error Handling Strategy

```
┌────────────────────────────────────────────────────────────┐
│                    Error Categories                         │
├────────────────────────────────────────────────────────────┤
│ Config Errors     → Fail fast, clear message               │
│ Connection Errors → Retry with backoff, eventual failure   │
│ API Errors        → Log warning, continue operation        │
│ State Errors      → Reset to defaults, warn user           │
└────────────────────────────────────────────────────────────┘
```

## Dependencies

### Runtime Dependencies
- Docker daemon (socket access)
- Homarr container running
- halos-homarr-branding package installed

### Build Dependencies
- Rust toolchain (1.70+)
- OpenSSL development headers
- pkg-config

## Security Model

1. **File Permissions**
   - Config files: root:root 644
   - Bootstrap API key: root:root 600
   - State file (contains permanent API key): root:root 600

2. **Authentication**
   - API key authentication (no credentials login)
   - Bootstrap key rotated on first boot (minimal exposure window)
   - Homarr runs with AUTH_PROVIDERS="oidc" only

3. **Network**
   - Localhost-only communication with Homarr
   - No external network access required

4. **Docker Access**
   - Read-only container listing
   - Requires docker group membership or root

## Future Considerations

- Real-time container events (Docker events API)
- Category/section management
- Icon caching
- Health check integration
- Multi-board support
