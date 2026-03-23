# bevy_metalfx

Bevy plugin for Apple MetalFX upscaling and frame interpolation.

Renders your scene at a lower resolution and uses MetalFX's ML-based upscaling
to reconstruct a full-resolution image, improving performance on Apple Silicon Macs.

## Features

| Mode | Description | macOS Version | Cargo Feature |
|------|-------------|---------------|---------------|
| **Spatial** | Single-frame ML upscaling | 13+ | `spatial` (default) |
| **Temporal** | Multi-frame upscaling with motion vectors | 13+ | `temporal` |
| **Frame Interpolation** | Generate intermediate frames | 26+ (Metal 4) | `frame-interpolation` |

## Quick Start

```toml
[dependencies]
bevy_metalfx = "0.1"
```

```rust
use bevy::prelude::*;
use bevy_metalfx::{MetalFxPlugin, MetalFxMode};

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .add_plugins(MetalFxPlugin {
            render_scale: 0.5,  // Render at half resolution
            mode: MetalFxMode::Spatial,
        })
        .run();
}
```

On non-macOS platforms, the plugin detects that MetalFX is unavailable and
gracefully disables itself — no `#[cfg]` guards needed in your app code.

## How It Works

```text
Scene render (low-res via MainPassResolutionOverride)
  -> MetalFX upscale (raw Metal encode on command buffer)
    -> Full-res output texture
      -> Blit to swapchain
```

The plugin inserts a render graph node after Bevy's built-in `UpscalingNode`.
It extracts raw Metal textures and command buffers from wgpu-hal and encodes
the MetalFX upscale pass directly, then blits the result to the swapchain.

## Feature Flags

```toml
# Spatial only (default)
bevy_metalfx = "0.1"

# Temporal upscaling (adds motion vector + depth prepass)
bevy_metalfx = { version = "0.1", features = ["temporal"] }

# Frame interpolation (requires macOS 26+)
bevy_metalfx = { version = "0.1", features = ["frame-interpolation"] }
```

## Bevy Compatibility

| bevy_metalfx | Bevy |
|-------------|------|
| 0.1 | 0.18 |

## Platform Support

- **macOS 13+** (Apple Silicon): Full support
- **macOS < 13**: Plugin disables itself gracefully
- **Linux / Windows**: Plugin disables itself; type stubs available for cross-platform code

## License

MIT OR Apache-2.0
