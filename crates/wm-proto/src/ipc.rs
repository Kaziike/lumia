use crate::action::ActionCommand;
use crate::dom::DomElementInfo;
use crate::state::StateValue;
use crate::style::StyleInspectionReport;
use crate::wm::{OutputDescriptor, WindowDescriptor, WorkspaceDescriptor};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const DEFAULT_SOCKET_NAME: &str = "lumina-wm.sock";

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReloadTarget {
    Css,
    Html,
    All,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SubscriptionTopic {
    Wm,
    State,
    Dom,
    Widgets,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WidgetStatus {
    pub name: String,
    pub layer: String,
    pub visible: bool,
    pub width: u32,
    pub height: u32,
    pub node_count: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IpcRequest {
    pub id: u64,
    pub command: IpcCommand,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum IpcCommand {
    Ping,
    // 1. DOM & Style Inspection
    DomQuery {
        surface: Option<String>,
        selector: String,
    },
    DomTree {
        surface: Option<String>,
        max_depth: Option<usize>,
    },
    DomInspectStyle {
        surface: Option<String>,
        selector: String,
    },
    DomSetAttr {
        surface: String,
        selector: String,
        attr: String,
        value: Option<String>,
    },
    // 2. Hot-Reload & Live CSS Injection
    Reload {
        kind: ReloadTarget,
        surface: Option<String>,
    },
    CssApply {
        surface: Option<String>,
        css_text: String,
    },
    CssSetVar {
        surface: Option<String>,
        name: String,
        value: String,
    },
    // 3. Widget / Surface Lifecycle
    WidgetList,
    WidgetOpen {
        name: String,
        #[serde(default)]
        args: BTreeMap<String, StateValue>,
    },
    WidgetClose {
        name: String,
    },
    WidgetToggle {
        name: String,
        #[serde(default)]
        args: BTreeMap<String, StateValue>,
    },
    // 4. Reactive State Store
    StateGet {
        path: Option<String>,
    },
    StateSet {
        path: String,
        value: StateValue,
    },
    // 5. Window Manager Control & Queries
    WmDispatch {
        action: ActionCommand,
    },
    WmWindows,
    WmWorkspaces,
    WmOutputs,
    // 6. Real-Time Event Subscription
    Subscribe {
        topics: Vec<SubscriptionTopic>,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IpcResponse {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<IpcPayload>,
}

impl IpcResponse {
    pub fn success(id: u64, payload: IpcPayload) -> Self {
        Self {
            id,
            ok: true,
            error: None,
            payload: Some(payload),
        }
    }

    pub fn err(id: u64, msg: impl Into<String>) -> Self {
        Self {
            id,
            ok: false,
            error: Some(msg.into()),
            payload: None,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "data", rename_all = "snake_case")]
pub enum IpcPayload {
    Pong { version: String },
    Message(String),
    DomElements(Vec<DomElementInfo>),
    StyleInspection(Vec<StyleInspectionReport>),
    Widgets(Vec<WidgetStatus>),
    State(StateValue),
    Windows(Vec<WindowDescriptor>),
    Workspaces(Vec<WorkspaceDescriptor>),
    Outputs(Vec<OutputDescriptor>),
}
