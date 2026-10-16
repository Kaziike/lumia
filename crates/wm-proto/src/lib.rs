pub mod action;
pub mod dom;
pub mod ipc;
pub mod layout;
pub mod state;
pub mod style;
pub mod wm;

pub use action::*;
pub use dom::*;
pub use ipc::*;
pub use layout::*;
pub use state::*;
pub use style::*;
pub use wm::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_action_dsl_parsing() {
        let cmds = ActionCommand::parse_dsl_list(
            "wm:focus-workspace(3); apps:launch('org.mozilla.firefox'); shell:close-widget('launcher')",
        )
        .expect("should parse action list");
        assert_eq!(cmds.len(), 3);
        assert_eq!(
            cmds[0],
            ActionCommand::Wm(WmAction::FocusWorkspace {
                target: WorkspaceTarget::Index(3)
            })
        );
        assert_eq!(
            cmds[1],
            ActionCommand::Apps(AppsAction::Launch {
                app_id: "org.mozilla.firefox".to_string()
            })
        );
        assert_eq!(
            cmds[2],
            ActionCommand::Shell(ShellAction::CloseWidget {
                name: "launcher".to_string()
            })
        );
    }

    #[test]
    fn test_state_path_overlap() {
        let p1 = StatePath::new("sys.cpu");
        let p2 = StatePath::new("sys.cpu.usage_pct");
        let p3 = StatePath::new("sys.ram");
        assert!(p1.overlaps(&p2));
        assert!(p2.overlaps(&p1));
        assert!(!p1.overlaps(&p3));
    }
}
