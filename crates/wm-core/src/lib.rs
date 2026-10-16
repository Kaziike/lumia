pub mod config;
pub mod engine;
pub mod layout;
pub mod wayland_host;

pub use config::{WmConfig, WmThemeConfig};
pub use engine::{InteractiveGrab, LuminaWmCore};
pub use layout::{
    available_rect, build_window_placement, compute_bsp_rects, compute_monocle_rects,
    compute_scrolling_rects, compute_tiling_rects, compute_workspace_placements,
    default_floating_rect,
};
pub use wayland_host::{
    AnchorEdges, HitTarget, HostedLayerSurface, KeyboardInteractivity, LayerTier,
    WaylandCompositorHost, XdgToplevelConfigure,
};
pub use wm_proto;

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;
    use wm_proto::{
        Direction, LayoutMode, Point, Rect, ResizeEdge, StatePatch, StateValue, SurfaceMetadata,
        WindowDescriptor, WindowManagerEngine, WorkspaceTarget, WmAction,
    };

    fn make_win(id: u64, title: &str, app_id: &str) -> WindowDescriptor {
        WindowDescriptor {
            id,
            title: title.to_string(),
            app_id: app_id.to_string(),
            icon: app_id.to_string(),
            workspace_id: 1,
            is_focused: false,
            is_floating: false,
            is_fullscreen: false,
            is_maximized: false,
            is_minimized: false,
            is_urgent: false,
            rect: Rect::default(),
        }
    }

    #[test]
    fn test_all_five_layout_modes() {
        let mut wm = LuminaWmCore::new();
        let usable = wm.primary_usable_rect();
        assert_eq!(usable, Rect::new(0.0, 44.0, 1920.0, 1036.0));

        wm.on_window_opened(make_win(1, "Alacritty 1", "org.alacritty"));
        wm.on_window_opened(make_win(2, "Firefox", "org.mozilla.firefox"));
        wm.on_window_opened(make_win(3, "Neovim", "nvim"));

        // 1. Tiling (Master-Stack)
        assert_eq!(wm.active_layout_mode(), LayoutMode::Tiling);
        let tiling_placements = wm.compute_window_placements(usable);
        assert_eq!(tiling_placements.len(), 3);
        let master = &tiling_placements[0];
        let stack1 = &tiling_placements[1];
        let stack2 = &tiling_placements[2];
        assert!(master.frame_rect.x < stack1.frame_rect.x);
        assert_eq!(stack1.frame_rect.x, stack2.frame_rect.x);
        assert!(stack1.frame_rect.y < stack2.frame_rect.y);
        assert!(master.titlebar_rect.is_some());
        assert!(master.content_rect.height < master.frame_rect.height);

        // 2. Scrolling (Niri/PaperWM horizontal strip)
        wm.dispatch_wm_action(&WmAction::SetLayoutMode {
            mode: LayoutMode::Scrolling,
        })
        .unwrap();
        let scrolling_placements = wm.compute_window_placements(usable);
        assert_eq!(scrolling_placements.len(), 3);
        assert!(scrolling_placements[0].frame_rect.x < scrolling_placements[1].frame_rect.x);
        assert!(scrolling_placements[1].frame_rect.x < scrolling_placements[2].frame_rect.x);
        assert_eq!(
            scrolling_placements[0].frame_rect.height,
            scrolling_placements[1].frame_rect.height
        );

        // Focusing window 1 vs window 3 scrolls the strip
        wm.dispatch_wm_action(&WmAction::FocusWindow { id: 1 }).unwrap();
        let scroll_first = wm.compute_window_placements(usable);
        wm.dispatch_wm_action(&WmAction::FocusWindow { id: 3 }).unwrap();
        let scroll_last = wm.compute_window_placements(usable);
        assert!(scroll_first[0].frame_rect.x > scroll_last[0].frame_rect.x);

        // 3. BSP (Binary Space Partitioning / Dwindle)
        wm.dispatch_wm_action(&WmAction::SetLayoutMode {
            mode: LayoutMode::Bsp,
        })
        .unwrap();
        let bsp_placements = wm.compute_window_placements(usable);
        assert_eq!(bsp_placements.len(), 3);
        // First split is vertical (win 1 on left, win 2 & 3 on right), second split is horizontal (win 2 top-right, win 3 bottom-right)
        assert!(bsp_placements[0].frame_rect.x < bsp_placements[1].frame_rect.x);
        assert_eq!(bsp_placements[1].frame_rect.x, bsp_placements[2].frame_rect.x);
        assert!(bsp_placements[1].frame_rect.y < bsp_placements[2].frame_rect.y);

        // 4. Monocle (Maximized deck)
        wm.dispatch_wm_action(&WmAction::SetLayoutMode {
            mode: LayoutMode::Monocle,
        })
        .unwrap();
        let monocle_placements = wm.compute_window_placements(usable);
        assert_eq!(monocle_placements.len(), 3);
        assert_eq!(
            monocle_placements[0].frame_rect,
            monocle_placements[1].frame_rect
        );
        assert_eq!(
            monocle_placements[1].frame_rect,
            monocle_placements[2].frame_rect
        );
        // Focused window (id 3) has higher z_order than unfocused windows
        assert!(monocle_placements[2].z_order > monocle_placements[0].z_order);

        // 5. Floating
        wm.dispatch_wm_action(&WmAction::SetLayoutMode {
            mode: LayoutMode::Floating,
        })
        .unwrap();
        let floating_placements = wm.compute_window_placements(usable);
        assert_eq!(floating_placements.len(), 3);
        assert!(floating_placements.iter().all(|p| p.is_floating));
    }

    #[test]
    fn test_workspace_switching_and_window_movement() {
        let mut wm = LuminaWmCore::new();
        assert_eq!(wm.workspaces().len(), 9);
        assert_eq!(wm.active_workspace_id(), 1);

        wm.on_window_opened(make_win(10, "Term", "term"));
        wm.on_window_opened(make_win(20, "Browser", "browser"));
        assert_eq!(wm.focused_window_id(), Some(20));

        // Move focused window (20) to workspace 3
        wm.dispatch_wm_action(&WmAction::MoveWindowToWorkspace {
            win_id: None,
            workspace: WorkspaceTarget::Index(3),
        })
        .unwrap();

        // Window 10 should now be focused on workspace 1
        assert_eq!(wm.focused_window_id(), Some(10));
        let ws_list = wm.workspaces();
        assert_eq!(ws_list[0].window_count, 1);
        assert_eq!(ws_list[2].window_count, 1);

        // Switch to workspace 3
        wm.dispatch_wm_action(&WmAction::FocusWorkspace {
            target: WorkspaceTarget::Index(3),
        })
        .unwrap();
        assert_eq!(wm.active_workspace_id(), 3);
        assert_eq!(wm.focused_window_id(), Some(20));

        // Relative workspace navigation (+1 -> 4, -3 -> 1)
        wm.dispatch_wm_action(&WmAction::FocusWorkspace {
            target: WorkspaceTarget::Relative(1),
        })
        .unwrap();
        assert_eq!(wm.active_workspace_id(), 4);
        assert_eq!(wm.focused_window_id(), None);

        wm.dispatch_wm_action(&WmAction::FocusWorkspace {
            target: WorkspaceTarget::Relative(-3),
        })
        .unwrap();
        assert_eq!(wm.active_workspace_id(), 1);
        assert_eq!(wm.focused_window_id(), Some(10));
    }

    #[test]
    fn test_directional_focus_and_move_swap() {
        let mut wm = LuminaWmCore::new();
        wm.on_window_opened(make_win(1, "Master", "master"));
        wm.on_window_opened(make_win(2, "StackTop", "top"));
        wm.on_window_opened(make_win(3, "StackBottom", "bottom"));

        // Initially window 3 (bottom-right) is focused
        assert_eq!(wm.focused_window_id(), Some(3));

        // Focus Up -> window 2 (top-right)
        wm.dispatch_wm_action(&WmAction::FocusDirection { dir: Direction::Up })
            .unwrap();
        assert_eq!(wm.focused_window_id(), Some(2));

        // Focus Left -> window 1 (master on left)
        wm.dispatch_wm_action(&WmAction::FocusDirection {
            dir: Direction::Left,
        })
        .unwrap();
        assert_eq!(wm.focused_window_id(), Some(1));

        // Swap master (1) to the right
        wm.dispatch_wm_action(&WmAction::MoveWindowDirection {
            dir: Direction::Right,
        })
        .unwrap();
        let wins = wm.windows();
        // Window 1 and Window 2 swapped slots in the tiled list
        assert_eq!(wins[0].id, 2);
        assert_eq!(wins[1].id, 1);
    }

    #[test]
    fn test_floating_fullscreen_minimize_and_interactive_grab() {
        let mut wm = LuminaWmCore::new();
        wm.on_window_opened(make_win(1, "App1", "app1"));
        wm.on_window_opened(make_win(2, "App2", "app2"));

        // Toggle floating on window 2
        wm.dispatch_wm_action(&WmAction::ToggleFloating { id: Some(2) })
            .unwrap();
        let w2 = wm.windows().into_iter().find(|w| w.id == 2).unwrap();
        assert!(w2.is_floating);

        // Nudge floating window right and down
        let orig_rect = w2.rect;
        wm.dispatch_wm_action(&WmAction::MoveWindowDirection {
            dir: Direction::Right,
        })
        .unwrap();
        let w2_moved = wm.windows().into_iter().find(|w| w.id == 2).unwrap();
        assert!(w2_moved.rect.x > orig_rect.x);

        // Toggle fullscreen on window 1
        wm.dispatch_wm_action(&WmAction::ToggleFullscreen { id: Some(1) })
            .unwrap();
        let placements = wm.compute_window_placements(wm.primary_usable_rect());
        let p1 = placements.iter().find(|p| p.window_id == 1).unwrap();
        assert!(p1.is_fullscreen);
        assert!(p1.titlebar_rect.is_none());
        assert_eq!(p1.border_width, 0.0);

        // FocusOrMinimize: window 2 is currently focused -> minimizes it and focuses window 1
        wm.dispatch_wm_action(&WmAction::FocusOrMinimizeWindow { id: 2 })
            .unwrap();
        assert_eq!(wm.focused_window_id(), Some(1));
        assert!(wm.windows().iter().find(|w| w.id == 2).unwrap().is_minimized);

        // FocusOrMinimize again on window 2 -> unminimizes and focuses window 2
        wm.dispatch_wm_action(&WmAction::FocusOrMinimizeWindow { id: 2 })
            .unwrap();
        assert_eq!(wm.focused_window_id(), Some(2));
        assert!(!wm.windows().iter().find(|w| w.id == 2).unwrap().is_minimized);

        // Interactive resize grab
        wm.dispatch_wm_action(&WmAction::BeginInteractiveResize {
            win_id: 2,
            edge: ResizeEdge::BottomRight,
        })
        .unwrap();
        let before_size = wm.windows().into_iter().find(|w| w.id == 2).unwrap().rect;
        wm.update_interactive_grab(50.0, 40.0);
        wm.end_interactive_grab();
        let after_size = wm.windows().into_iter().find(|w| w.id == 2).unwrap().rect;
        assert_eq!(after_size.width, before_size.width + 50.0);
        assert_eq!(after_size.height, before_size.height + 40.0);
    }

    #[test]
    fn test_reactive_state_patches_and_wayland_host() {
        let mut wm = LuminaWmCore::new();
        let mut host = WaylandCompositorHost::default();

        // Register top panel with 44px exclusive zone
        host.register_layer_surface(&mut wm, SurfaceMetadata::default(), None);

        // Map an XDG client window
        let patches = host.map_xdg_toplevel(&mut wm, make_win(42, "Code", "code"), None);

        let mut paths = BTreeMap::new();
        for patch in patches {
            if let StatePatch::Set { path, value } = patch {
                paths.insert(path, value);
            }
        }

        assert!(paths.contains_key("wm.workspaces"));
        assert!(paths.contains_key("wm.windows"));
        assert!(paths.contains_key("wm.focused_window"));
        assert_eq!(paths.get("wm.active_workspace"), Some(&StateValue::Int(1)));
        assert_eq!(
            paths.get("wm.layout_mode"),
            Some(&StateValue::String("tiling".to_string()))
        );
        assert_eq!(
            paths.get("wm.overview_open"),
            Some(&StateValue::Bool(false))
        );

        let focused = paths.get("wm.focused_window").unwrap();
        assert_eq!(
            focused.get_path("title"),
            Some(&StateValue::String("Code".to_string()))
        );

        // Check pending XDG configure event
        let configures = host.drain_pending_configures();
        assert_eq!(configures.len(), 1);
        assert_eq!(configures[0].window_id, 42);
        assert!(configures[0].is_activated);

        // Hit-test topbar vs client window
        assert_eq!(
            host.hit_test_surface_or_window(&wm, Point::new(100.0, 10.0)),
            Some(HitTarget::LayerSurface("panel".to_string()))
        );
        assert_eq!(
            host.hit_test_surface_or_window(&wm, Point::new(500.0, 500.0)),
            Some(HitTarget::ClientWindow {
                window_id: 42,
                in_titlebar: false
            })
        );

        // Composite output frame buffer
        let frame = host.composite_output_frame(&wm);
        assert_eq!(frame.width, 1920);
        assert_eq!(frame.height, 1080);
        assert!(frame.non_transparent_pixel_count() > 0);
    }
}
