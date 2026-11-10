# mln-rs-globe

A spinning globe with an atmosphere and stars, drawn by [mln-rs](https://github.com/louwers/mln-rs), the Rust port of the MapLibre Native core, in a [winit](https://github.com/rust-windowing/winit) window.

## Install

On macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -LsSf https://github.com/johncarmack1984/mln-rs-globe/releases/latest/download/mln-rs-globe-installer.sh | sh
```

On Windows, in PowerShell:

```powershell
powershell -ExecutionPolicy Bypass -c "irm https://github.com/johncarmack1984/mln-rs-globe/releases/latest/download/mln-rs-globe-installer.ps1 | iex"
```

Both put `mln-rs-globe` in Cargo's bin folder (`~/.cargo/bin`, or `%USERPROFILE%\.cargo\bin` on Windows). Each release also has the archives themselves, for Apple silicon and Intel Macs, x64 and ARM Linux, and x64 Windows. The builds aren't signed, so one downloaded in a browser needs `xattr -d com.apple.quarantine mln-rs-globe` on macOS, and "More info", then "Run anyway" on Windows. Linux needs Vulkan drivers and X11 or Wayland.

## Run

```sh
mln-rs-globe
```

Drag to turn the globe, scroll to zoom, space pauses the spin, S saves a screenshot to the current directory, Esc quits.

```text
--style URL|FILE   Draw this style instead of the built-in one. It gets the
                   globe projection and an atmosphere if it has neither
--lat DEGREES      Latitude to start over [default: 20]
--lon DEGREES      Longitude to start over [default: -30]
--zoom ZOOM        Zoom [default: 1.6]
--speed DEGREES    How fast the globe turns, in degrees a second; 0 holds it
                   still [default: 6]
--screenshot FILE  Save a PNG once the map has loaded, then quit
```

For example, OpenFreeMap's Liberty style on the globe: `mln-rs-globe --style https://tiles.openfreemap.org/styles/liberty`.

## Build

```sh
cargo run
```

The globe and the sky aren't released yet, so `mln-rs` comes from `globe-pr`, the branch of the pull request to louwers/mln-rs, at the commit `Cargo.lock` pins. `cargo update -p mln-rs` moves to the branch's latest. To build against a local clone instead, give the dependency a `path` to it. Once the globe is on crates.io, the dependency becomes a version from there.

## Release

Bump `version` in `Cargo.toml`, commit, then push a tag for it:

```sh
git tag v0.1.1
git push origin v0.1.1
```

The Release workflow builds the five targets and publishes a GitHub release with their archives and the two installers. [dist](https://github.com/axodotdev/cargo-dist) generates `.github/workflows/release.yml` from `dist-workspace.toml`: change the targets or installers there, then run `dist generate`.

## How it works

- `style.json` is the built-in style: Natural Earth's shaded relief with OpenFreeMap's water and borders, on the `globe` projection with a `sky` whose `atmosphere-blend` draws the atmosphere. Its `viewport` light keeps the sun up and to the left of the camera, a little behind the globe.
- `src/main.rs` opens the window and the map, turns the globe a little each frame (`jump_to` a center further west, so the Earth turns east), and runs the map's `RunLoop` once a frame. The loop's waker wakes the window when work arrives between frames.
- `src/frontend.rs` is the map's renderer frontend, as in mln-rs's GLFW app: it keeps the map's latest update until the next frame draws it.
- `src/target.rs` is the renderer backend. mln-rs's own window backend draws the map straight into the window; this one has it draw into a texture, then draws the window from `src/stars.wgsl`.
- `src/stars.wgsl` draws the stars and a faint Milky Way, with the map's frame over them. Around the globe the map is transparent and its atmosphere translucent, so space shows there and dims behind the glow.

## Data

The built-in style's tiles come from [OpenFreeMap](https://openfreemap.org): the relief is [Natural Earth](https://www.naturalearthdata.com) (public domain), the water and borders are [OpenMapTiles](https://openmaptiles.org) data © [OpenStreetMap](https://www.openstreetmap.org/copyright) contributors.
