# homarr-container-adapter Specification

## Overview

The homarr-container-adapter is a Rust service that bridges Docker container metadata with the Homarr dashboard. It performs two primary functions:

1. **First-boot setup**: Configures Homarr on initial installation with HaLOS branding
2. **Container auto-discovery**: Monitors Docker containers and syncs labeled containers to Homarr

## Requirements

### Functional Requirements

#### First-Boot Setup (FR-1)
- FR-1.1: Complete Homarr onboarding wizard automatically
- FR-1.2: Create admin user with credentials from branding config
- FR-1.3: Configure server settings (analytics, crawling)
- FR-1.4: Create default dashboard board with configured layout
- FR-1.5: Add Cockpit app tile to dashboard
- FR-1.6: Set dashboard as home board
- FR-1.7: Apply theme color scheme

#### Container Discovery (FR-2)
- FR-2.1: Monitor Docker daemon for container changes
- FR-2.2: Parse `homarr.*` labels from containers
- FR-2.3: Add discovered apps to Homarr dashboard
- FR-2.4: Respect user removal decisions (don't re-add removed apps)
- FR-2.5: Track sync state persistently

### Non-Functional Requirements

- NFR-1: Run as systemd service
- NFR-2: Minimal resource footprint
- NFR-3: Graceful error handling with retries
- NFR-4: Structured logging with configurable verbosity

## Docker Label Schema

Containers opt-in to Homarr visibility using labels:

| Label | Required | Description |
|-------|----------|-------------|
| `homarr.enable` | Yes | Must be "true" to enable |
| `homarr.name` | Yes | Display name in Homarr |
| `homarr.url` | Yes | URL to access the app (used for clicking) |
| `homarr.description` | No | App description |
| `homarr.icon` | No | Icon URL |
| `homarr.category` | No | Category grouping |

**Note:** The `pingUrl` for health checks is automatically derived by replacing the hostname with `host.docker.internal`. This allows Homarr (running in a container) to reach apps on the host for health checks while the display URL can use the external hostname (e.g., `halos.local`). Requires `extra_hosts: ["host.docker.internal:host-gateway"]` in Homarr's docker-compose.yml.

Example:
```yaml
labels:
  homarr.enable: "true"
  homarr.name: "Signal K"
  homarr.url: "http://localhost:3000"
  homarr.description: "Marine data server"
  homarr.icon: "https://signalk.org/images/signalk-logo-transparent.png"
  homarr.category: "Marine"
```

## Configuration

### Adapter Configuration (`/etc/homarr-container-adapter/config.toml`)

```toml
# Homarr API endpoint
homarr_url = "http://localhost:7575"

# Path to branding configuration
branding_file = "/etc/halos-homarr-branding/branding.toml"

# State persistence file
state_file = "/var/lib/homarr-container-adapter/state.json"

# Docker socket path
docker_socket = "/var/run/docker.sock"

# Bootstrap API key file (from halos-homarr-branding package)
bootstrap_api_key_file = "/etc/halos-homarr-branding/bootstrap-api-key"

# Authelia users database file
authelia_users_db = "/var/lib/container-apps/authelia-container/data/users_database.yml"

# Periodic sync interval in seconds (for watch mode)
sync_interval = 300

# Startup delay in seconds before first sync (for watch mode)
startup_delay = 10

# Enable debug logging
debug = false
```

### Branding Configuration

See halos-homarr-branding package for branding configuration schema.

## API Interactions

The adapter uses Homarr's tRPC API (not REST). Key endpoints:

### Onboarding
- `GET /api/trpc/onboard.currentStep` - Get current step
- `POST /api/trpc/onboard.nextStep` - Advance to next step
- `POST /api/trpc/user.initUser` - Create initial user
- `POST /api/trpc/serverSettings.initSettings` - Configure settings

### Authentication (API Key)

The adapter uses API key authentication via the `Authorization: Bearer <api_key>` header.
This allows Homarr to run with `AUTH_PROVIDERS="oidc"` (no credentials login).

**API Key Rotation Flow (First Boot):**
1. Read bootstrap API key from `/etc/halos-homarr-branding/bootstrap-api-key`
2. Create new permanent API key via `POST /api/trpc/apiKeys.create`
3. Delete bootstrap key via `POST /api/trpc/apiKeys.delete`
4. Store permanent key in state file

Key endpoints:
- `POST /api/trpc/apiKeys.create` - Create new API key
- `POST /api/trpc/apiKeys.delete` - Delete API key by ID

### Board Management
- `GET /api/trpc/board.getBoardByName` - Get board by name
- `POST /api/trpc/board.createBoard` - Create new board
- `POST /api/trpc/board.saveBoard` - Save board with items
- `POST /api/trpc/board.setHomeBoard` - Set home board

### App Management
- `POST /api/trpc/app.create` - Create app
- `GET /api/trpc/app.all` - List all apps

## State Management

The adapter maintains state in a JSON file:

```json
{
  "version": "1.0",
  "first_boot_completed": true,
  "authelia_sync_completed": true,
  "api_key": "abc123.randomtoken...",
  "removed_apps": ["http://localhost:3000"],
  "last_sync": "2025-01-15T10:30:00Z",
  "discovered_apps": {
    "http://localhost:3000": {
      "name": "Signal K",
      "container_id": "abc123def456",
      "added_at": "2025-01-15T10:30:00Z"
    }
  }
}
```

## CLI Interface

```
homarr-container-adapter <COMMAND>

Commands:
  setup   Run first-boot setup (onboarding + board creation)
  sync    Sync Docker containers with Homarr
  status  Show current adapter status

Options:
  -c, --config <FILE>  Config file path [default: /etc/homarr-container-adapter/config.toml]
  -d, --debug          Enable debug logging
  -h, --help           Print help
  -V, --version        Print version
```

## Error Handling

- Connection failures: Retry with exponential backoff
- API errors: Log and continue (don't fail entire sync)
- Configuration errors: Fail fast with clear error message
- State corruption: Reset to defaults with warning

## Security Considerations

- **API Key Storage**: Permanent API key stored in state file (file permissions: 600)
- **Bootstrap Key**: Well-known bootstrap key rotated on first boot (window of vulnerability: seconds)
- **No Credentials Login**: Homarr runs with `AUTH_PROVIDERS="oidc"` only
- **Docker Socket**: Access required (add to docker group)
