pub use wm_proto;

use std::collections::BTreeMap;
use wm_proto::{ReactiveStore, StateAction, StatePatch, StatePath, StateValue};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Subscription {
    path_prefix: StatePath,
    callback_id: u64,
}

/// Hierarchical reactive state store supporting dot-path reads/writes, atomic patches,
/// prefix/child subscriptions, and declarative expression evaluation.
#[derive(Clone, Debug, Default)]
pub struct StateStore {
    root: BTreeMap<String, StateValue>,
    subscriptions: Vec<Subscription>,
}

impl StateStore {
    pub fn new() -> Self {
        Self {
            root: BTreeMap::new(),
            subscriptions: Vec::new(),
        }
    }

    pub fn from_map(root: BTreeMap<String, StateValue>) -> Self {
        Self {
            root,
            subscriptions: Vec::new(),
        }
    }

    pub fn root_map(&self) -> &BTreeMap<String, StateValue> {
        &self.root
    }

    /// Registers a subscription callback ID to be notified whenever a path overlapping
    /// `path_prefix` is mutated.
    pub fn subscribe(&mut self, path_prefix: impl Into<String>, callback_id: u64) {
        let prefix = normalize_path(&path_prefix.into());
        let entry = Subscription {
            path_prefix: StatePath::new(prefix),
            callback_id,
        };
        if !self.subscriptions.contains(&entry) {
            self.subscriptions.push(entry);
        }
    }

    /// Removes all subscriptions associated with `callback_id`.
    pub fn unsubscribe(&mut self, callback_id: u64) {
        self.subscriptions.retain(|s| s.callback_id != callback_id);
    }

    /// Returns the list of unique `callback_id`s whose subscribed `path_prefix` overlaps
    /// any path in `changed_paths`.
    pub fn triggered_subscriptions(&self, changed_paths: &[StatePath]) -> Vec<u64> {
        if changed_paths.is_empty() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for sub in &self.subscriptions {
            let matches = if sub.path_prefix.as_str().is_empty() {
                true
            } else {
                changed_paths
                    .iter()
                    .any(|changed| changed.as_str().is_empty() || sub.path_prefix.overlaps(changed))
            };
            if matches && !out.contains(&sub.callback_id) {
                out.push(sub.callback_id);
            }
        }
        out
    }

    /// Alias for [`Self::triggered_subscriptions`].
    pub fn matching_subscriptions(&self, changed_paths: &[StatePath]) -> Vec<u64> {
        self.triggered_subscriptions(changed_paths)
    }

    /// Applies a `StateAction` (`Set` or `ToggleBool`) and returns affected paths.
    pub fn apply_state_action(&mut self, action: &StateAction) -> Vec<StatePath> {
        match action {
            StateAction::Set { path, value } => self.set(path, value.clone()),
            StateAction::ToggleBool { path } => {
                let current = self.get(path).map(|v| v.as_bool()).unwrap_or(false);
                self.set(path, StateValue::Bool(!current))
            }
        }
    }

    pub fn get(&self, path: &str) -> Option<StateValue> {
        <Self as ReactiveStore>::get(self, path)
    }

    pub fn set(&mut self, path: &str, value: StateValue) -> Vec<StatePath> {
        <Self as ReactiveStore>::set(self, path, value)
    }

    pub fn apply_patch(&mut self, patch: StatePatch) -> Vec<StatePath> {
        <Self as ReactiveStore>::apply_patch(self, patch)
    }

    pub fn apply_patches(&mut self, patches: Vec<StatePatch>) -> Vec<StatePath> {
        <Self as ReactiveStore>::apply_patches(self, patches)
    }

    pub fn evaluate_expr(
        &self,
        expr: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<StateValue, String> {
        <Self as ReactiveStore>::evaluate_expr(self, expr, local_scope)
    }

    pub fn interpolate_string(
        &self,
        template: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<String, String> {
        <Self as ReactiveStore>::interpolate_string(self, template, local_scope)
    }

    pub fn snapshot(&self) -> StateValue {
        <Self as ReactiveStore>::snapshot(self)
    }
}

fn normalize_path(raw: &str) -> String {
    let trimmed = raw.trim().trim_matches('.');
    if !trimmed.contains('[') {
        return trimmed.to_string();
    }
    let mut out = String::with_capacity(trimmed.len());
    for ch in trimmed.chars() {
        match ch {
            '[' => {
                if !out.is_empty() && !out.ends_with('.') {
                    out.push('.');
                }
            }
            ']' => {}
            _ => out.push(ch),
        }
    }
    out.trim_matches('.').to_string()
}

fn lookup_in_value(root: &StateValue, path: &str) -> Option<StateValue> {
    let norm = normalize_path(path);
    if norm.is_empty() {
        return Some(root.clone());
    }
    if let Some(found) = root.get_path(&norm) {
        return Some(found.clone());
    }
    // Support `.len` / `.length` synthetic properties on Lists, Strings, and Maps.
    if let Some((parent_path, last)) = norm.rsplit_once('.') {
        if last == "len" || last == "length" {
            let parent = root.get_path(parent_path)?;
            return value_length(parent);
        }
    } else if norm == "len" || norm == "length" {
        return value_length(root);
    }
    None
}

fn value_length(val: &StateValue) -> Option<StateValue> {
    match val {
        StateValue::List(l) => Some(StateValue::Int(l.len() as i64)),
        StateValue::String(s) => Some(StateValue::Int(s.chars().count() as i64)),
        StateValue::Map(m) => Some(StateValue::Int(m.len() as i64)),
        _ => None,
    }
}

fn ensure_mut_slot<'a>(
    root: &'a mut BTreeMap<String, StateValue>,
    segments: &[&str],
) -> Option<&'a mut StateValue> {
    if segments.is_empty() {
        return None;
    }
    let first = segments[0];
    let mut current = root
        .entry(first.to_string())
        .or_insert_with(|| StateValue::Map(BTreeMap::new()));

    for &seg in &segments[1..] {
        let is_list_idx = match current {
            StateValue::List(list) => seg.parse::<usize>().ok().filter(|&i| i < list.len()),
            _ => None,
        };
        if let Some(idx) = is_list_idx {
            if let StateValue::List(list) = current {
                current = &mut list[idx];
                continue;
            }
        }
        if !matches!(current, StateValue::Map(_)) {
            *current = StateValue::Map(BTreeMap::new());
        }
        if let StateValue::Map(map) = current {
            current = map
                .entry(seg.to_string())
                .or_insert_with(|| StateValue::Map(BTreeMap::new()));
        } else {
            return None;
        }
    }
    Some(current)
}

impl ReactiveStore for StateStore {
    fn get(&self, path: &str) -> Option<StateValue> {
        let norm = normalize_path(path);
        if norm.is_empty() {
            return Some(StateValue::Map(self.root.clone()));
        }
        let mut segments = norm.split('.').filter(|s| !s.is_empty());
        let first = segments.next()?;
        let first_val = self.root.get(first)?;
        let rest: Vec<&str> = segments.collect();
        if rest.is_empty() {
            return Some(first_val.clone());
        }
        lookup_in_value(first_val, &rest.join("."))
    }

    fn set(&mut self, path: &str, value: StateValue) -> Vec<StatePath> {
        let norm = normalize_path(path);
        if norm.is_empty() {
            if let StateValue::Map(map) = value {
                self.root = map;
                return vec![StatePath::new("")];
            }
            return Vec::new();
        }
        let segments: Vec<&str> = norm.split('.').filter(|s| !s.is_empty()).collect();
        if segments.is_empty() {
            return Vec::new();
        }
        if segments.len() == 1 {
            self.root.insert(segments[0].to_string(), value);
            return vec![StatePath::new(norm)];
        }
        if let Some(slot) = ensure_mut_slot(&mut self.root, &segments) {
            *slot = value;
            vec![StatePath::new(norm)]
        } else {
            Vec::new()
        }
    }

    fn apply_patch(&mut self, patch: StatePatch) -> Vec<StatePath> {
        match patch {
            StatePatch::Set { path, value } => self.set(&path, value),
            StatePatch::Merge { path, fields } => {
                let norm = normalize_path(&path);
                if fields.is_empty() {
                    return if norm.is_empty() {
                        Vec::new()
                    } else {
                        vec![StatePath::new(norm)]
                    };
                }
                let mut affected = Vec::with_capacity(fields.len() + 1);
                if !norm.is_empty() {
                    affected.push(StatePath::new(norm.clone()));
                }
                for (k, v) in fields {
                    let full_path = if norm.is_empty() {
                        k
                    } else {
                        format!("{norm}.{k}")
                    };
                    for p in self.set(&full_path, v) {
                        if !affected.contains(&p) {
                            affected.push(p);
                        }
                    }
                }
                affected
            }
            StatePatch::ListUpsert {
                path,
                key_field,
                item,
            } => {
                let norm = normalize_path(&path);
                let segments: Vec<&str> = norm.split('.').filter(|s| !s.is_empty()).collect();
                if segments.is_empty() {
                    return Vec::new();
                }
                let Some(slot) = ensure_mut_slot(&mut self.root, &segments) else {
                    return Vec::new();
                };
                if !matches!(slot, StateValue::List(_)) {
                    *slot = StateValue::List(Vec::new());
                }
                if let StateValue::List(list) = slot {
                    if !key_field.is_empty() {
                        let target_key = item.get_path(&key_field).cloned();
                        if let Some(tk) = target_key {
                            if let Some(existing) = list
                                .iter_mut()
                                .find(|el| el.get_path(&key_field) == Some(&tk))
                            {
                                *existing = item;
                            } else {
                                list.push(item);
                            }
                        } else {
                            list.push(item);
                        }
                    } else if let Some(existing) = list.iter_mut().find(|el| **el == item) {
                        *existing = item;
                    } else {
                        list.push(item);
                    }
                }
                vec![StatePath::new(norm)]
            }
            StatePatch::ListRemove {
                path,
                key_field,
                key_value,
            } => {
                let norm = normalize_path(&path);
                let segments: Vec<&str> = norm.split('.').filter(|s| !s.is_empty()).collect();
                if segments.is_empty() {
                    return Vec::new();
                }
                let Some(slot) = ensure_mut_slot(&mut self.root, &segments) else {
                    return Vec::new();
                };
                if let StateValue::List(list) = slot {
                    if !key_field.is_empty() {
                        list.retain(|el| el.get_path(&key_field) != Some(&key_value));
                    } else {
                        list.retain(|el| el != &key_value);
                    }
                }
                vec![StatePath::new(norm)]
            }
        }
    }

    fn evaluate_expr(
        &self,
        expr: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<StateValue, String> {
        evaluate_expr(self, expr, local_scope)
    }

    fn interpolate_string(
        &self,
        template: &str,
        local_scope: Option<&BTreeMap<String, StateValue>>,
    ) -> Result<String, String> {
        interpolate_string(self, template, local_scope)
    }

    fn snapshot(&self) -> StateValue {
        StateValue::Map(self.root.clone())
    }
}

/// Evaluates a declarative expression against `store` and an optional `local_scope`.
pub fn evaluate_expr(
    store: &dyn ReactiveStore,
    expr: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> Result<StateValue, String> {
    let trimmed = expr.trim();
    if trimmed.is_empty() {
        return Ok(StateValue::Null);
    }
    let tokens = tokenize(trimmed)?;
    let mut parser = Parser::new(&tokens, store, local_scope);
    let val = parser.parse_pipe()?;
    if parser.pos < parser.tokens.len() {
        return Err(format!(
            "Unexpected token {:?} in expression '{expr}'",
            parser.tokens[parser.pos]
        ));
    }
    Ok(val)
}

/// Replaces all `{{ ... }}` blocks in `template` with their evaluated `to_display_string()`.
pub fn interpolate_string(
    store: &dyn ReactiveStore,
    template: &str,
    local_scope: Option<&BTreeMap<String, StateValue>>,
) -> Result<String, String> {
    if !template.contains("{{") {
        return Ok(template.to_string());
    }

    let mut out = String::with_capacity(template.len());
    let bytes = template.as_bytes();
    let mut i = 0;

    while i < bytes.len() {
        if i + 1 < bytes.len() && bytes[i] == b'{' && bytes[i + 1] == b'{' {
            let expr_start = i + 2;
            let mut j = expr_start;
            let mut in_single = false;
            let mut in_double = false;
            let mut found_end = None;

            while j < bytes.len() {
                let b = bytes[j];
                if b == b'\\' && (in_single || in_double) {
                    j += 2;
                    continue;
                }
                if b == b'\'' && !in_double {
                    in_single = !in_single;
                } else if b == b'"' && !in_single {
                    in_double = !in_double;
                } else if !in_single
                    && !in_double
                    && j + 1 < bytes.len()
                    && bytes[j] == b'}'
                    && bytes[j + 1] == b'}'
                {
                    found_end = Some(j);
                    break;
                }
                j += 1;
            }

            let end_idx =
                found_end.ok_or_else(|| format!("Unclosed '{{{{' in template: {template}"))?;
            let inner = &template[expr_start..end_idx];
            let val = evaluate_expr(store, inner, local_scope)?;
            out.push_str(&val.to_display_string());
            i = end_idx + 2;
        } else {
            let ch = template[i..].chars().next().unwrap();
            out.push(ch);
            i += ch.len_utf8();
        }
    }

    Ok(out)
}

#[derive(Clone, Debug, PartialEq)]
enum Token {
    Str(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Null,
    Ident(String),
    LParen,
    RParen,
    Comma,
    Question,
    Colon,
    Pipe,
    OrOr,
    AndAnd,
    EqEq,
    NotEq,
    Gt,
    Lt,
    Ge,
    Le,
    Plus,
    Minus,
    Star,
    Slash,
    Percent,
    Bang,
}

fn tokenize(input: &str) -> Result<Vec<Token>, String> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = input.chars().collect();
    let len = chars.len();
    let mut i = 0;

    while i < len {
        let c = chars[i];
        if c.is_whitespace() {
            i += 1;
            continue;
        }

        // String literal
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            let mut s = String::new();
            let mut closed = false;
            while i < len {
                let ch = chars[i];
                if ch == '\\' && i + 1 < len {
                    i += 1;
                    match chars[i] {
                        'n' => s.push('\n'),
                        't' => s.push('\t'),
                        'r' => s.push('\r'),
                        other => s.push(other),
                    }
                    i += 1;
                } else if ch == quote {
                    closed = true;
                    i += 1;
                    break;
                } else {
                    s.push(ch);
                    i += 1;
                }
            }
            if !closed {
                return Err(format!("Unterminated string literal in '{input}'"));
            }
            tokens.push(Token::Str(s));
            continue;
        }

        // Number literal
        if c.is_ascii_digit() {
            let start = i;
            let mut is_float = false;
            while i < len && chars[i].is_ascii_digit() {
                i += 1;
            }
            if i < len && chars[i] == '.' && (i + 1 < len && chars[i + 1].is_ascii_digit()) {
                is_float = true;
                i += 1;
                while i < len && chars[i].is_ascii_digit() {
                    i += 1;
                }
            }
            let num_str: String = chars[start..i].iter().collect();
            if is_float {
                let f = num_str
                    .parse::<f64>()
                    .map_err(|e| format!("Invalid float '{num_str}': {e}"))?;
                tokens.push(Token::Float(f));
            } else {
                let n = num_str
                    .parse::<i64>()
                    .map_err(|e| format!("Invalid int '{num_str}': {e}"))?;
                tokens.push(Token::Int(n));
            }
            continue;
        }

        // Two-character operators
        if i + 1 < len {
            match (c, chars[i + 1]) {
                ('|', '|') => {
                    tokens.push(Token::OrOr);
                    i += 2;
                    continue;
                }
                ('&', '&') => {
                    tokens.push(Token::AndAnd);
                    i += 2;
                    continue;
                }
                ('=', '=') => {
                    tokens.push(Token::EqEq);
                    i += 2;
                    continue;
                }
                ('!', '=') => {
                    tokens.push(Token::NotEq);
                    i += 2;
                    continue;
                }
                ('>', '=') => {
                    tokens.push(Token::Ge);
                    i += 2;
                    continue;
                }
                ('<', '=') => {
                    tokens.push(Token::Le);
                    i += 2;
                    continue;
                }
                _ => {}
            }
        }

        // Single-character punctuation/operators
        match c {
            '(' => {
                tokens.push(Token::LParen);
                i += 1;
                continue;
            }
            ')' => {
                tokens.push(Token::RParen);
                i += 1;
                continue;
            }
            ',' => {
                tokens.push(Token::Comma);
                i += 1;
                continue;
            }
            '?' => {
                tokens.push(Token::Question);
                i += 1;
                continue;
            }
            ':' => {
                tokens.push(Token::Colon);
                i += 1;
                continue;
            }
            '|' => {
                tokens.push(Token::Pipe);
                i += 1;
                continue;
            }
            '>' => {
                tokens.push(Token::Gt);
                i += 1;
                continue;
            }
            '<' => {
                tokens.push(Token::Lt);
                i += 1;
                continue;
            }
            '+' => {
                tokens.push(Token::Plus);
                i += 1;
                continue;
            }
            '-' => {
                tokens.push(Token::Minus);
                i += 1;
                continue;
            }
            '*' => {
                tokens.push(Token::Star);
                i += 1;
                continue;
            }
            '/' => {
                tokens.push(Token::Slash);
                i += 1;
                continue;
            }
            '%' => {
                tokens.push(Token::Percent);
                i += 1;
                continue;
            }
            '!' => {
                tokens.push(Token::Bang);
                i += 1;
                continue;
            }
            _ => {}
        }

        // Identifier or dot-path (including bracket indices like `items[0].name` or hyphens in keys)
        if c.is_ascii_alphabetic() || c == '_' || c == '$' {
            let start = i;
            while i < len {
                let ch = chars[i];
                if ch.is_ascii_alphanumeric()
                    || ch == '_'
                    || ch == '.'
                    || ch == '['
                    || ch == ']'
                    || ch == '$'
                {
                    i += 1;
                } else {
                    break;
                }
            }
            let ident: String = chars[start..i].iter().collect();
            match ident.as_str() {
                "true" => tokens.push(Token::Bool(true)),
                "false" => tokens.push(Token::Bool(false)),
                "null" | "nil" | "None" => tokens.push(Token::Null),
                _ => tokens.push(Token::Ident(ident)),
            }
            continue;
        }

        return Err(format!("Unexpected character '{c}' in expression '{input}'"));
    }

    Ok(tokens)
}

struct Parser<'a> {
    tokens: &'a [Token],
    pos: usize,
    store: &'a dyn ReactiveStore,
    local_scope: Option<&'a BTreeMap<String, StateValue>>,
}

impl<'a> Parser<'a> {
    fn new(
        tokens: &'a [Token],
        store: &'a dyn ReactiveStore,
        local_scope: Option<&'a BTreeMap<String, StateValue>>,
    ) -> Self {
        Self {
            tokens,
            pos: 0,
            store,
            local_scope,
        }
    }

    fn peek(&self) -> Option<&'a Token> {
        self.tokens.get(self.pos)
    }

    fn next(&mut self) -> Option<&'a Token> {
        let tok = self.tokens.get(self.pos);
        if tok.is_some() {
            self.pos += 1;
        }
        tok
    }

    // Precedence 1 (lowest): Pipe filters `expr | filter(args)`
    fn parse_pipe(&mut self) -> Result<StateValue, String> {
        let mut val = self.parse_ternary()?;
        while matches!(self.peek(), Some(Token::Pipe)) {
            self.next(); // consume `|`
            let filter_name = match self.next() {
                Some(Token::Ident(name)) => name.clone(),
                other => {
                    return Err(format!(
                        "Expected filter name after '|', found {:?}",
                        other
                    ))
                }
            };
            let mut args = Vec::new();
            if matches!(self.peek(), Some(Token::LParen)) {
                self.next(); // consume `(`
                if !matches!(self.peek(), Some(Token::RParen)) {
                    loop {
                        args.push(self.parse_ternary()?);
                        if matches!(self.peek(), Some(Token::Comma)) {
                            self.next();
                        } else {
                            break;
                        }
                    }
                }
                match self.next() {
                    Some(Token::RParen) => {}
                    other => {
                        return Err(format!(
                            "Expected ')' after filter arguments for '{filter_name}', found {:?}",
                            other
                        ))
                    }
                }
            }
            val = apply_filter(&filter_name, val, &args)?;
        }
        Ok(val)
    }

    // Precedence 2: Ternary `cond ? then_branch : else_branch`
    fn parse_ternary(&mut self) -> Result<StateValue, String> {
        let cond = self.parse_or()?;
        if matches!(self.peek(), Some(Token::Question)) {
            self.next(); // consume `?`
            let then_val = self.parse_pipe()?;
            match self.next() {
                Some(Token::Colon) => {}
                other => {
                    return Err(format!(
                        "Expected ':' in ternary expression, found {:?}",
                        other
                    ))
                }
            }
            let else_val = self.parse_pipe()?;
            if cond.as_bool() {
                Ok(then_val)
            } else {
                Ok(else_val)
            }
        } else {
            Ok(cond)
        }
    }

    // Precedence 3: Logical OR `||`
    fn parse_or(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_and()?;
        while matches!(self.peek(), Some(Token::OrOr)) {
            self.next();
            let right = self.parse_and()?;
            left = StateValue::Bool(left.as_bool() || right.as_bool());
        }
        Ok(left)
    }

    // Precedence 4: Logical AND `&&`
    fn parse_and(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_equality()?;
        while matches!(self.peek(), Some(Token::AndAnd)) {
            self.next();
            let right = self.parse_equality()?;
            left = StateValue::Bool(left.as_bool() && right.as_bool());
        }
        Ok(left)
    }

    // Precedence 5: Equality `==`, `!=`
    fn parse_equality(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_relational()?;
        while let Some(tok @ (Token::EqEq | Token::NotEq)) = self.peek() {
            let is_eq = matches!(tok, Token::EqEq);
            self.next();
            let right = self.parse_relational()?;
            let equal = values_equal(&left, &right);
            left = StateValue::Bool(if is_eq { equal } else { !equal });
        }
        Ok(left)
    }

    // Precedence 6: Relational `>`, `<`, `>=`, `<=`
    fn parse_relational(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_additive()?;
        while let Some(tok @ (Token::Gt | Token::Lt | Token::Ge | Token::Le)) = self.peek() {
            let op = tok.clone();
            self.next();
            let right = self.parse_additive()?;
            let ord = compare_values(&left, &right);
            let res = match op {
                Token::Gt => ord.is_some_and(|o| o.is_gt()),
                Token::Lt => ord.is_some_and(|o| o.is_lt()),
                Token::Ge => ord.is_some_and(|o| o.is_ge()),
                Token::Le => ord.is_some_and(|o| o.is_le()),
                _ => unreachable!(),
            };
            left = StateValue::Bool(res);
        }
        Ok(left)
    }

    // Precedence 7: Additive `+`, `-`
    fn parse_additive(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_multiplicative()?;
        while let Some(tok @ (Token::Plus | Token::Minus)) = self.peek() {
            let is_plus = matches!(tok, Token::Plus);
            self.next();
            let right = self.parse_multiplicative()?;
            left = if is_plus {
                eval_add(left, right)
            } else {
                eval_sub(left, right)
            };
        }
        Ok(left)
    }

    // Precedence 8: Multiplicative `*`, `/`, `%`
    fn parse_multiplicative(&mut self) -> Result<StateValue, String> {
        let mut left = self.parse_unary()?;
        while let Some(tok @ (Token::Star | Token::Slash | Token::Percent)) = self.peek() {
            let op = tok.clone();
            self.next();
            let right = self.parse_unary()?;
            left = match op {
                Token::Star => eval_mul(left, right),
                Token::Slash => eval_div(left, right),
                Token::Percent => eval_rem(left, right),
                _ => unreachable!(),
            };
        }
        Ok(left)
    }

    // Precedence 9: Unary `!`, `-`, `+`
    fn parse_unary(&mut self) -> Result<StateValue, String> {
        match self.peek() {
            Some(Token::Bang) => {
                self.next();
                let v = self.parse_unary()?;
                Ok(StateValue::Bool(!v.as_bool()))
            }
            Some(Token::Minus) => {
                self.next();
                let v = self.parse_unary()?;
                match v {
                    StateValue::Int(i) => Ok(StateValue::Int(-i)),
                    StateValue::Float(f) => Ok(StateValue::Float(-f)),
                    other => Ok(StateValue::Float(-other.as_f64().unwrap_or(0.0))),
                }
            }
            Some(Token::Plus) => {
                self.next();
                let v = self.parse_unary()?;
                match v {
                    StateValue::Int(i) => Ok(StateValue::Int(i)),
                    StateValue::Float(f) => Ok(StateValue::Float(f)),
                    other => Ok(StateValue::Float(other.as_f64().unwrap_or(0.0))),
                }
            }
            _ => self.parse_primary(),
        }
    }

    // Precedence 10: Primary literals, dot-paths, and parenthesized expressions
    fn parse_primary(&mut self) -> Result<StateValue, String> {
        match self.next() {
            Some(Token::Str(s)) => Ok(StateValue::String(s.clone())),
            Some(Token::Int(i)) => Ok(StateValue::Int(*i)),
            Some(Token::Float(f)) => Ok(StateValue::Float(*f)),
            Some(Token::Bool(b)) => Ok(StateValue::Bool(*b)),
            Some(Token::Null) => Ok(StateValue::Null),
            Some(Token::Ident(path)) => Ok(self.resolve_path(path)),
            Some(Token::LParen) => {
                let val = self.parse_pipe()?;
                match self.next() {
                    Some(Token::RParen) => Ok(val),
                    other => Err(format!("Expected closing ')', found {:?}", other)),
                }
            }
            other => Err(format!(
                "Unexpected token {:?} while parsing primary expression",
                other
            )),
        }
    }

    fn resolve_path(&self, raw_path: &str) -> StateValue {
        let norm = normalize_path(raw_path);
        if norm.is_empty() {
            return StateValue::Null;
        }

        // 1. Check local_scope first
        if let Some(scope) = self.local_scope {
            if let Some(exact) = scope.get(&norm) {
                return exact.clone();
            }
            let mut parts = norm.split('.');
            if let Some(first) = parts.next() {
                if let Some(root_val) = scope.get(first) {
                    let rest: Vec<&str> = parts.collect();
                    if rest.is_empty() {
                        return root_val.clone();
                    }
                    return lookup_in_value(root_val, &rest.join(".")).unwrap_or(StateValue::Null);
                }
            }
        }

        // 2. Check global StateStore
        if let Some(val) = self.store.get(&norm) {
            return val;
        }

        // Support `.len` / `.length` on paths in arbitrary ReactiveStore implementations
        if let Some((parent, last)) = norm.rsplit_once('.') {
            if last == "len" || last == "length" {
                if let Some(parent_val) = self.store.get(parent) {
                    if let Some(len_val) = value_length(&parent_val) {
                        return len_val;
                    }
                }
            }
        }

        StateValue::Null
    }
}

fn values_equal(a: &StateValue, b: &StateValue) -> bool {
    match (a, b) {
        (StateValue::Int(i), StateValue::Float(f)) | (StateValue::Float(f), StateValue::Int(i)) => {
            (*i as f64 - *f).abs() < 1e-9
        }
        (StateValue::Float(f1), StateValue::Float(f2)) => (*f1 - *f2).abs() < 1e-9,
        _ => a == b,
    }
}

fn compare_values(a: &StateValue, b: &StateValue) -> Option<std::cmp::Ordering> {
    match (a, b) {
        (StateValue::Int(i1), StateValue::Int(i2)) => i1.partial_cmp(i2),
        (StateValue::String(s1), StateValue::String(s2)) => s1.partial_cmp(s2),
        _ => {
            let f1 = a.as_f64()?;
            let f2 = b.as_f64()?;
            f1.partial_cmp(&f2)
        }
    }
}

fn eval_add(a: StateValue, b: StateValue) -> StateValue {
    match (&a, &b) {
        (StateValue::String(_), _) | (_, StateValue::String(_)) => {
            StateValue::String(format!("{}{}", a.to_display_string(), b.to_display_string()))
        }
        (StateValue::Int(i1), StateValue::Int(i2)) => StateValue::Int(i1.saturating_add(*i2)),
        _ => {
            let f1 = a.as_f64().unwrap_or(0.0);
            let f2 = b.as_f64().unwrap_or(0.0);
            StateValue::Float(f1 + f2)
        }
    }
}

fn eval_sub(a: StateValue, b: StateValue) -> StateValue {
    match (&a, &b) {
        (StateValue::Int(i1), StateValue::Int(i2)) => StateValue::Int(i1.saturating_sub(*i2)),
        _ => {
            let f1 = a.as_f64().unwrap_or(0.0);
            let f2 = b.as_f64().unwrap_or(0.0);
            StateValue::Float(f1 - f2)
        }
    }
}

fn eval_mul(a: StateValue, b: StateValue) -> StateValue {
    match (&a, &b) {
        (StateValue::Int(i1), StateValue::Int(i2)) => StateValue::Int(i1.saturating_mul(*i2)),
        _ => {
            let f1 = a.as_f64().unwrap_or(0.0);
            let f2 = b.as_f64().unwrap_or(0.0);
            StateValue::Float(f1 * f2)
        }
    }
}

fn eval_div(a: StateValue, b: StateValue) -> StateValue {
    match (&a, &b) {
        (StateValue::Int(i1), StateValue::Int(i2)) => {
            if *i2 == 0 {
                StateValue::Int(0)
            } else if *i1 % *i2 == 0 {
                StateValue::Int(*i1 / *i2)
            } else {
                StateValue::Float(*i1 as f64 / *i2 as f64)
            }
        }
        _ => {
            let f1 = a.as_f64().unwrap_or(0.0);
            let f2 = b.as_f64().unwrap_or(0.0);
            if f2.abs() < 1e-12 {
                StateValue::Float(0.0)
            } else {
                StateValue::Float(f1 / f2)
            }
        }
    }
}

fn eval_rem(a: StateValue, b: StateValue) -> StateValue {
    match (&a, &b) {
        (StateValue::Int(i1), StateValue::Int(i2)) => {
            if *i2 == 0 {
                StateValue::Int(0)
            } else {
                StateValue::Int(*i1 % *i2)
            }
        }
        _ => {
            let f1 = a.as_f64().unwrap_or(0.0);
            let f2 = b.as_f64().unwrap_or(0.0);
            if f2.abs() < 1e-12 {
                StateValue::Float(0.0)
            } else {
                StateValue::Float(f1 % f2)
            }
        }
    }
}

fn apply_filter(name: &str, input: StateValue, args: &[StateValue]) -> Result<StateValue, String> {
    match name {
        "default" => {
            let fallback = args
                .first()
                .cloned()
                .unwrap_or_else(|| StateValue::String(String::new()));
            let is_empty = match &input {
                StateValue::Null => true,
                StateValue::String(s) => s.is_empty(),
                _ => false,
            };
            if is_empty {
                Ok(fallback)
            } else {
                Ok(input)
            }
        }
        "truncate" => {
            let max_chars = args
                .first()
                .and_then(|v| v.as_i64())
                .unwrap_or(24)
                .max(0) as usize;
            let ellipsis = args
                .get(1)
                .map(|v| v.to_display_string())
                .unwrap_or_else(|| "…".to_string());
            let s = input.to_display_string();
            let char_count = s.chars().count();
            if char_count > max_chars {
                let truncated: String = s.chars().take(max_chars).collect();
                Ok(StateValue::String(format!("{truncated}{ellipsis}")))
            } else {
                Ok(StateValue::String(s))
            }
        }
        "round" => {
            let decimals = args.first().and_then(|v| v.as_i64()).unwrap_or(0);
            let num = input.as_f64().unwrap_or(0.0);
            if decimals <= 0 {
                Ok(StateValue::Int(num.round() as i64))
            } else {
                let factor = 10_f64.powi(decimals as i32);
                let rounded = (num * factor).round() / factor;
                Ok(StateValue::Float(rounded))
            }
        }
        "upper" => Ok(StateValue::String(
            input.to_display_string().to_uppercase(),
        )),
        "lower" => Ok(StateValue::String(
            input.to_display_string().to_lowercase(),
        )),
        "percent" => Ok(StateValue::String(format!(
            "{}%",
            input.to_display_string()
        ))),
        other => Err(format!("Unknown pipe filter '{other}'")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hierarchical_get_set_and_merge() {
        let mut store = StateStore::new();
        let changed = store.set("sys.cpu.usage_pct", StateValue::Float(42.5));
        assert_eq!(changed, vec![StatePath::new("sys.cpu.usage_pct")]);
        assert_eq!(
            store.get("sys.cpu.usage_pct"),
            Some(StateValue::Float(42.5))
        );

        let mut fields = BTreeMap::new();
        fields.insert("cores".to_string(), StateValue::Int(8));
        fields.insert("temp_c".to_string(), StateValue::Float(55.0));
        let merged = store.apply_patch(StatePatch::Merge {
            path: "sys.cpu".to_string(),
            fields,
        });
        assert!(merged.contains(&StatePath::new("sys.cpu.cores")));
        assert!(merged.contains(&StatePath::new("sys.cpu.temp_c")));
        assert_eq!(store.get("sys.cpu.cores"), Some(StateValue::Int(8)));
        assert_eq!(store.get("sys.cpu.usage_pct"), Some(StateValue::Float(42.5)));
    }

    #[test]
    fn test_list_upsert_and_remove() {
        let mut store = StateStore::new();

        let make_ws = |id: i64, name: &str, active: bool| {
            let mut m = BTreeMap::new();
            m.insert("id".to_string(), StateValue::Int(id));
            m.insert("name".to_string(), StateValue::String(name.to_string()));
            m.insert("active".to_string(), StateValue::Bool(active));
            StateValue::Map(m)
        };

        store.apply_patch(StatePatch::ListUpsert {
            path: "wm.workspaces".to_string(),
            key_field: "id".to_string(),
            item: make_ws(1, "term", true),
        });
        store.apply_patch(StatePatch::ListUpsert {
            path: "wm.workspaces".to_string(),
            key_field: "id".to_string(),
            item: make_ws(2, "web", false),
        });

        assert_eq!(
            store.get("wm.workspaces.0.name"),
            Some(StateValue::String("term".to_string()))
        );
        assert_eq!(
            store.get("wm.workspaces.1.name"),
            Some(StateValue::String("web".to_string()))
        );
        assert_eq!(store.get("wm.workspaces.len"), Some(StateValue::Int(2)));

        // Upsert existing id=2
        store.apply_patch(StatePatch::ListUpsert {
            path: "wm.workspaces".to_string(),
            key_field: "id".to_string(),
            item: make_ws(2, "browser", true),
        });
        assert_eq!(store.get("wm.workspaces.len"), Some(StateValue::Int(2)));
        assert_eq!(
            store.get("wm.workspaces.1.name"),
            Some(StateValue::String("browser".to_string()))
        );
        assert_eq!(
            store.get("wm.workspaces.1.active"),
            Some(StateValue::Bool(true))
        );

        // Remove id=1
        store.apply_patch(StatePatch::ListRemove {
            path: "wm.workspaces".to_string(),
            key_field: "id".to_string(),
            key_value: StateValue::Int(1),
        });
        assert_eq!(store.get("wm.workspaces.len"), Some(StateValue::Int(1)));
        assert_eq!(
            store.get("wm.workspaces.0.name"),
            Some(StateValue::String("browser".to_string()))
        );
    }

    #[test]
    fn test_subscriptions_trigger_on_overlapping_paths() {
        let mut store = StateStore::new();
        store.subscribe("sys.cpu", 10);
        store.subscribe("sys.cpu.usage_pct", 20);
        store.subscribe("sys.ram", 30);

        let changed = store.set("sys.cpu.usage_pct", StateValue::Float(75.0));
        let triggered = store.triggered_subscriptions(&changed);
        assert_eq!(triggered, vec![10, 20]);

        let changed_parent = store.set("sys.cpu", StateValue::Map(BTreeMap::new()));
        let triggered_parent = store.triggered_subscriptions(&changed_parent);
        assert_eq!(triggered_parent, vec![10, 20]);
    }

    #[test]
    fn test_expression_arithmetic_logic_and_ternary() {
        let mut store = StateStore::new();
        store.set("sys.cpu.usage_pct", StateValue::Float(85.4));
        store.set("sys.battery.charging", StateValue::Bool(true));
        store.set("sys.battery.capacity", StateValue::Int(40));

        assert_eq!(
            store.evaluate_expr("10 + 5 * 2 - 4 / 2", None).unwrap(),
            StateValue::Int(18)
        );
        assert_eq!(
            store.evaluate_expr("17 % 5", None).unwrap(),
            StateValue::Int(2)
        );
        assert_eq!(
            store
                .evaluate_expr("sys.cpu.usage_pct >= 80 && !false", None)
                .unwrap(),
            StateValue::Bool(true)
        );
        assert_eq!(
            store
                .evaluate_expr(
                    "sys.battery.charging ? 'Charging' : sys.battery.capacity < 20 ? 'Low' : 'Discharging'",
                    None
                )
                .unwrap(),
            StateValue::String("Charging".to_string())
        );
    }

    #[test]
    fn test_local_scope_and_pipe_filters() {
        let mut store = StateStore::new();
        store.set("wm.focused_window.title", StateValue::String(String::new()));
        store.set("sys.cpu.usage_pct", StateValue::Float(37.68));

        let mut ws_map = BTreeMap::new();
        ws_map.insert("idx".to_string(), StateValue::Int(3));
        ws_map.insert(
            "name".to_string(),
            StateValue::String("development-environment-workspace".to_string()),
        );
        let mut local = BTreeMap::new();
        local.insert("ws".to_string(), StateValue::Map(ws_map));

        // default filter on empty string or missing path
        assert_eq!(
            store
                .evaluate_expr("wm.focused_window.title | default('Desktop')", None)
                .unwrap(),
            StateValue::String("Desktop".to_string())
        );
        assert_eq!(
            store
                .evaluate_expr("wm.nonexistent.field | default('Fallback')", None)
                .unwrap(),
            StateValue::String("Fallback".to_string())
        );

        // truncate filter
        assert_eq!(
            store
                .evaluate_expr("ws.name | truncate(11) | upper", Some(&local))
                .unwrap(),
            StateValue::String("DEVELOPMENT…".to_string())
        );

        // round and percent filters
        assert_eq!(
            store
                .evaluate_expr("sys.cpu.usage_pct | round(1) | percent", None)
                .unwrap(),
            StateValue::String("37.7%".to_string())
        );
        assert_eq!(
            store
                .evaluate_expr("'MIXED Case' | lower", None)
                .unwrap(),
            StateValue::String("mixed case".to_string())
        );
    }

    #[test]
    fn test_string_interpolation() {
        let mut store = StateStore::new();
        store.set("sys.cpu.usage_pct", StateValue::Float(64.24));
        store.set("sys.ram.used_gb", StateValue::Float(7.8));
        store.set("wm.focused_window.title", StateValue::Null);

        let rendered = store
            .interpolate_string(
                "{{ wm.focused_window.title | default('Desktop') }} - CPU {{ sys.cpu.usage_pct | round(1) | percent }} / RAM {{ sys.ram.used_gb }}G",
                None,
            )
            .unwrap();
        assert_eq!(rendered, "Desktop - CPU 64.2% / RAM 7.8G");
    }
}
