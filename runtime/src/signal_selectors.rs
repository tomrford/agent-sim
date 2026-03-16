use crate::sim::project::Project;
use crate::sim::types::SignalType;
use globset::{Glob, GlobMatcher};
use std::collections::{BTreeSet, HashMap};

pub type SelectorError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone)]
pub struct EnvSignalCatalogEntry {
    pub instance: String,
    pub local_id: u32,
    pub signal_name: String,
    pub qualified_name: String,
    pub signal_type: SignalType,
    pub units: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct EnvSignalCatalog {
    entries: Vec<EnvSignalCatalogEntry>,
    qualified_index: HashMap<String, usize>,
}

impl EnvSignalCatalog {
    pub fn build(
        entries: impl IntoIterator<Item = EnvSignalCatalogEntry>,
    ) -> Result<Self, String> {
        let mut sorted = entries.into_iter().collect::<Vec<_>>();
        sorted.sort_by(|lhs, rhs| lhs.qualified_name.cmp(&rhs.qualified_name));

        let mut qualified_index = HashMap::with_capacity(sorted.len());
        for (idx, entry) in sorted.iter().enumerate() {
            if entry.signal_name.contains(':') {
                return Err(format!(
                    "signal '{}' in instance '{}' contains reserved ':'",
                    entry.signal_name, entry.instance
                ));
            }
            if !entry.qualified_name.starts_with(&entry.instance)
                || !entry.qualified_name[entry.instance.len()..].starts_with(':')
            {
                return Err(format!(
                    "invalid qualified signal name '{}' for instance '{}'",
                    entry.qualified_name, entry.instance
                ));
            }
            if let Some(existing) = qualified_index.insert(entry.qualified_name.clone(), idx) {
                return Err(format!(
                    "duplicate env qualified signal '{}'; entries {} and {} collide",
                    entry.qualified_name, existing, idx
                ));
            }
        }

        Ok(Self {
            entries: sorted,
            qualified_index,
        })
    }

    pub fn entries(&self) -> &[EnvSignalCatalogEntry] {
        &self.entries
    }

    pub fn resolve_selector_indices(&self, selector: &str) -> Result<Vec<usize>, String> {
        if selector == "*" {
            return Ok((0..self.entries.len()).collect());
        }

        let Some((instance_selector, signal_selector)) = selector.split_once(':') else {
            return Err(format!(
                "invalid env selector '{selector}'; expected '<instance>:<signal>' or '*'"
            ));
        };
        if instance_selector.is_empty() || signal_selector.is_empty() {
            return Err(format!(
                "invalid env selector '{selector}'; expected '<instance>:<signal>'"
            ));
        }
        if signal_selector.starts_with('#') {
            return Err(format!(
                "env selector '{selector}' cannot use local signal ids; use qualified signal names"
            ));
        }

        if !contains_glob(instance_selector) && !contains_glob(signal_selector) {
            let qualified = format!("{instance_selector}:{signal_selector}");
            if let Some(index) = self.qualified_index.get(&qualified) {
                return Ok(vec![*index]);
            }
            return Err(format!("env signal not found: '{selector}'"));
        }

        let instance_glob = if contains_glob(instance_selector) {
            Some(
                compile_glob(instance_selector)
                    .map_err(|err| format!("invalid env selector '{selector}': {err}"))?,
            )
        } else {
            None
        };
        let signal_glob = if contains_glob(signal_selector) {
            Some(
                compile_glob(signal_selector)
                    .map_err(|err| format!("invalid env selector '{selector}': {err}"))?,
            )
        } else {
            None
        };

        let mut out = Vec::new();
        for (idx, entry) in self.entries.iter().enumerate() {
            if !string_matches(&entry.instance, instance_selector, instance_glob.as_ref()) {
                continue;
            }
            if !string_matches(&entry.signal_name, signal_selector, signal_glob.as_ref()) {
                continue;
            }
            out.push(idx);
        }
        if out.is_empty() {
            return Err(format!("env selector matched nothing: '{selector}'"));
        }
        Ok(out)
    }

    pub fn resolve_selectors(&self, selectors: &[String]) -> Result<Vec<usize>, String> {
        if selectors.is_empty() {
            return Err("missing env signal selectors".to_string());
        }
        let mut resolved = Vec::new();
        for selector in selectors {
            resolved.extend(self.resolve_selector_indices(selector)?);
        }
        Ok(resolved)
    }
}

pub fn select_instance_signal_ids(
    project: &Project,
    selectors: &[String],
) -> Result<Vec<u32>, SelectorError> {
    if selectors.is_empty() {
        return Err("missing signal selectors".into());
    }
    let mut ids = BTreeSet::new();
    for selector in selectors {
        if selector == "*" {
            ids.extend(project.signals().iter().map(|s| s.id));
            continue;
        }
        if let Some(raw_id) = selector.strip_prefix('#') {
            let id = raw_id.parse::<u32>()?;
            if project.signal_by_id(id).is_none() {
                return Err(format!("signal not found: '#{id}'").into());
            }
            ids.insert(id);
            continue;
        }
        if contains_glob(selector) {
            let matcher = compile_glob(selector)?;
            let mut matched = false;
            for signal in project.signals() {
                if matcher.is_match(&signal.name) {
                    ids.insert(signal.id);
                    matched = true;
                }
            }
            if !matched {
                return Err(format!("signal glob matched nothing: '{selector}'").into());
            }
            continue;
        }

        if let Some(id) = project.signal_id_by_name(selector) {
            ids.insert(id);
        } else {
            return Err(format!("signal not found: '{selector}'").into());
        }
    }
    Ok(ids.into_iter().collect())
}

fn compile_glob(pattern: &str) -> Result<GlobMatcher, SelectorError> {
    Ok(Glob::new(pattern)?.compile_matcher())
}

fn contains_glob(value: &str) -> bool {
    value.contains('*') || value.contains('?') || value.contains('[')
}

fn string_matches(value: &str, selector: &str, glob: Option<&GlobMatcher>) -> bool {
    if let Some(glob) = glob {
        glob.is_match(value)
    } else {
        value == selector
    }
}

