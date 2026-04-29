# Contributing & Development

## Directory Structure

When running spyweb, it interacts with the following files and directories in the same folder as the executable:

```text
spyweb-folder/
├── spyweb                 # The main executable
├── jobs.toml              # Quick multi-job config
├── jobs/                  # Per-job directories (config.toml + hook.lua)
├── ui/                    # Admin dashboard HTML/CSS
├── data/                  # Embedded database (auto-created on first run)
├── docs/                  # Interactive documentation
└── examples/              # Example configurations
```

The Rust source code lives in `src/`.

## Hot Reload
spyweb watches `jobs.toml` and all `jobs/*/config.toml` files. Edit a config or Lua script, save, and spyweb automatically drops running jobs and respawns with the new configuration — no restart needed. Lua state is fully reset on reload.

## Building from Source

If you prefer to compile spyweb from source instead of using the pre-compiled binary, ensure you have Rust installed:

```bash
git clone https://github.com/spyweb-rs/spyweb.git
cd spyweb

# Build CLI binary only
cargo build --release

# Build both CLI and Tray binaries
cargo build --release --features tray

# Cross-compile for Windows (with Tray icon)
cargo build --release --target x86_64-pc-windows-gnu --features tray
```

### Power User: Enabling Lua 5.4 (C-Modules)
By default, SpyWeb uses **Luau** for stability and speed. If you need to support external C-libraries (via `require`), you can switch to standard Lua 5.4 using cargo features:

```bash
# Build CLI with Lua 5.4 support
cargo build --release --no-default-features --features lua54
```

If you also want the Tray app with Lua 5.4:
```bash
cargo build --release --no-default-features --features "lua54 tray"
```
*Note: Enabling Lua 5.4 allows scripts to load external binary modules (C-bindings). A badly compiled or wrong-architecture `.so` or `.dll` will kill the entire process via a Segmentation Fault—no Rust error handling can save that from happening. Use with caution.*

Alternatively, for active development (especially if you are on **WSL** and tired of typing long cross-platform commands), you can use the included helper script. It handles the target flags, binary paths, and even spawns the Windows `.exe` from Linux for you:

```bash
./run ld          # linux-debug
./run lr          # linux-release
./run wd          # win-debug
./run wr          # win-release
./run w           # cargo-watch (auto-rebuild on change)
```
