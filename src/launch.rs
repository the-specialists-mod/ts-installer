//! Starting the installed game.

use std::path::Path;
use std::process::{Child, Command};

use anyhow::{Context, Result, bail};

use crate::install::{Installation, Runtime};
use crate::store::LaunchOptions;

/// The command that starts TS for `install` with `opts`, followed by `extra` arguments (e.g.
/// `+connect host:port`).
pub fn command(install: &Installation, opts: &LaunchOptions, extra: &[String]) -> Result<Command> {
    let root = &install.root;
    let mut args: Vec<String> = vec!["-game".into(), "ts".into()];
    if opts.windowed {
        args.push("-windowed".into());
    }
    if opts.console {
        args.push("-console".into());
    }
    args.extend(opts.extra_args.split_whitespace().map(str::to_owned));
    args.extend(extra.iter().cloned());

    let mut cmd = match install.runtime {
        Runtime::Xash(build) => {
            let exe = root.join(build.executable());
            ensure_exists(&exe)?;
            if build.needs_wine() {
                let mut c = Command::new(if opts.wine.trim().is_empty() { "wine" } else { opts.wine.trim() });
                c.arg(&exe);
                if !opts.wine_prefix.trim().is_empty() {
                    c.env("WINEPREFIX", opts.wine_prefix.trim());
                }
                c
            } else {
                Command::new(exe)
            }
        }
        Runtime::GoldSrc if cfg!(windows) => {
            let exe = root.join("hl.exe");
            ensure_exists(&exe)?;
            Command::new(exe)
        }
        // Steam sets up its runtime (or Proton) for Half-Life on Linux.
        Runtime::GoldSrc => {
            let mut c = Command::new("steam");
            c.args(["-applaunch", "70"]);
            c
        }
    };
    cmd.args(args).current_dir(root);
    if let Runtime::Xash(_) = install.runtime {
        // Xash3D FWGS takes its base directory from the working directory unless told otherwise.
        cmd.env("XASH3D_BASEDIR", root);
    }
    Ok(cmd)
}

fn ensure_exists(exe: &Path) -> Result<()> {
    if !exe.is_file() {
        bail!("{} is missing; reinstall", exe.display());
    }
    Ok(())
}

pub fn launch(install: &Installation, opts: &LaunchOptions, extra: &[String]) -> Result<Child> {
    let mut cmd = command(install, opts, extra)?;
    log::info!("launching {cmd:?}");
    cmd.spawn().with_context(|| format!("start {:?}", cmd.get_program()))
}

/// `cmd` as one line for display.
pub fn display(cmd: &Command) -> String {
    std::iter::once(cmd.get_program()).chain(cmd.get_args()).map(|a| a.to_string_lossy().into_owned()).collect::<Vec<_>>().join(" ")
}
