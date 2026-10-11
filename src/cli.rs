//! Command-line mode, for scripted installs and for testing without the UI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::config::Config;
use crate::install::{Engine, GameLibs, Plan, PortSource, TsSource, ValveSource};
use crate::progress::{Event, Progress};

pub const USAGE: &str = "\
usage:
  ts-launcher                       open the launcher
  ts-launcher install --root DIR (--half-life | --xash) [options]
      --valve-from HL_DIR           (xash) copy valve from a Half-Life install instead of
                                    downloading it from xash3d-ts
      --ts PATH                     use a local ts-3-final.exe or ts folder instead of downloading
      --port DIR_OR_URL             use the Rust port libraries instead of the TS 3.0 originals
  ts-launcher play                  start the installed game
  ts-launcher print-command         show the command `play` would run";

/// Runs the command line in `args` (without the program name).
pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("install") => install(&args[1..]),
        Some("play") => {
            let config = Config::load();
            let inst = config.installation.as_ref().context("nothing installed yet")?;
            crate::launch::launch(inst, &config.launch)?;
            Ok(())
        }
        Some("print-command") => {
            let config = Config::load();
            let inst = config.installation.as_ref().context("nothing installed yet")?;
            println!("{:?}", crate::launch::command(inst, &config.launch)?);
            Ok(())
        }
        _ => bail!("{USAGE}"),
    }
}

fn install(args: &[String]) -> Result<()> {
    let mut root = None;
    let mut engine = None;
    let mut valve = ValveSource::XashTs;
    let mut ts = TsSource::Download;
    let mut libs = GameLibs::Original;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--half-life" => engine = Some(false),
            "--xash" => engine = Some(true),
            "--valve-from" => valve = ValveSource::CopyFrom(value()?.into()),
            "--ts" => ts = TsSource::Local(value()?.into()),
            "--port" => {
                let v = value()?;
                libs = GameLibs::RustPort(if v.contains("://") { PortSource::Url(v) } else { PortSource::Dir(v.into()) });
            }
            _ => bail!("unknown option {arg}\n{USAGE}"),
        }
    }
    let engine = match engine.context("choose --half-life or --xash")? {
        true => Engine::Xash { valve },
        false => Engine::HalfLife,
    };
    let plan = Plan { root: root.context("--root is required")?, engine, ts, libs };

    let last = std::sync::Mutex::new((0u64, std::time::Instant::now()));
    let progress = Progress::new(move |e| match e {
        Event::Step { index, total, title } => eprintln!("[{}/{total}] {title}", index + 1),
        Event::Log(m) => eprintln!("    {m}"),
        Event::Bytes { done, total } => {
            let mut last = last.lock().unwrap();
            if last.1.elapsed().as_millis() >= 1000 || Some(done) == total {
                *last = (done, std::time::Instant::now());
                match total {
                    Some(t) if t > 0 => eprintln!("    {:.0}% ({} / {} MB)", done as f64 * 100.0 / t as f64, done >> 20, t >> 20),
                    _ => eprintln!("    {} MB", done >> 20),
                }
            }
        }
    });
    let installation = plan.run(&progress)?;
    let mut config = Config::load();
    config.installation = Some(installation);
    config.save()?;
    eprintln!("installed; saved {}", Config::path().display());
    Ok(())
}
