mod diagnostics;

use anyhow::{Context, Result, bail, ensure};
use inspirum_terminal::{ProxyKind, app::App};
use std::{ffi::OsString, path::PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum UiMode {
    #[default]
    Legacy,
    Iced,
}

fn default_profiles() -> Result<PathBuf> {
    Ok(
        directories::ProjectDirs::from("com", "Inspirum", "Inspirum Terminal")
            .context("cannot locate configuration directory; pass --profiles PATH")?
            .config_dir()
            .join("sessions.json"),
    )
}

fn parse_proxy_helper(args: &[OsString]) -> Result<()> {
    let mut mode = None;
    let mut proxy_host = None;
    let mut proxy_port = None;
    let mut target_host = None;
    let mut target_port = None;
    let mut index = 0;
    while index < args.len() {
        let name = args[index]
            .to_str()
            .context("proxy helper arguments must be valid Unicode")?;
        index += 1;
        let value = args
            .get(index)
            .context("proxy helper option requires a value")?
            .to_str()
            .context("proxy helper values must be valid Unicode")?;
        index += 1;
        match name {
            "--mode" => {
                mode = Some(match value {
                    "http-connect" => ProxyKind::HttpConnect,
                    "socks5" => ProxyKind::Socks5,
                    _ => bail!("unsupported proxy helper mode"),
                })
            }
            "--proxy-host" => proxy_host = Some(value.to_owned()),
            "--proxy-port" => {
                proxy_port = Some(value.parse::<u16>().context("invalid proxy port")?)
            }
            "--target-host" => target_host = Some(value.to_owned()),
            "--target-port" => {
                target_port = Some(value.parse::<u16>().context("invalid target port")?)
            }
            _ => bail!("unknown proxy helper option"),
        }
    }
    inspirum_terminal::proxy::run_stdio(
        mode.context("proxy helper mode is required")?,
        proxy_host.as_deref().context("proxy host is required")?,
        proxy_port.context("proxy port is required")?,
        target_host.as_deref().context("target host is required")?,
        target_port.context("target port is required")?,
    )
}

fn main() -> Result<()> {
    let raw_args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if raw_args.first().and_then(|arg| arg.to_str()) == Some("--proxy-helper") {
        return parse_proxy_helper(&raw_args[1..]);
    }

    let mut config = None;
    let mut profiles = None;
    let mut diagnostics = false;
    let mut diagnostics_output = None;
    let mut diagnostic_profile = None;
    let mut ui_mode = if cfg!(feature = "iced-ui") {
        UiMode::Iced
    } else {
        UiMode::Legacy
    };
    let mut args = raw_args.into_iter();
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!(
                    "Inspirum Terminal {}\n\
                     Usage: inspirum-terminal [--profiles PATH] [--ssh-config PATH] [--ui legacy|iced]\n\
                     Diagnostics: --diagnostics [--diagnostic-profile NAME] [--diagnostics-output PATH]\n\
                     Native SSH terminal. Requires system OpenSSH and a graphical desktop.\n\
                     The Iced frontend requires a build with --features iced-ui.\n\
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
            Some("--ui") => {
                let value = args
                    .next()
                    .context("--ui requires legacy or iced")?
                    .into_string()
                    .map_err(|_| anyhow::anyhow!("--ui value must be valid Unicode"))?;
                ui_mode = match value.as_str() {
                    "legacy" => UiMode::Legacy,
                    "iced" => UiMode::Iced,
                    _ => bail!("--ui must be legacy or iced"),
                };
            }
            Some("--diagnostics") => diagnostics = true,
            Some("--diagnostics-output") => {
                diagnostics_output = Some(PathBuf::from(
                    args.next()
                        .context("--diagnostics-output requires a path")?,
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
    if ui_mode == UiMode::Iced {
        #[cfg(feature = "iced-ui")]
        {
            inspirum_terminal::iced_app::run(path.clone(), config.clone()).map_err(|error| {
                anyhow::anyhow!("native Iced window initialization failed: {error}")
            })?;
            return Ok(());
        }
        #[cfg(not(feature = "iced-ui"))]
        {
            bail!("this build does not include the Iced frontend; rebuild with --features iced-ui");
        }
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
