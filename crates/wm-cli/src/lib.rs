pub mod cli;
pub mod daemon;
pub mod nested;
pub mod output_config;
pub mod wayland_compositor;

pub use cli::*;
pub use nested::NestedApp;
pub use wayland_compositor::{RealWaylandServer, WaylandCompositorEvent};
pub use css_style;
pub use daemon::LuminaDaemon;
pub use html_dom;
pub use ipc_server;
pub use layout_engine;
pub use reactive_state;
pub use renderer;
pub use sys_providers;
pub use wm_core;
pub use wm_proto;

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    fn exec_ctl(sock: &Path, args: &[&str]) -> String {
        let mut full_args = vec![
            "luminactl".to_string(),
            "--socket".to_string(),
            sock.to_string_lossy().to_string(),
        ];
        full_args.extend(args.iter().map(|s| s.to_string()));
        let cli = LuminaCtlCli::try_parse_from(full_args).expect("CLI args should parse");
        run_cli(&cli).expect("CLI execution should succeed")
    }

    #[test]
    fn test_luminactl_full_e2e_against_daemon() {
        let sock_path = std::env::temp_dir().join(format!(
            "lumina-cli-e2e-{}.sock",
            std::process::id()
        ));
        let daemon = Arc::new(Mutex::new(LuminaDaemon::new()));
        let mut server =
            LuminaDaemon::spawn_ipc_server(Arc::clone(&daemon), &sock_path).unwrap();

        // 1. Ping (human and JSON)
        let pong = exec_ctl(&sock_path, &["ping"]);
        assert!(pong.contains("PONG"));
        let pong_json = exec_ctl(&sock_path, &["--json", "ping"]);
        assert!(pong_json.contains("\"pong\""));

        // 2. WM queries: workspaces, windows, outputs
        let ws_out = exec_ctl(&sock_path, &["wm", "workspaces"]);
        assert!(ws_out.contains("tiling"));
        let win_out = exec_ctl(&sock_path, &["wm", "windows"]);
        assert!(win_out.contains("org.mozilla.firefox"));
        let out_str = exec_ctl(&sock_path, &["wm", "outputs"]);
        assert!(out_str.contains("eDP-1"));

        // 3. Dispatch DSL actions & verify live DOM reactive data-for updates
        exec_ctl(&sock_path, &["dispatch", "wm:focus-workspace(2)"]);
        let active_ws = exec_ctl(&sock_path, &["--json", "state", "get", "wm.active_workspace"]);
        assert!(active_ws.contains("2"));

        // Query `.ws-btn.active` in the live DOM (populated via `data-for="ws in wm.workspaces"`)
        let active_ws_btn = exec_ctl(
            &sock_path,
            &["dom", "query", ".ws-btn.active", "--surface", "topbar"],
        );
        assert!(active_ws_btn.contains("ws-btn"));
        assert!(active_ws_btn.contains("\"2\"") || active_ws_btn.contains("active"));

        exec_ctl(&sock_path, &["dispatch", "wm:set-layout-mode(scrolling)"]);
        let mode_json = exec_ctl(&sock_path, &["--json", "state", "get", "wm.layout_mode"]);
        assert!(mode_json.contains("scrolling"));

        // 4. DOM query, tree, style on real parsed HTML + CSS + Taffy layout
        let dom_query = exec_ctl(
            &sock_path,
            &["dom", "query", "#topbar", "--surface", "panel"],
        );
        assert!(dom_query.contains("#topbar"));

        let dom_tree = exec_ctl(
            &sock_path,
            &["dom", "tree", "--surface", "panel", "--depth", "2"],
        );
        assert!(dom_tree.contains("#topbar"));

        let dom_style = exec_ctl(
            &sock_path,
            &["dom", "style", "#topbar", "--surface", "panel"],
        );
        assert!(dom_style.contains("--accent"));

        // 5. CSS set-var and apply
        exec_ctl(
            &sock_path,
            &["css", "set-var", "--wm-gap-inner", "18px", "--surface", "panel"],
        );
        assert_eq!(daemon.lock().unwrap().wm.theme_config().inner_gap, 18.0);

        let css_apply = exec_ctl(
            &sock_path,
            &["css", "apply", ".topbar { opacity: 0.95; }", "--surface", "panel"],
        );
        assert!(css_apply.contains("Applied live CSS"));

        // 6. Reload
        let reload_out = exec_ctl(&sock_path, &["reload", "css", "--surface", "panel"]);
        assert!(reload_out.contains("Reloaded Css"));

        // 7. Widget list / open / toggle / close
        let w_list = exec_ctl(&sock_path, &["widget", "list"]);
        assert!(w_list.contains("launcher"));

        exec_ctl(&sock_path, &["widget", "open", "launcher"]);
        assert!(daemon.lock().unwrap().widgets["launcher"].visible);

        exec_ctl(&sock_path, &["widget", "toggle", "launcher"]);
        assert!(!daemon.lock().unwrap().widgets["launcher"].visible);

        exec_ctl(&sock_path, &["widget", "open", "control-center"]);
        exec_ctl(&sock_path, &["widget", "close", "control-center"]);
        assert!(!daemon.lock().unwrap().widgets["control-center"].visible);

        // 8. State set & get
        exec_ctl(&sock_path, &["state", "set", "audio.volume_pct", "88"]);
        let vol_out = exec_ctl(&sock_path, &["state", "get", "audio.volume_pct"]);
        assert_eq!(vol_out.trim(), "88");

        // 9. Verify full 1920x1080 desktop composite rendering
        {
            let mut guard = daemon.lock().unwrap();
            let frame = guard.render_desktop_frame();
            assert_eq!(frame.width, 1920);
            assert_eq!(frame.height, 1080);
            assert!(frame.non_transparent_pixel_count() > 1_000_000);
        }

        server.shutdown();
    }
}
