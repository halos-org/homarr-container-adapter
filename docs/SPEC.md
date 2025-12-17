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

**Note:** The `pingUrl` for health checks is automatically derived by replacing the hostname with `localhost`. This allows Homarr (running in a container) to reach apps for health checks while the display URL can use the external hostname (e.g., `halos.local`).

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

### Authentication
- `GET /api/auth/csrf` - Get CSRF token
- `POST /api/auth/callback/credentials` - Login with credentials

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
  "removed_apps": ["container-id-1", "container-id-2"],
  "last_sync": "2025-01-15T10:30:00Z",
  "discovered_apps": {
    "container-id": {
      "name": "Signal K",
      "url": "http://localhost:3000",
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

- Credentials stored in branding config (file permissions: 600)
- Session cookies handled in-memory only
- Docker socket access required (add to docker group)
