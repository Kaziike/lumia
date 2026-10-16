use clap::{Parser, Subcommand, ValueEnum};
use ipc_server::{default_socket_path, IpcClient};
use std::collections::BTreeMap;
use std::path::PathBuf;
use wm_proto::{
    ActionCommand, DomElementInfo, IpcCommand, IpcPayload, IpcResponse, ReloadTarget, StateValue,
};

#[derive(Parser, Debug, Clone)]
#[command(
    name = "luminactl",
    version,
    about = "Command-line inspector, live CSS/DOM debugger, and IPC controller for Lumina WM"
)]
pub struct LuminaCtlCli {
    /// Path to the Lumina WM Unix domain socket
    #[arg(short, long, global = true)]
    pub socket: Option<PathBuf>,

    /// Output responses as pretty-printed JSON
    #[arg(short, long, global = true)]
    pub json: bool,

    #[command(subcommand)]
    pub command: CtlCommand,
}

#[derive(Subcommand, Debug, Clone)]
pub enum CtlCommand {
    /// Ping the running Lumina WM daemon
    Ping,
    /// Query Window Manager workspaces, windows, or outputs
    #[command(subcommand)]
    Wm(WmSubcommand),
    /// Dispatch one or more declarative action DSL commands (e.g. "wm:focus-workspace(2)")
    Dispatch {
        /// Declarative action DSL string
        #[arg(allow_hyphen_values = true)]
        dsl: String,
    },
    /// Inspect live HTML DOM elements, hierarchy tree, or computed CSS styles
    #[command(subcommand)]
    Dom(DomSubcommand),
    /// Live-edit CSS custom properties or inject CSS rules at runtime
    #[command(subcommand)]
    Css(CssSubcommand),
    /// Hot-reload CSS stylesheets, HTML templates, or all assets
    Reload {
        /// Target to reload: css, html, or all (defaults to all)
        #[arg(default_value = "all")]
        target: ReloadKindArg,
        /// Optional surface name to reload
        #[arg(long)]
        surface: Option<String>,
    },
    /// Manage HTML/CSS desktop shell widgets and layer surfaces
    #[command(subcommand)]
    Widget(WidgetSubcommand),
    /// Query or mutate the reactive state tree (`wm.*`, `sys.*`, `audio.*`, etc.)
    #[command(subcommand)]
    State(StateSubcommand),
}

#[derive(Subcommand, Debug, Clone)]
pub enum WmSubcommand {
    /// List all workspaces and their layout modes
    Workspaces,
    /// List all managed client windows and their geometries
    Windows,
    /// List all connected display outputs and usable areas
    Outputs,
}

#[derive(Subcommand, Debug, Clone)]
pub enum DomSubcommand {
    /// Query DOM elements matching a CSS selector
    Query {
        /// CSS selector (e.g. "#topbar", ".workspace-strip", "div")
        selector: String,
        /// Target shell surface name (defaults to "panel")
        #[arg(long)]
        surface: Option<String>,
    },
    /// Print the DOM hierarchy tree for a surface
    Tree {
        /// Target shell surface name (defaults to "panel")
        #[arg(long)]
        surface: Option<String>,
        /// Maximum tree depth to traverse
        #[arg(long)]
        depth: Option<usize>,
    },
    /// Inspect matched CSS rules and computed styles for a selector
    Style {
        /// CSS selector to inspect
        selector: String,
        /// Target shell surface name (defaults to "panel")
        #[arg(long)]
        surface: Option<String>,
    },
}

#[derive(Subcommand, Debug, Clone)]
pub enum CssSubcommand {
    /// Set a CSS custom property (`--var`) at runtime
    SetVar {
        /// Variable name (e.g. "--accent" or "--wm-gap-inner")
        #[arg(allow_hyphen_values = true)]
        name: String,
        /// Variable value (e.g. "#f43f5e" or "14px")
        #[arg(allow_hyphen_values = true)]
        value: String,
        /// Optional surface name
        #[arg(long)]
        surface: Option<String>,
    },
    /// Append and apply a raw CSS rule snippet at runtime
    Apply {
        /// Raw CSS rules text
        #[arg(allow_hyphen_values = true)]
        css_text: String,
        /// Optional surface name
        #[arg(long)]
        surface: Option<String>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum ReloadKindArg {
    Css,
    Html,
    All,
}

impl From<ReloadKindArg> for ReloadTarget {
    fn from(arg: ReloadKindArg) -> Self {
        match arg {
            ReloadKindArg::Css => ReloadTarget::Css,
            ReloadKindArg::Html => ReloadTarget::Html,
            ReloadKindArg::All => ReloadTarget::All,
        }
    }
}

#[derive(Subcommand, Debug, Clone)]
pub enum WidgetSubcommand {
    /// List all registered shell widgets and their visibility
    List,
    /// Open/show a shell widget by name
    Open { name: String },
    /// Close/hide a shell widget by name
    Close { name: String },
    /// Toggle visibility of a shell widget by name
    Toggle { name: String },
}

#[derive(Subcommand, Debug, Clone)]
pub enum StateSubcommand {
    /// Read a reactive state value at a dot-separated path (or full state tree if omitted)
    Get {
        /// Dot-separated state path (e.g. "wm.active_workspace", "sys.cpu.usage_pct")
        path: Option<String>,
    },
    /// Set a reactive state value at a dot-separated path
    Set {
        /// Dot-separated state path
        path: String,
        /// Value to set (parsed as JSON/bool/number/string)
        #[arg(allow_hyphen_values = true)]
        value: String,
    },
}

pub fn parse_state_value_arg(raw: &str) -> StateValue {
    let trimmed = raw.trim();
    if let Ok(val) = serde_json::from_str::<StateValue>(trimmed) {
        return val;
    }
    if trimmed.eq_ignore_ascii_case("true") {
        StateValue::Bool(true)
    } else if trimmed.eq_ignore_ascii_case("false") {
        StateValue::Bool(false)
    } else if let Ok(i) = trimmed.parse::<i64>() {
        StateValue::Int(i)
    } else if let Ok(f) = trimmed.parse::<f64>() {
        StateValue::Float(f)
    } else {
        StateValue::String(trimmed.to_string())
    }
}

/// Converts a parsed `CtlCommand` into one or more `IpcCommand`s.
pub fn build_ipc_commands(cmd: &CtlCommand) -> Result<Vec<IpcCommand>, String> {
    let out = match cmd {
        CtlCommand::Ping => vec![IpcCommand::Ping],
        CtlCommand::Wm(wm_sub) => match wm_sub {
            WmSubcommand::Workspaces => vec![IpcCommand::WmWorkspaces],
            WmSubcommand::Windows => vec![IpcCommand::WmWindows],
            WmSubcommand::Outputs => vec![IpcCommand::WmOutputs],
        },
        CtlCommand::Dispatch { dsl } => {
            let actions = ActionCommand::parse_dsl_list(dsl)?;
            actions
                .into_iter()
                .map(|action| IpcCommand::WmDispatch { action })
                .collect()
        }
        CtlCommand::Dom(dom_sub) => match dom_sub {
            DomSubcommand::Query { selector, surface } => vec![IpcCommand::DomQuery {
                surface: surface.clone(),
                selector: selector.clone(),
            }],
            DomSubcommand::Tree { surface, depth } => vec![IpcCommand::DomTree {
                surface: surface.clone(),
                max_depth: *depth,
            }],
            DomSubcommand::Style { selector, surface } => vec![IpcCommand::DomInspectStyle {
                surface: surface.clone(),
                selector: selector.clone(),
            }],
        },
        CtlCommand::Css(css_sub) => match css_sub {
            CssSubcommand::SetVar {
                name,
                value,
                surface,
            } => vec![IpcCommand::CssSetVar {
                surface: surface.clone(),
                name: name.clone(),
                value: value.clone(),
            }],
            CssSubcommand::Apply { css_text, surface } => vec![IpcCommand::CssApply {
                surface: surface.clone(),
                css_text: css_text.clone(),
            }],
        },
        CtlCommand::Reload { target, surface } => vec![IpcCommand::Reload {
            kind: (*target).into(),
            surface: surface.clone(),
        }],
        CtlCommand::Widget(w_sub) => match w_sub {
            WidgetSubcommand::List => vec![IpcCommand::WidgetList],
            WidgetSubcommand::Open { name } => vec![IpcCommand::WidgetOpen {
                name: name.clone(),
                args: BTreeMap::new(),
            }],
            WidgetSubcommand::Close { name } => vec![IpcCommand::WidgetClose {
                name: name.clone(),
            }],
            WidgetSubcommand::Toggle { name } => vec![IpcCommand::WidgetToggle {
                name: name.clone(),
                args: BTreeMap::new(),
            }],
        },
        CtlCommand::State(s_sub) => match s_sub {
            StateSubcommand::Get { path } => vec![IpcCommand::StateGet {
                path: path.clone(),
            }],
            StateSubcommand::Set { path, value } => vec![IpcCommand::StateSet {
                path: path.clone(),
                value: parse_state_value_arg(value),
            }],
        },
    };
    Ok(out)
}

/// Executes a `LuminaCtlCli` invocation against the socket and returns formatted output.
pub fn run_cli(cli: &LuminaCtlCli) -> Result<String, String> {
    let sock_path = cli.socket.clone().unwrap_or_else(default_socket_path);
    let client = IpcClient::new(sock_path);
    let commands = build_ipc_commands(&cli.command)?;

    let mut responses = Vec::with_capacity(commands.len());
    for cmd in commands {
        let resp = client.send(cmd).map_err(|e| e.to_string())?;
        if !resp.ok {
            return Err(resp
                .error
                .unwrap_or_else(|| "IPC command failed".to_string()));
        }
        responses.push(resp);
    }

    if cli.json {
        if responses.len() == 1 {
            serde_json::to_string_pretty(&responses[0]).map_err(|e| e.to_string())
        } else {
            serde_json::to_string_pretty(&responses).map_err(|e| e.to_string())
        }
    } else {
        let mut lines = Vec::new();
        for resp in &responses {
            lines.push(format_human_response(resp));
        }
        Ok(lines.join("\n"))
    }
}

pub fn format_human_response(resp: &IpcResponse) -> String {
    if !resp.ok {
        return format!(
            "Error: {}",
            resp.error.as_deref().unwrap_or("unknown error")
        );
    }
    let Some(payload) = &resp.payload else {
        return "OK".to_string();
    };

    match payload {
        IpcPayload::Pong { version } => format!("PONG (lumina-wm v{version})"),
        IpcPayload::Message(msg) => msg.clone(),
        IpcPayload::Workspaces(workspaces) => {
            let mut out = String::from("ID  NAME  OUTPUT  MODE       ACTIVE  WINDOWS\n");
            for ws in workspaces {
                let active_mark = if ws.is_active { "*" } else { " " };
                out.push_str(&format!(
                    "{:<3} {:<5} {:<7} {:<10} {:<7} {}\n",
                    ws.id,
                    ws.name,
                    ws.output,
                    ws.layout_mode.as_str(),
                    active_mark,
                    ws.window_count
                ));
            }
            out.trim_end().to_string()
        }
        IpcPayload::Windows(windows) => {
            if windows.is_empty() {
                return "No managed windows.".to_string();
            }
            let mut out = String::from("ID    WS  FOCUS  FLOAT  APP_ID                  TITLE\n");
            for w in windows {
                out.push_str(&format!(
                    "{:<5} {:<3} {:<6} {:<6} {:<23} {}\n",
                    w.id,
                    w.workspace_id,
                    if w.is_focused { "yes" } else { "no" },
                    if w.is_floating { "yes" } else { "no" },
                    w.app_id,
                    w.title
                ));
            }
            out.trim_end().to_string()
        }
        IpcPayload::Outputs(outputs) => {
            let mut out = String::from("NAME    MAKE     MODEL                    GEOMETRY           USABLE\n");
            for o in outputs {
                out.push_str(&format!(
                    "{:<7} {:<8} {:<24} {:.0}x{:.0}+{:.0}+{:.0}   {:.0}x{:.0}+{:.0}+{:.0}\n",
                    o.name,
                    o.make,
                    o.model,
                    o.rect.width,
                    o.rect.height,
                    o.rect.x,
                    o.rect.y,
                    o.usable_rect.width,
                    o.usable_rect.height,
                    o.usable_rect.x,
                    o.usable_rect.y
                ));
            }
            out.trim_end().to_string()
        }
        IpcPayload::Widgets(widgets) => {
            let mut out = String::from("WIDGET           LAYER     VISIBLE  SIZE       NODES\n");
            for w in widgets {
                out.push_str(&format!(
                    "{:<16} {:<9} {:<8} {:>4}x{:<4}  {}\n",
                    w.name,
                    w.layer,
                    if w.visible { "open" } else { "closed" },
                    w.width,
                    w.height,
                    w.node_count
                ));
            }
            out.trim_end().to_string()
        }
        IpcPayload::DomElements(elements) => {
            let mut lines = Vec::new();
            for el in elements {
                format_dom_element(el, 0, &mut lines);
            }
            lines.join("\n")
        }
        IpcPayload::StyleInspection(reports) => {
            serde_json::to_string_pretty(reports).unwrap_or_default()
        }
        IpcPayload::State(val) => serde_json::to_string_pretty(val).unwrap_or_default(),
    }
}

fn format_dom_element(el: &DomElementInfo, indent: usize, lines: &mut Vec<String>) {
    let pad = "  ".repeat(indent);
    let id_str = el
        .id
        .as_deref()
        .map(|i| format!("#{i}"))
        .unwrap_or_default();
    let cls_str = if el.classes.is_empty() {
        String::new()
    } else {
        format!(".{}", el.classes.join("."))
    };
    let rect_str = el
        .layout_box
        .map(|r| format!(" [{:.0}x{:.0}+{:.0}+{:.0}]", r.width, r.height, r.x, r.y))
        .unwrap_or_default();
    lines.push(format!(
        "{pad}<{}{}{}> (node {}){}",
        el.tag, id_str, cls_str, el.node_id, rect_str
    ));
    for child in &el.children {
        format_dom_element(child, indent + 1, lines);
    }
}
