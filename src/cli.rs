//! Command-line mode, for scripted installs and for testing without the UI.

use std::path::PathBuf;

use anyhow::{Context, Result, bail};

use crate::install::{Engine, EngineSource, Existing, GameLibs, Plan, PortSource, TsSource, ValveSource};
use crate::progress::{Event, Progress};
use crate::sources::Arch;
use crate::store::Store;

pub const USAGE: &str = "\
usage:
  ts-launcher                       open the launcher
  ts-launcher install --root DIR (--half-life | --xash) [options]
      --valve-from HL_DIR           (xash) copy valve from a Half-Life install instead of
                                    downloading it from xash3d-ts
      --ts PATH                     use a local ts-3-final.exe or ts folder instead of downloading
      --port [DIR_OR_URL]           use the Rust port libraries instead of the TS 3.0 originals:
                                    its latest release unless a dist/ folder or archive URL is given
      --32bit                       (xash) with the Rust port, use the 32-bit engine and the port's
                                    i686 libraries instead of 64-bit
      --port-version TAG            install that release of the Rust port instead of the latest
      --port-debug                  use the debug build of that Rust port release
      --keep-engine | --keep-valve | --keep-ts
                                    keep what the folder already has instead of installing it
  ts-launcher list                  list the registered installations
  ts-launcher play [ID]             start an installation (default: the last one used)
  ts-launcher print-command [ID]    show the command `play` would run
  ts-launcher validate [ID]         compare the SHA-256 of the TS 3.0 files with ts-3-final.exe,
                                    and of the Rust port files with the release they came from
  ts-launcher repair [ID]           extract damaged or missing TS 3.0 files again (keeps settings)
  ts-launcher reset [ID] --yes      back to a clean TS 3.0: original libraries and every release
                                    file, settings included (custom maps and models stay)";

/// Runs the command line in `args` (without the program name).
pub fn run(args: &[String]) -> Result<()> {
    match args.first().map(String::as_str) {
        Some("install") => install(&args[1..]),
        Some("list") => {
            let store = Store::load();
            for i in &store.instances {
                let mark = if store.last_used.as_deref() == Some(i.id.as_str()) { "*" } else { " " };
                println!("{mark} {}  {}  {}  {}", i.id, i.name, i.install.runtime.label(), i.install.root.display());
            }
            Ok(())
        }
        Some(cmd @ ("validate" | "repair" | "reset")) => {
            let mut store = Store::load();
            let rest: Vec<&String> = args[1..].iter().filter(|a| *a != "--yes").collect();
            let id = rest.first().map(|s| s.to_string()).or(store.last_used.clone()).context("nothing installed yet")?;
            let inst = store.get(&id).with_context(|| format!("no installation {id}"))?.clone();
            let progress = Progress::new(|e| match e {
                Event::Step { title, .. } => eprintln!("{title}"),
                Event::Log(m) => eprintln!("    {m}"),
                Event::Bytes { .. } => {}
            });
            let root = &inst.install.root;
            match cmd {
                "reset" => {
                    if !args.iter().any(|a| a == "--yes") {
                        bail!("reset puts the TS 3.0 libraries and every release file back, settings included; run it again with --yes");
                    }
                    let install = crate::tsfiles::reset(&inst.install, &progress)?;
                    println!("reset {} to TS 3.0 ({})", root.display(), install.runtime.label());
                    if let Some(i) = store.get_mut(&id) {
                        i.install = install;
                    }
                    store.save()?;
                    Ok(())
                }
                "repair" => {
                    let check = crate::tsfiles::repair(root, &progress)?;
                    match check.broken().len() {
                        0 => println!("ok: nothing to repair ({} TS 3.0 files match)", check.matching),
                        n => println!("repaired {n} TS 3.0 files: {}", check.broken().join(", ")),
                    }
                    Ok(())
                }
                _ => {
                    let check = crate::tsfiles::validate(root, &progress)?;
                    let mut problems = Vec::new();
                    if check.ok() {
                        println!("TS 3.0: all {} files match the release", check.matching);
                    } else {
                        problems.push(format!("TS 3.0: damaged or missing ({}): {}", check.broken().len(), check.broken().join(", ")));
                    }
                    if !check.settings_changed.is_empty() {
                        println!("TS 3.0: settings changed from the defaults: {}", check.settings_changed.join(", "));
                    }
                    if crate::install::port_manifest(&root.join("ts")).is_some() {
                        let port = crate::install::validate_port(root, &progress)?;
                        let m = &port.manifest;
                        if port.ok() {
                            println!("Rust port: all {} files match {} ({})", port.matching, m.tag, m.asset);
                        } else {
                            let bad: Vec<String> = port.different.iter().chain(&port.missing).cloned().collect();
                            problems.push(format!("Rust port {}: damaged or missing ({}): {}", m.tag, bad.len(), bad.join(", ")));
                        }
                    }
                    if problems.is_empty() { Ok(()) } else { bail!("{}", problems.join("\n")) }
                }
            }
        }
        Some(cmd @ ("play" | "print-command")) => {
            let mut store = Store::load();
            let id = args.get(1).cloned().or(store.last_used.clone()).context("nothing installed yet")?;
            let inst = store.get(&id).with_context(|| format!("no installation {id}"))?.clone();
            if cmd == "play" {
                crate::launch::play(&inst.install, &inst.launch)?;
                if let Some(i) = store.get_mut(&id) {
                    i.last_played = Some(crate::store::now());
                }
                store.last_used = Some(id);
                store.save()?;
            } else {
                let args = crate::launch::play_args(&inst.install, &inst.launch);
                println!("{}", crate::launch::display(&crate::launch::command(&inst.install, &inst.launch, &args)?));
            }
            Ok(())
        }
        // Not in USAGE: the server browser runs it to ask the Steam client (see steam.rs).
        #[cfg(steam_client)]
        Some(crate::steam::COMMAND) => crate::steam::print_server_list(),
        // Not in USAGE either: Play runs it next to Xash3D FWGS (see steam_broker.rs).
        #[cfg(steam_client)]
        Some(crate::steam_broker::COMMAND) => crate::steam_broker::run(),
        _ => bail!("{USAGE}"),
    }
}

fn install(args: &[String]) -> Result<()> {
    let mut root = None;
    let mut engine = None;
    let mut valve = ValveSource::XashTs;
    let mut keep_engine = false;
    let mut ts = TsSource::Download;
    let mut libs = GameLibs::Original;
    let mut arch = Arch::Amd64;
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        let mut value = || it.next().cloned().with_context(|| format!("{arg} needs a value"));
        match arg.as_str() {
            "--root" => root = Some(PathBuf::from(value()?)),
            "--half-life" => engine = Some(false),
            "--xash" => engine = Some(true),
            "--valve-from" => valve = ValveSource::CopyFrom(value()?.into()),
            "--ts" => ts = TsSource::Local(value()?.into()),
            "--keep-ts" => ts = TsSource::Keep,
            "--keep-valve" => valve = ValveSource::Keep,
            "--keep-engine" => keep_engine = true,
            "--port" => {
                let v = it.as_slice().first().filter(|v| !v.starts_with("--")).cloned();
                libs = GameLibs::RustPort(match v {
                    None => PortSource::Release { tag: None, debug: false },
                    Some(v) => {
                        it.next();
                        if v.contains("://") { PortSource::Url(v) } else { PortSource::Dir(v.into()) }
                    }
                });
            }
            "--32bit" => arch = Arch::I386,
            "--port-debug" => {
                libs = match libs {
                    GameLibs::RustPort(PortSource::Release { tag, .. }) => GameLibs::RustPort(PortSource::Release { tag, debug: true }),
                    _ => GameLibs::RustPort(PortSource::Release { tag: None, debug: true }),
                };
            }
            "--port-version" => {
                let tag = value()?;
                libs = match libs {
                    GameLibs::RustPort(PortSource::Release { debug, .. }) => {
                        GameLibs::RustPort(PortSource::Release { tag: Some(tag), debug })
                    }
                    _ => GameLibs::RustPort(PortSource::Release { tag: Some(tag), debug: false }),
                };
            }
            _ => bail!("unknown option {arg}\n{USAGE}"),
        }
    }
    let root: PathBuf = root.context("--root is required")?;
    let engine = match engine.context("choose --half-life or --xash")? {
        true => {
            let engine = match (keep_engine, Existing::inspect(&root).xash) {
                (true, Some(build)) => EngineSource::Keep(build),
                (true, None) => bail!("{} has no Xash3D FWGS engine to keep", root.display()),
                (false, _) => EngineSource::Download,
            };
            Engine::Xash { engine, valve, arch }
        }
        false => Engine::HalfLife,
    };
    let plan = Plan { root, engine, ts, libs };

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
    let mut store = Store::load();
    let id = store.add(installation);
    store.save()?;
    eprintln!("installed as {id}; saved {}", Store::path().display());
    Ok(())
}
