mod diagnostics;

use anyhow::{Context, Result, bail, ensure};
use inspirum_terminal::app::App;
use std::path::PathBuf;

fn default_profiles() -> Result<PathBuf> {
    Ok(directories::ProjectDirs::from("com", "Inspirum", "Inspirum Terminal")
        .context("cannot locate configuration directory; pass --profiles PATH")?
        .config_dir()
        .join("sessions.json"))
}

fn main() -> Result<()> {
    let mut config = None;
    let mut profiles = None;
    let mut diagnostics = false;
    let mut diagnostics_output = None;
    let mut diagnostic_profile = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "Inspirum Terminal {}\n\
                     Usage: inspirum-terminal [--profiles PATH] [--ssh-config PATH]\n\
                     Diagnostics: --diagnostics [--diagnostic-profile NAME] [--diagnostics-output PATH]\n\
                     Native SSH terminal. Requires system OpenSSH and a graphical desktop.\n\
                     Diagnostics require no desktop and never connect to a server.",
                    env!("CARGO_PKG_VERSION")
                );
                return Ok(());
            }
            Some("--version" | "-V") => {
                println!("inspirum-terminal {}", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--profiles") => {
                profiles = Some(PathBuf::from(
                    args.next().context("--profiles requires a path")?,
                ));
            }
            Some("--ssh-config") => {
                config = Some(PathBuf::from(
                    args.next().context("--ssh-config requires a path")?,
                ));
            }
            Some("--diagnostics") => diagnostics = true,
            Some("--diagnostics-output") => {
                diagnostics_output = Some(PathBuf::from(
                    args.next().context("--diagnostics-output requires a path")?,
                ));
            }
            Some("--diagnostic-profile") => {
                diagnostic_profile = Some(
                    args.next()
                        .context("--diagnostic-profile requires a saved profile name")?
                        .into_string()
                        .map_err(|_| anyhow::anyhow!("profile name must be valid Unicode"))?,
                );
            }
            _ => bail!("unknown argument {arg:?}; use --help"),
        }
    }
    ensure!(
        diagnostics || (diagnostics_output.is_none() && diagnostic_profile.is_none()),
        "--diagnostics-output and --diagnostic-profile require --diagnostics"
    );
    if diagnostics {
        // This path exits before environment mutation and graphical initialization.
        if diagnostic_profile.is_some() && profiles.is_none() {
            profiles = Some(default_profiles()?);
        }
        let report = diagnostics::collect(
            profiles.as_deref(),
            diagnostic_profile.as_deref(),
            config.is_some(),
        );
        if let Some(path) = diagnostics_output {
            diagnostics::export(&path, &report)?;
            println!("Support report saved.");
        } else {
            print!("{report}");
        }
        return Ok(());
    }
    let path = match profiles {
        Some(path) => path,
        None => default_profiles()?,
    };
    // SAFETY: main has not spawned threads or initialized the GUI. All later child
    // terminals inherit a widely available terminfo name; no concurrent env access.
    unsafe {
        std::env::set_var("TERM", "xterm-256color");
    }
    eframe::run_native(
        "Inspirum Terminal",
        eframe::NativeOptions {
            viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1100.0, 720.0]),
            ..Default::default()
        },
        Box::new(move |_cc| Ok(Box::new(App::new(path, config)))),
    )
    .map_err(|error| anyhow::anyhow!("native window initialization failed: {error}"))
}
