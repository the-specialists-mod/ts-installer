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
| [Rust port](https://github.com/the-specialists-mod/the-specialistsmod-rs) | native 64-bit or 32-bit | native | native |

With the Rust port you pick the 64-bit (amd64) or 32-bit (i386) Xash3D FWGS build, which loads
the port's `x86_64` or `i686` libraries (`--32bit` on the command line). The 32-bit Linux build
needs the system's 32-bit libraries (lib32 / i386 packages).

The Rust port is downloaded from the latest
[the-specialistsmod-rs release](https://github.com/the-specialists-mod/the-specialistsmod-rs/releases),
using the archive for your platform. You can instead point the launcher at its `dist/` folder
(from `cargo xtask dist`), one platform's `ts/` folder, or a `.zip` / `.tar.gz` URL with the same layout. With a Half-Life
install, the `*-i686` libraries are used. On Linux, a Half-Life install with the original DLLs is
launched through Steam, which needs Proton for it.

An installation's overview can switch its game libraries later between the TS 3.0 originals and
the Rust port's i686 and amd64 builds, each as a release or a debug build (the `-debug` archives,
with debug info and assertions). Xash3D installs get the engine build the libraries
need (32-bit Windows for the originals, native 32-bit for i686, native 64-bit for amd64); Half-Life installs only
load 32-bit libraries, so they don't offer amd64. The TS 3.0 files the port replaces are kept in
`ts/.tsinstaller-originals` and put back when switching to the originals.

After installing, the launcher shows a play screen with launch options (windowed, console, extra
arguments, and Wine command / `WINEPREFIX` where relevant). "Start a local server" makes Play host
a listen server on a chosen map with a number of bots; the game mode, limits and bot skill come
from "Start a server" in the server browser. Settings are saved in
`<config dir>/ts-launcher/config.json`; downloads are cached in `<cache dir>/ts-launcher/` and
interrupted downloads resume.

On start the launcher checks this repository's latest release. When it is newer, a banner asks
whether to download it; the new build replaces the running one and "Restart now" starts it. Set
`TSINSTALLER_NO_UPDATE_CHECK=1` to skip the check.

### Player models

The Player models tab lists `ts/models/player/<name>/` with each model's preview picture and
readme, and draws the selected `.mdl` in 3D (drag to turn; `src/mdl.rs` parses studio model v10
and renders it in software). "Use" sets the engine's `model` cvar in `ts/config.cfg`, which is
how TS picks your model. Team play servers (team deathmatch, The One) override it with the team's
models. "Add model…" copies a folder with a `.mdl` (and its `.bmp` / `.txt`) into
`models/player/<mdl name>/`.

### Controls

The Controls tab edits the `bind` lines of `ts/config.cfg` without starting the game. Actions and
their sections come from `ts/gfx/shell/kb_act.lst` (Half-Life's if TS has none). Click a key slot
and press a key, mouse button or wheel. Esc cancels and Backspace/Delete clears the slot. A key
already bound elsewhere moves to the new action. Changes are staged until Save, which keeps
`config.cfg.bak`. Save is disabled while the game runs, because the game rewrites `config.cfg`
when it quits. "Reset to defaults" uses `ts/gfx/shell/kb_def.lst` if present. Otherwise it uses
the binds of the `config.cfg` that TS 3.0 ships, since TS has no `kb_def.lst`. GPUI doesn't tell
keypad digits and arrows apart from the main keys, so those can't be captured as `kp_*` keys.

### Game configuration

The **Game configuration** tab changes TS settings without starting the game: player name, mouse,
video, audio, HUD and TS client options, and network rates, plus a filterable table of every cvar
in `ts/config.cfg` and an editor for `ts/userconfig.cfg` (both engines run it after
`config.cfg`, so it is the place for custom commands). It only offers what the installation's
engine supports. On Xash3D FWGS the resolution and display mode go in `ts/video.cfg`; Steam
Half-Life keeps those outside `config.cfg` (`hl.conf` or the registry), so they are left to the
game. Changes are written on **Save**, each file keeping a `.bak`, and saving is off while the game
runs because it rewrites `config.cfg` when it quits. If `config.cfg` doesn't exist yet, the new one
starts as a copy of the file the game would otherwise load, so key bindings stay.

### Server browser and your own server

The old GoldSrc/Xash master servers are gone, so the server browser combines
[`servers.txt`](servers.txt) in this repository (downloaded at run time, so adding a line here
lists a server for everyone; open a pull request), Xash3D FWGS's HTTP list, a LAN broadcast, your
favorites and, with a Steam Web API key, Steam's master server. Every server is asked directly
(A2S_INFO / A2S_PLAYER) for its name, map, players and ping.

**Start a server** hosts a game on this computer: a listen server (the game hosts the map and you
play on it) or, with the Linux FWGS build, a dedicated server. The launcher writes
`ts/tslauncher_server.cfg` (and `ts/tslauncher_bots.cfg`), which TS execs once the map is
running, so your `server.cfg` and `listenserver.cfg` are left alone. Bots (TS 3.0's
`addcustombot`, or the Rust port's `ts_bot_quota` on a dedicated server) take their names from
`ts/bots.txt`, which the form can edit (the old file is kept as `bots.txt.bak`).

## Command line

The same installer runs without the UI:

```sh
ts-launcher install --root ~/games/ts --xash                      # everything downloaded
ts-launcher install --root ~/games/ts --xash \
    --valve-from ~/.local/share/Steam/steamapps/common/Half-Life \
    --ts ~/Downloads/ts-3-final.exe \
    --port                                                        # native Rust port, latest release
ts-launcher install --root ~/games/ts-debug --xash --port-debug   # the Rust port's debug build
ts-launcher install --root ~/games/ts-dev --xash \
    --port ~/src/the-specialistsmod-rs/dist                       # Rust port from a local build
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
