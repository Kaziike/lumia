use crate::state::StateValue;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceTarget {
    Index(u32),
    Name(String),
    Relative(i32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutMode {
    Tiling,
    Scrolling,
    Floating,
    Monocle,
    Bsp,
}

impl Default for LayoutMode {
    fn default() -> Self {
        Self::Tiling
    }
}

impl LayoutMode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Tiling => "tiling",
            Self::Scrolling => "scrolling",
            Self::Floating => "floating",
            Self::Monocle => "monocle",
            Self::Bsp => "bsp",
        }
    }

    pub fn from_str_name(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "tiling" | "master" | "master-stack" => Some(Self::Tiling),
            "scrolling" | "columns" | "niri" => Some(Self::Scrolling),
            "floating" | "float" => Some(Self::Floating),
            "monocle" | "max" | "deck" => Some(Self::Monocle),
            "bsp" | "dwindle" => Some(Self::Bsp),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeEdge {
    Top,
    Bottom,
    Left,
    Right,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WmAction {
    FocusWorkspace { target: WorkspaceTarget },
    FocusWindow { id: u64 },
    FocusDirection { dir: Direction },
    CloseWindow { id: Option<u64> },
    ToggleFloating { id: Option<u64> },
    ToggleFullscreen { id: Option<u64> },
    ToggleMaximize { id: Option<u64> },
    MinimizeWindow { id: Option<u64> },
    FocusOrMinimizeWindow { id: u64 },
    MoveWindowToWorkspace { win_id: Option<u64>, workspace: WorkspaceTarget },
    MoveWindowDirection { dir: Direction },
    ResizeWindow { id: Option<u64>, dx: i32, dy: i32 },
    SetLayoutMode { mode: LayoutMode },
    CycleLayoutMode,
    ToggleOverview,
    BeginInteractiveMove { win_id: u64 },
    BeginInteractiveResize { win_id: u64, edge: ResizeEdge },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ShellAction {
    OpenWidget {
        name: String,
        #[serde(default)]
        args: BTreeMap<String, StateValue>,
    },
    CloseWidget {
        name: String,
    },
    ToggleWidget {
        name: String,
        #[serde(default)]
        args: BTreeMap<String, StateValue>,
    },
    ReloadCss {
        surface: Option<String>,
    },
    ReloadHtml {
        surface: Option<String>,
    },
    SetTheme {
        name: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SysAction {
    SetVolume { percent: u8 },
    ChangeVolume { delta: i8 },
    ToggleMute,
    SetBrightness { percent: u8 },
    SetResolution { width: u32, height: u32 },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AppsAction {
    SetQuery { query: String },
    SelectRelative { delta: i32 },
    Launch { app_id: String },
    LaunchSelected,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum NotifAction {
    Dismiss { id: u64 },
    ClearAll,
    ToggleDnd,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum StateAction {
    Set { path: String, value: StateValue },
    ToggleBool { path: String },
}

/// Unified action command triggered by HTML event attributes (`on-click="wm:focus-workspace(1)"`)
/// or external CLI IPC calls (`luminactl dispatch ...`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "domain", content = "action", rename_all = "snake_case")]
pub enum ActionCommand {
    Wm(WmAction),
    Shell(ShellAction),
    Sys(SysAction),
    Apps(AppsAction),
    Notif(NotifAction),
    State(StateAction),
    Exec { command: String, args: Vec<String> },
}

impl ActionCommand {
    /// Parses a semicolon-separated list of declarative action expressions, e.g.:
    /// `wm:focus-workspace(2)`
    /// `apps:launch('firefox'); shell:close-widget('launcher')`
    pub fn parse_dsl_list(input: &str) -> Result<Vec<Self>, String> {
        let mut out = Vec::new();
        for part in split_top_level(input, ';') {
            let trimmed = part.trim();
            if !trimmed.is_empty() {
                out.push(Self::parse_dsl(trimmed)?);
            }
        }
        Ok(out)
    }

    pub fn parse_dsl(input: &str) -> Result<Self, String> {
        let s = input.trim();
        let (domain_cmd, args_str) = if let Some(open_idx) = s.find('(') {
            let close_idx = s
                .rfind(')')
                .ok_or_else(|| format!("Missing closing ')' in action: {s}"))?;
            (&s[..open_idx], &s[open_idx + 1..close_idx])
        } else {
            (s, "")
        };

        let (domain, cmd) = domain_cmd
            .split_once(':')
            .unwrap_or(("wm", domain_cmd.trim()));
        let domain = domain.trim().to_ascii_lowercase();
        let cmd = cmd.trim().to_ascii_lowercase();
        let args: Vec<String> = if args_str.trim().is_empty() {
            Vec::new()
        } else {
            split_top_level(args_str, ',')
                .into_iter()
                .map(|a| unquote(a.trim()))
                .collect()
        };

        match domain.as_str() {
            "wm" => Self::parse_wm_dsl(&cmd, &args),
            "shell" => Self::parse_shell_dsl(&cmd, &args),
            "sys" => Self::parse_sys_dsl(&cmd, &args),
            "apps" => Self::parse_apps_dsl(&cmd, &args),
            "notif" => Self::parse_notif_dsl(&cmd, &args),
            "state" => Self::parse_state_dsl(&cmd, &args),
            "exec" => {
                if args.is_empty() && !cmd.is_empty() && cmd != "run" && cmd != "spawn" {
                    Ok(Self::Exec {
                        command: cmd,
                        args: Vec::new(),
                    })
                } else {
                    let command = args
                        .first()
                        .cloned()
                        .ok_or_else(|| "exec requires a command argument".to_string())?;
                    let exec_args = args.into_iter().skip(1).collect();
                    Ok(Self::Exec {
                        command,
                        args: exec_args,
                    })
                }
            }
            other => Err(format!("Unknown action domain '{other}' in '{s}'")),
        }
    }

    fn parse_wm_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "focus-workspace" => {
                let first = args.first().ok_or("focus-workspace requires an argument")?;
                let target = if let Ok(idx) = first.parse::<u32>() {
                    WorkspaceTarget::Index(idx)
                } else {
                    WorkspaceTarget::Name(first.clone())
                };
                WmAction::FocusWorkspace { target }
            }
            "focus-workspace-relative" => {
                let delta = args
                    .first()
                    .ok_or("focus-workspace-relative requires delta")?
                    .parse::<i32>()
                    .map_err(|e| e.to_string())?;
                WmAction::FocusWorkspace {
                    target: WorkspaceTarget::Relative(delta),
                }
            }
            "focus-window" => {
                let id = args
                    .first()
                    .ok_or("focus-window requires window id")?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                WmAction::FocusWindow { id }
            }
            "focus-or-minimize-window" => {
                let id = args
                    .first()
                    .ok_or("focus-or-minimize-window requires window id")?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                WmAction::FocusOrMinimizeWindow { id }
            }
            "focus-direction" | "focus" => {
                let dir = parse_direction(args.first().map(|s| s.as_str()).unwrap_or("right"))?;
                WmAction::FocusDirection { dir }
            }
            "close-window" | "close" => {
                let id = args.first().and_then(|s| s.parse::<u64>().ok());
                WmAction::CloseWindow { id }
            }
            "toggle-floating" => {
                let id = args.first().and_then(|s| s.parse::<u64>().ok());
                WmAction::ToggleFloating { id }
            }
            "toggle-fullscreen" => {
                let id = args.first().and_then(|s| s.parse::<u64>().ok());
                WmAction::ToggleFullscreen { id }
            }
            "toggle-maximize" => {
                let id = args.first().and_then(|s| s.parse::<u64>().ok());
                WmAction::ToggleMaximize { id }
            }
            "minimize-window" => {
                let id = args.first().and_then(|s| s.parse::<u64>().ok());
                WmAction::MinimizeWindow { id }
            }
            "move-window-to-workspace" => {
                let (win_id, ws_str) = if args.len() >= 2 {
                    (args[0].parse::<u64>().ok(), &args[1])
                } else if args.len() == 1 {
                    (None, &args[0])
                } else {
                    return Err("move-window-to-workspace requires workspace target".into());
                };
                let workspace = if let Ok(idx) = ws_str.parse::<u32>() {
                    WorkspaceTarget::Index(idx)
                } else {
                    WorkspaceTarget::Name(ws_str.clone())
                };
                WmAction::MoveWindowToWorkspace { win_id, workspace }
            }
            "move-window-direction" | "swap-direction" => {
                let dir = parse_direction(args.first().map(|s| s.as_str()).unwrap_or("right"))?;
                WmAction::MoveWindowDirection { dir }
            }
            "resize-window" => {
                let (id, dx, dy) = if args.len() >= 3 {
                    (
                        args[0].parse::<u64>().ok(),
                        args[1].parse::<i32>().unwrap_or(0),
                        args[2].parse::<i32>().unwrap_or(0),
                    )
                } else if args.len() == 2 {
                    (
                        None,
                        args[0].parse::<i32>().unwrap_or(0),
                        args[1].parse::<i32>().unwrap_or(0),
                    )
                } else {
                    return Err("resize-window requires (dx, dy) or (id, dx, dy)".into());
                };
                WmAction::ResizeWindow { id, dx, dy }
            }
            "set-layout-mode" | "set-layout" => {
                let mode_str = args.first().ok_or("set-layout-mode requires mode name")?;
                let mode = LayoutMode::from_str_name(mode_str)
                    .ok_or_else(|| format!("Unknown layout mode '{mode_str}'"))?;
                WmAction::SetLayoutMode { mode }
            }
            "cycle-layout-mode" | "cycle-layout" => WmAction::CycleLayoutMode,
            "toggle-overview" => WmAction::ToggleOverview,
            "begin-move" => {
                let win_id = args
                    .first()
                    .ok_or("begin-move requires win_id")?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                WmAction::BeginInteractiveMove { win_id }
            }
            other => return Err(format!("Unknown wm action '{other}'")),
        };
        Ok(Self::Wm(act))
    }

    fn parse_shell_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "open-widget" | "open" => {
                let name = args.first().ok_or("open-widget requires widget name")?.clone();
                ShellAction::OpenWidget {
                    name,
                    args: BTreeMap::new(),
                }
            }
            "close-widget" | "close" => {
                let name = args.first().ok_or("close-widget requires widget name")?.clone();
                ShellAction::CloseWidget { name }
            }
            "toggle-widget" | "toggle" => {
                let name = args.first().ok_or("toggle-widget requires widget name")?.clone();
                ShellAction::ToggleWidget {
                    name,
                    args: BTreeMap::new(),
                }
            }
            "reload-css" => ShellAction::ReloadCss {
                surface: args.first().cloned(),
            },
            "reload-html" => ShellAction::ReloadHtml {
                surface: args.first().cloned(),
            },
            "set-theme" => {
                let name = args.first().ok_or("set-theme requires theme name")?.clone();
                ShellAction::SetTheme { name }
            }
            other => return Err(format!("Unknown shell action '{other}'")),
        };
        Ok(Self::Shell(act))
    }

    fn parse_sys_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "set-volume" => {
                let percent = args
                    .first()
                    .ok_or("set-volume requires percent")?
                    .parse::<f64>()
                    .map_err(|e| e.to_string())?
                    .clamp(0.0, 100.0) as u8;
                SysAction::SetVolume { percent }
            }
            "change-volume" => {
                let delta = args
                    .first()
                    .ok_or("change-volume requires delta")?
                    .parse::<i8>()
                    .map_err(|e| e.to_string())?;
                SysAction::ChangeVolume { delta }
            }
            "toggle-mute" => SysAction::ToggleMute,
            "set-brightness" => {
                let percent = args
                    .first()
                    .ok_or("set-brightness requires percent")?
                    .parse::<f64>()
                    .map_err(|e| e.to_string())?
                    .clamp(0.0, 100.0) as u8;
                SysAction::SetBrightness { percent }
            }
            "set-resolution" => {
                let (width, height) = if args.len() >= 2 {
                    let w = args[0]
                        .trim()
                        .parse::<u32>()
                        .map_err(|e| format!("Invalid width in set-resolution: {e}"))?;
                    let h = args[1]
                        .trim()
                        .parse::<u32>()
                        .map_err(|e| format!("Invalid height in set-resolution: {e}"))?;
                    (w, h)
                } else if let Some(first) = args.first() {
                    let (w_str, h_str) = first
                        .split_once('x')
                        .or_else(|| first.split_once('X'))
                        .ok_or_else(|| {
                            "set-resolution requires 'WxH' or (width, height)".to_string()
                        })?;
                    let w = w_str
                        .trim()
                        .parse::<u32>()
                        .map_err(|e| format!("Invalid width in set-resolution: {e}"))?;
                    let h = h_str
                        .trim()
                        .parse::<u32>()
                        .map_err(|e| format!("Invalid height in set-resolution: {e}"))?;
                    (w, h)
                } else {
                    return Err("set-resolution requires 'WxH' or (width, height)".to_string());
                };
                SysAction::SetResolution { width, height }
            }
            other => return Err(format!("Unknown sys action '{other}'")),
        };
        Ok(Self::Sys(act))
    }

    fn parse_apps_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "set-query" => AppsAction::SetQuery {
                query: args.first().cloned().unwrap_or_default(),
            },
            "select-relative" => {
                let delta = args
                    .first()
                    .and_then(|s| s.parse::<i32>().ok())
                    .unwrap_or(1);
                AppsAction::SelectRelative { delta }
            }
            "launch" => {
                let app_id = args.first().ok_or("apps:launch requires app_id")?.clone();
                AppsAction::Launch { app_id }
            }
            "launch-selected" => AppsAction::LaunchSelected,
            other => return Err(format!("Unknown apps action '{other}'")),
        };
        Ok(Self::Apps(act))
    }

    fn parse_notif_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "dismiss" => {
                let id = args
                    .first()
                    .ok_or("notif:dismiss requires id")?
                    .parse::<u64>()
                    .map_err(|e| e.to_string())?;
                NotifAction::Dismiss { id }
            }
            "clear-all" => NotifAction::ClearAll,
            "toggle-dnd" => NotifAction::ToggleDnd,
            other => return Err(format!("Unknown notif action '{other}'")),
        };
        Ok(Self::Notif(act))
    }

    fn parse_state_dsl(cmd: &str, args: &[String]) -> Result<Self, String> {
        let act = match cmd {
            "set" => {
                if args.len() < 2 {
                    return Err("state:set requires (path, value)".into());
                }
                let path = args[0].clone();
                let raw = &args[1];
                let value = if raw == "true" {
                    StateValue::Bool(true)
                } else if raw == "false" {
                    StateValue::Bool(false)
                } else if let Ok(i) = raw.parse::<i64>() {
                    StateValue::Int(i)
                } else if let Ok(f) = raw.parse::<f64>() {
                    StateValue::Float(f)
                } else {
                    StateValue::String(raw.clone())
                };
                StateAction::Set { path, value }
            }
            "toggle" | "toggle-bool" => {
                let path = args.first().ok_or("state:toggle requires path")?.clone();
                StateAction::ToggleBool { path }
            }
            other => return Err(format!("Unknown state action '{other}'")),
        };
        Ok(Self::State(act))
    }
}

fn parse_direction(s: &str) -> Result<Direction, String> {
    match s.trim().to_ascii_lowercase().as_str() {
        "left" | "l" | "west" => Ok(Direction::Left),
        "right" | "r" | "east" => Ok(Direction::Right),
        "up" | "u" | "north" => Ok(Direction::Up),
        "down" | "d" | "south" => Ok(Direction::Down),
        other => Err(format!("Invalid direction '{other}'")),
    }
}

fn split_top_level(s: &str, delimiter: char) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut in_single = false;
    let mut in_double = false;
    let mut start = 0usize;

    for (idx, ch) in s.char_indices() {
        match ch {
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '(' | '[' | '{' if !in_single && !in_double => depth += 1,
            ')' | ']' | '}' if !in_single && !in_double => depth = depth.saturating_sub(1),
            c if c == delimiter && depth == 0 && !in_single && !in_double => {
                parts.push(&s[start..idx]);
                start = idx + c.len_utf8();
            }
            _ => {}
        }
    }
    parts.push(&s[start..]);
    parts
}

fn unquote(s: &str) -> String {
    let bytes = s.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"'))
    {
        s[1..s.len() - 1].to_string()
    } else {
        s.to_string()
    }
}
