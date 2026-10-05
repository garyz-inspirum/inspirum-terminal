use anyhow::{Context, Result, bail};
use inspirum_terminal::app::App;
use std::path::PathBuf;
fn main() -> Result<()> {
    let mut config = None;
    let mut profiles = None;
    let mut args = std::env::args_os().skip(1);
    while let Some(arg) = args.next() {
        match arg.to_str() {
            Some("--help" | "-h") => {
                println!("Inspirum Terminal {}\nUsage: inspirum-terminal [--profiles PATH] [--ssh-config PATH]\nNative SSH terminal. Requires system OpenSSH and a graphical desktop.", env!("CARGO_PKG_VERSION"));
                return Ok(());
            }
            Some("--version" | "-V") => { println!("inspirum-terminal {}", env!("CARGO_PKG_VERSION")); return Ok(()); }
            Some("--profiles") => profiles = Some(PathBuf::from(args.next().context("--profiles requires a path")?)),
            Some("--ssh-config") => config = Some(PathBuf::from(args.next().context("--ssh-config requires a path")?)),
            _ => bail!("unknown argument {arg:?}; use --help"),
        }
    }
    let path = match profiles {
        Some(path) => path,
        None => directories::ProjectDirs::from("com", "Inspirum", "Inspirum Terminal")
            .context("cannot locate configuration directory; pass --profiles PATH")?.config_dir().join("sessions.json"),
    };
    // SAFETY: main has not spawned threads or initialized the GUI. All later child
    // terminals inherit a widely available terminfo name; no concurrent env access.
    unsafe { std::env::set_var("TERM", "xterm-256color"); }
    eframe::run_native("Inspirum Terminal", eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_inner_size([1100.0, 720.0]),
        ..Default::default()
    }, Box::new(move |_cc| Ok(Box::new(App::new(path, config)))))
    .map_err(|error| anyhow::anyhow!("native window initialization failed: {error}"))
}
