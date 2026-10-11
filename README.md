# ts-installer

An installer and launcher for **The Specialists 3.0**, built with [GPUI](https://gpui.rs) and
[gpui-component](https://github.com/longbridge/gpui-component). Runs on Windows and Linux.

## What it does

On first start it asks how you want to play:

- **Use my Half-Life install.** Pick a Half-Life folder (Steam libraries are detected
  automatically). TS goes into its `ts/` folder.
- **Install Xash3D FWGS.** Downloads [Xash3D FWGS](https://github.com/FWGS/xash3d-fwgs)
  (`continuous` release) into a folder of your choice. The Half-Life `valve` data is either
  downloaded from [xash3d-ts](https://github.com/The-Specialists-Archive/xash3d-ts) or copied
  from a local Half-Life install.

TS itself is `ts-3-final.exe`, the original client installer from
[ModDB](https://www.moddb.com/mods/the-specialists/downloads/the-specialists-v30-client-windows).
ModDB sits behind a Cloudflare browser check that rejects scripted downloads, so the launcher
downloads the copy attached to this repository's
[`ts-3.0` release](https://github.com/the-specialists-mod/ts-installer/releases/tag/ts-3.0)
(it is too big to commit), falling back to the identical file on the
[Internet Archive](https://archive.org/details/ts-3.0). Either way the download is checked
against the known MD5 (`e0fe976a8d9d59393e66b9f1cb94c256`). A local copy works too: the
installer `.exe`, or a folder containing a `ts` directory.

The installer is a Clickteam Install Creator package. `src/clickteam.rs` unpacks it natively
(format notes from [cicdec](https://github.com/Bioruebe/cicdec)), so nothing has to run under
Windows or Wine to install TS.

### Game libraries

TS 3.0 only ever shipped a 32-bit Windows client (`client.dll`). The launcher offers two choices:

| Choice | Xash3D build | Linux | Windows |
|---|---|---|---|
| Original TS 3.0 DLLs | `win32-i386` | runs through Wine | native |
| [Rust port](https://github.com/CrunchyMonkies/rust-hlmod-the-specialists) | native 64-bit | native | native |

For the Rust port, point the launcher at its `dist/` folder (from `cargo xtask dist`), one
platform's `ts/` folder, or a `.zip` / `.tar.gz` URL with the same layout. With a Half-Life
install, the `*-i686` libraries are used. On Linux, a Half-Life install with the original DLLs is
launched through Steam, which needs Proton for it.

After installing, the launcher shows a play screen with launch options (windowed, console, extra
arguments, and Wine command / `WINEPREFIX` where relevant). Settings are saved in
`<config dir>/ts-launcher/config.json`; downloads are cached in `<cache dir>/ts-launcher/` and
interrupted downloads resume.

### Player models

The Player models tab lists `ts/models/player/<name>/` with each model's preview picture and
readme, and draws the selected `.mdl` in 3D (drag to turn; `src/mdl.rs` parses studio model v10
and renders it in software). "Use" sets the engine's `model` cvar in `ts/config.cfg`, which is
how TS picks your model. Team play servers (team deathmatch, The One) override it with the team's
models. "Add model…" copies a folder with a `.mdl` (and its `.bmp` / `.txt`) into
`models/player/<mdl name>/`.

## Command line

The same installer runs without the UI:

```sh
ts-launcher install --root ~/games/ts --xash                      # everything downloaded
ts-launcher install --root ~/games/ts --xash \
    --valve-from ~/.local/share/Steam/steamapps/common/Half-Life \
    --ts ~/Downloads/ts-3-final.exe \
    --port ~/src/rust-hlmod-the-specialists/dist                  # native Rust port
ts-launcher install --root "C:\Program Files (x86)\Steam\steamapps\common\Half-Life" --half-life
ts-launcher play
ts-launcher print-command
```

## Building

```sh
cargo build --release
```

Linux needs the usual GPUI dependencies (Vulkan, Wayland/X11, xkbcommon, fontconfig). Windows
release builds compile GPUI's DirectX shaders with `fxc.exe` from the Windows SDK, so build them
on Windows (CI does). `TS_INSTALLER=path/to/ts-3-final.exe cargo test` also runs the Clickteam
extractor against the real installer.
