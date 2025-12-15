# homarr-container-adapter

Adapter service for Homarr dashboard: handles first-boot setup with HaLOS branding and auto-discovers Docker containers for the dashboard.

## Features

- **First-boot setup**: Completes Homarr onboarding, creates admin user, configures theming
- **Container auto-discovery**: Monitors Docker containers with `homarr.*` labels
- **State persistence**: Remembers removed apps, tracks sync status

## Installation

```bash
# Install the Debian package
sudo apt install homarr-container-adapter
```

## Usage

```bash
# Run first-boot setup (usually called by systemd)
homarr-container-adapter setup

# Sync Docker containers with Homarr
homarr-container-adapter sync

# Check adapter status
homarr-container-adapter status
```

## Docker Labels

Add these labels to containers for Homarr visibility:

```yaml
labels:
  homarr.enable: "true"
  homarr.name: "My App"
  homarr.url: "http://localhost:8080"
  homarr.description: "App description"
  homarr.icon: "https://example.com/icon.png"
  homarr.category: "Tools"
```

## Configuration

Adapter config: `/etc/homarr-container-adapter/config.toml`
Branding config: `/etc/halos-homarr-branding/branding.toml` (from halos-homarr-branding package)

## Building

```bash
# Build debug
cargo build

# Build release
cargo build --release

# Run tests
cargo test
```

## Related Packages

- `homarr-container` - Homarr dashboard container
- `halos-homarr-branding` - HaLOS branding configuration

## License

MIT License - Hat Labs Ltd
