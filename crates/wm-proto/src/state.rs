use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Dynamic reactive value stored in the hierarchical state tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum StateValue {
    Null,
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
    List(Vec<StateValue>),
    Map(BTreeMap<String, StateValue>),
}

impl Default for StateValue {
    fn default() -> Self {
        Self::Null
    }
}

impl StateValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s.as_str()),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> bool {
        match self {
            Self::Null => false,
            Self::Bool(b) => *b,
            Self::Int(i) => *i != 0,
            Self::Float(f) => *f != 0.0 && !f.is_nan(),
            Self::String(s) => !s.is_empty() && s != "false" && s != "0",
            Self::List(l) => !l.is_empty(),
            Self::Map(m) => !m.is_empty(),
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            Self::String(s) => s.trim().parse::<f64>().ok(),
            Self::Bool(b) => Some(if *b { 1.0 } else { 0.0 }),
            _ => None,
        }
    }

    pub fn as_i64(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            Self::Float(f) => Some(*f as i64),
            Self::String(s) => s.trim().parse::<i64>().ok(),
            Self::Bool(b) => Some(if *b { 1 } else { 0 }),
            _ => None,
        }
    }

    pub fn to_display_string(&self) -> String {
        match self {
            Self::Null => String::new(),
            Self::Bool(b) => b.to_string(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => {
                if (f.fract()).abs() < 1e-9 {
                    format!("{:.0}", f)
                } else {
                    format!("{}", f)
                }
            }
            Self::String(s) => s.clone(),
            Self::List(items) => serde_json::to_string(items).unwrap_or_default(),
            Self::Map(map) => serde_json::to_string(map).unwrap_or_default(),
        }
    }

    pub fn get_path(&self, path: &str) -> Option<&StateValue> {
        if path.is_empty() {
            return Some(self);
        }
        let mut current = self;
        for segment in path.split('.') {
            if segment.is_empty() {
                continue;
            }
            match current {
                Self::Map(map) => {
                    current = map.get(segment)?;
                }
                Self::List(list) => {
                    let idx: usize = segment.parse().ok()?;
                    current = list.get(idx)?;
                }
                _ => return None,
            }
        }
        Some(current)
    }
}

impl From<bool> for StateValue {
    fn from(v: bool) -> Self {
        Self::Bool(v)
    }
}

impl From<i64> for StateValue {
    fn from(v: i64) -> Self {
        Self::Int(v)
    }
}

impl From<i32> for StateValue {
    fn from(v: i32) -> Self {
        Self::Int(v as i64)
    }
}

impl From<u64> for StateValue {
    fn from(v: u64) -> Self {
        Self::Int(v as i64)
    }
}

impl From<u32> for StateValue {
    fn from(v: u32) -> Self {
        Self::Int(v as i64)
    }
}

impl From<usize> for StateValue {
    fn from(v: usize) -> Self {
        Self::Int(v as i64)
    }
}

impl From<f64> for StateValue {
    fn from(v: f64) -> Self {
        Self::Float(v)
    }
}

impl From<f32> for StateValue {
    fn from(v: f32) -> Self {
        Self::Float(v as f64)
    }
}

impl From<String> for StateValue {
    fn from(v: String) -> Self {
        Self::String(v)
    }
}

impl From<&str> for StateValue {
    fn from(v: &str) -> Self {
        Self::String(v.to_string())
    }
}

/// Dot-separated state path (e.g. `wm.workspaces`, `sys.cpu.usage_pct`).
#[derive(Clone, Debug, Hash, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
pub struct StatePath(pub String);

impl StatePath {
    pub fn new(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Returns true if `self` and `other` overlap in hierarchy (either is a prefix of the other).
    pub fn overlaps(&self, other: &StatePath) -> bool {
        if self.0 == other.0 {
            return true;
        }
        if let Some(rest) = self.0.strip_prefix(&other.0) {
            if rest.starts_with('.') {
                return true;
            }
        }
        if let Some(rest) = other.0.strip_prefix(&self.0) {
            if rest.starts_with('.') {
                return true;
            }
        }
        false
    }
}

/// Atomic state mutation emitted by providers, WM core, or user actions.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum StatePatch {
    Set {
        path: String,
        value: StateValue,
    },
    Merge {
        path: String,
        fields: BTreeMap<String, StateValue>,
    },
    ListUpsert {
        path: String,
        key_field: String,
        item: StateValue,
    },
    ListRemove {
        path: String,
        key_field: String,
        key_value: StateValue,
    },
}

/// Trait implemented by `crates/reactive-state` for state management and expression evaluation.
pub trait ReactiveStore: Send + Sync {
    fn get(&self, path: &str) -> Option<StateValue>;
    fn set(&mut self, path: &str, value: StateValue) -> Vec<StatePath>;
    fn apply_patch(&mut self, patch: StatePatch) -> Vec<StatePath>;
    fn apply_patches(&mut self, patches: Vec<StatePatch>) -> Vec<StatePath> {
        let mut changed = Vec::new();
        for patch in patches {
            for p in self.apply_patch(patch) {
                if !changed.contains(&p) {
                    changed.push(p);
                }
            }
        }
        changed
    }
    fn evaluate_expr(
        &self,
        expr: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<StateValue, String>;
    fn interpolate_string(
        &self,
        template: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<String, String>;
    fn snapshot(&self) -> StateValue;
}

/// Trait implemented by system and desktop state providers in `crates/sys-providers`.
pub trait StateProvider: Send {
    fn namespace(&self) -> &'static str;
    fn initial_state(&mut self) -> Vec<StatePatch>;
    fn poll_updates(&mut self) -> Vec<StatePatch>;
    fn handle_action(&mut self, action: &crate::action::ActionCommand) -> Result<Vec<StatePatch>, String>;
}
