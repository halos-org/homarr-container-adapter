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
- Per-board removed apps tracking
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

### Container Sync Flow (Multi-Board)

```
┌────────┐     ┌──────────┐     ┌────────┐     ┌───────┐
│ Docker │────►│ adapter  │────►│ Homarr │────►│ State │
│ daemon │     │  (sync)  │     │  API   │     │ file  │
└────────┘     └──────────┘     └────────┘     └───────┘

1. Query Docker for running containers
2. Filter containers with homarr.enable=true
3. Parse homarr.* labels
4. Discover writable boards (query fresh each sync)
5. For each discovered app:
   a. Check if already in global app registry
   b. If not, create app in global registry
   c. Record in discovered_apps
6. For each writable board:
   a. For each discovered app:
      - If app exists on board but marked removed: clear removed flag
      - If removed from this board: skip
      - If already on board: skip
      - Otherwise: add app reference to board
7. Update last_sync timestamp
8. Save state
```

**Key design points:**
- Apps exist in a global registry, boards reference them
- Per-board removal tracking respects user intent at board level
- If user manually re-adds an app, the removed flag is cleared
- Writable boards = boards where sync user has "modify" or "full" permission

## Configuration Hierarchy

```
/etc/homarr-container-adapter/config.toml  (adapter config)
         │
         └──► branding_file ──► /etc/halos-homarr-branding/branding.toml
         │
         └──► state_file ──► /var/lib/homarr-container-adapter/state.json
```

## Homarr API Interactions

The adapter uses Homarr's tRPC API. While Homarr exposes OpenAPI for read-only operations, mutations (board creation, app creation, etc.) require tRPC.

### Authentication

API key authentication via `ApiKey: <api_key>` header.

**API Key Ownership:** The bootstrap API key (and rotated permanent key) is owned by the `halos-sync` service user, not the human admin user. This separates programmatic API access from human OIDC login.

**Rotation Flow:**
1. Read bootstrap key from `/etc/halos-homarr-branding/bootstrap-api-key`
2. Create permanent key, delete bootstrap key
3. Store permanent key in state file

### Homarr Data Model

Apps are stored in a **global registry**. Boards reference apps via items:

```
┌─────────────┐         ┌─────────────┐         ┌─────────────┐
│   Board     │ 1:N     │    Item     │ N:1     │    App      │
│             │────────►│ (kind=app)  │────────►│  (global)   │
│ - id        │         │ - boardId   │         │ - id        │
│ - name      │         │ - appId     │         │ - name      │
│ - sections  │         │ - layout    │         │ - href      │
└─────────────┘         └─────────────┘         └─────────────┘
```

### Board Permissions

The `board.getAllBoards` response includes permission arrays:

- `userPermissions`: `[{userId, permission}]`
- `groupPermissions`: `[{groupId, permission}]`

Permission levels: `view`, `modify`, `full`

The adapter syncs to boards where the sync user has `modify` or `full` permission (via direct user permission or group membership).

## State Management

The adapter maintains state in `/var/lib/homarr-container-adapter/state.json`:

```json
{
  "version": "1.0",
  "first_boot_completed": true,
  "authelia_sync_completed": true,
  "api_key": "permanent-key...",
  "last_sync": "2025-01-15T10:30:00Z",
  "discovered_apps": {
    "http://localhost:3000": {
      "name": "Signal K",
      "container_id": "abc123def456",
      "added_at": "2025-01-15T10:30:00Z"
    }
  },
  "removed_apps_by_board": {
    "board-id-abc": ["http://localhost:3000"],
    "board-id-xyz": []
  }
}
```

**Per-board removal tracking:** When a user removes an app from a board, the adapter records this per-board. Removing from Board A doesn't affect Board B. If the user manually re-adds an app, the adapter detects this and clears the removed flag.

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

## Seed Database

The `halos-homarr-branding` package provides a pre-configured SQLite database with:

**Two users:**

| User | Purpose | Provider | Group |
|------|---------|----------|-------|
| `halos-sync` | API key ownership, programmatic access | oidc | admins |
| `admin` | Human admin OIDC login | oidc | admins |

Both users have `provider=oidc` to enable OIDC account linking when users log in via Authelia.

**Bootstrap API key:** Owned by `halos-sync`, rotated on first boot by the adapter.

**Why two users:** Separates concerns between programmatic API access (halos-sync) and human admin access (admin). The halos-sync user owns the API key and performs container sync operations.

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

- Real-time container events (Docker events API) - see issue #30
- Category/section management
- Icon caching
- Health check integration
