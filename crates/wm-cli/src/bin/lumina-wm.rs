use clap::Parser;
use ipc_server::{default_socket_path, AssetWatcher, IpcServer};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;
use wm_cli::daemon::LuminaDaemon;
use wm_cli::nested::NestedApp;
use wm_proto::ActionCommand;

#[derive(Parser, Debug)]
#[command(
    name = "lumina-wm",
    version,
    about = "Lumina Wayland Window Manager & Pure-Rust HTML/CSS Desktop Shell Compositor"
)]
struct LuminaDaemonArgs {
    /// Unix domain socket path for IPC (`luminactl`)
    #[arg(short, long)]
    socket: Option<PathBuf>,

    /// Path to the HTML/CSS assets directory for live hot-reload
    #[arg(short, long, default_value = "assets")]
    assets: PathBuf,

    /// Theme name (`catppuccin` or `cyberpunk`)
    #[arg(short, long, default_value = "catppuccin")]
    theme: String,

    /// Optional initial actions to dispatch on startup (e.g. `shell:open-widget('launcher')`)
    #[arg(long)]
    dispatch: Vec<String>,

    /// Optional path to save a rendered 1920x1080 desktop screenshot (`.bmp` or `.ppm`)
    #[arg(long)]
    screenshot: Option<PathBuf>,

    /// Run as headless daemon without opening an interactive nested window
    #[arg(long)]
    headless: bool,

    /// Optional number of frames to run before exiting (for headless testing/CI)
    #[arg(long)]
    headless_frames: Option<u64>,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let _ = tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .try_init();

    let args = LuminaDaemonArgs::parse();
    let sock_path = args.socket.clone().unwrap_or_else(default_socket_path);

    // If a screenshot was requested as a single shot (or multi-frame Wayland capture)
    if let Some(ref shot_path) = args.screenshot {
        let mut daemon = LuminaDaemon::new();
        daemon.assets_dir = Some(args.assets.clone());
        let frames = args.headless_frames.unwrap_or(1);
        if frames > 1 {
            daemon.clear_startup_windows();
            let _ = daemon.enable_wayland_server();
        }
        let _ = daemon.set_theme(&args.theme);
        for dsl in &args.dispatch {
            if let Ok(cmds) = ActionCommand::parse_dsl_list(dsl) {
                for cmd in cmds {
                    let _ = daemon.dispatch_action(cmd);
                }
            }
        }
        if frames > 1 {
            for _ in 0..frames {
                let _ = daemon.poll_wayland_clients();
                let _ = daemon.render_desktop_frame();
                thread::sleep(Duration::from_millis(20));
            }
        }
        match daemon.save_desktop_screenshot(shot_path) {
            Ok(fb) => {
                println!(
                    "Saved {}x{} desktop composite screenshot to {:?} ({} rendered pixels)",
                    fb.width,
                    fb.height,
                    shot_path,
                    fb.non_transparent_pixel_count()
                );
            }
            Err(e) => {
                eprintln!("Failed to save screenshot to {:?}: {e}", shot_path);
                std::process::exit(1);
            }
        }
        if args.headless || args.headless_frames.is_some() {
            return Ok(());
        }
    }

    // Interactive nested mode if not explicitly headless and no frame limit was given
    if !args.headless && args.headless_frames.is_none() {
        let app = NestedApp::new(args.socket, args.assets, args.theme, args.dispatch);
        return app.run();
    }

    // Headless loop fallback
    let mut daemon = LuminaDaemon::new();
    daemon.assets_dir = Some(args.assets.clone());
    let _ = daemon.set_theme(&args.theme);
    for dsl in &args.dispatch {
        if let Ok(cmds) = ActionCommand::parse_dsl_list(dsl) {
            for cmd in cmds {
                let _ = daemon.dispatch_action(cmd);
            }
        }
    }

    let mut ipc_server = match IpcServer::bind(&sock_path) {
        Ok(s) => s,
        Err(err) => {
            eprintln!("Failed to bind IPC socket at {:?}: {err}", sock_path);
            std::process::exit(1);
        }
    };
    let mut watcher = AssetWatcher::new(&args.assets);

    tracing::info!(
        "lumina-wm listening on IPC socket {:?} (headless mode)",
        ipc_server.socket_path()
    );

    let mut frame = 0u64;
    loop {
        let _ = ipc_server.poll_once(|req| daemon.handle_ipc_request(req));
        let _reloaded = daemon.poll_hot_reload(&mut watcher);
        let patches = daemon.compositor.tick(&mut daemon.wm);
        daemon.apply_state_patches(patches);

        frame += 1;
        if let Some(limit) = args.headless_frames {
            if frame >= limit {
                break;
            }
        }
        thread::sleep(Duration::from_millis(16));
    }

    Ok(())
}
