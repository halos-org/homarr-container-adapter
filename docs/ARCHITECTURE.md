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
│  │ homarr-branding- │    │  State File                      │  │
│  │ halos            │    │  /var/lib/homarr-container-      │  │
│  │                  │    │  adapter/state.json              │  │
│  │ - branding.toml  │    └──────────────────────────────────┘  │
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
- HTTP client with cookie-based sessions
- tRPC API wrapper functions
- Onboarding flow automation
- Board and app management

#### docker.rs
- Docker API client (bollard)
- Container listing and filtering
- Label parsing for homarr.* namespace

#### state.rs
- JSON state persistence
- First-boot completion tracking
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
              │   branding   │
              │    config    │
              └──────────────┘

1. Load branding configuration
2. Check if first_boot_completed in state
3. If not completed:
   a. Wait for Homarr to be ready
   b. Complete onboarding wizard
   c. Create admin user
   d. Configure settings
   e. Create board with Cockpit tile
   f. Set as home board
   g. Mark first_boot_completed = true
4. Save state
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
   - Branding with credentials: root:root 600
   - State file: root:root 600

2. **Network**
   - Localhost-only communication with Homarr
   - No external network access required

3. **Docker Access**
   - Read-only container listing
   - Requires docker group membership or root

## Future Considerations

- Real-time container events (Docker events API)
- Category/section management
- Icon caching
- Health check integration
- Multi-board support
