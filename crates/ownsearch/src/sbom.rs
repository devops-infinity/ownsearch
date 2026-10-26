use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Context;
use serde_json::{Value, json};

pub(crate) fn run(args: &crate::cli::SbomArgs) -> anyhow::Result<()> {
    let lock = args.lock.clone().unwrap_or_else(find_lock);
    let document = build(&lock)
        .with_context(|| format!("building a bill of materials from {}", lock.display()))?;
    let mut body = serde_json::to_vec_pretty(&document)?;
    body.push(b'\n');
    match &args.out {
        Some(path) => {
            ownsearch_core::write_atomic(path, &body)
                .with_context(|| format!("writing {}", path.display()))?;
            println!("{}", path.display());
            Ok(())
        }
        None => crate::write_output(std::io::stdout().lock(), &body),
    }
}

fn find_lock() -> PathBuf {
    let mut current = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    loop {
        let candidate = current.join("Cargo.lock");
        if candidate.is_file() {
            return candidate;
        }
        if !current.pop() {
            return PathBuf::from("Cargo.lock");
        }
    }
}

fn build(lock: &Path) -> anyhow::Result<Value> {
    let raw = std::fs::read_to_string(lock)?;
    let parsed: toml::Value = toml::from_str(&raw)?;
    let packages = parsed
        .get("package")
        .and_then(toml::Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut versions: BTreeMap<String, String> = BTreeMap::new();
    for package in &packages {
        if let (Some(name), Some(version)) = (
            package.get("name").and_then(toml::Value::as_str),
            package.get("version").and_then(toml::Value::as_str),
        ) {
            versions
                .entry(name.to_owned())
                .or_insert_with(|| version.to_owned());
        }
    }
    let root = purl(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION"));
    let mut components: Vec<Value> = Vec::new();
    let mut depends: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let mut root_deps: BTreeSet<String> = BTreeSet::new();
    for package in &packages {
        let Some(name) = package.get("name").and_then(toml::Value::as_str) else {
            continue;
        };
        let Some(version) = package.get("version").and_then(toml::Value::as_str) else {
            continue;
        };
        let reference = purl(name, version);
        let is_root = reference == root;
        if !is_root {
            components.push(json!({
                "type": "library",
                "bom-ref": reference,
                "name": name,
                "version": version,
                "purl": reference,
            }));
        }
        let mut own: BTreeSet<String> = BTreeSet::new();
        for entry in package
            .get("dependencies")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default()
        {
            let Some(text) = entry.as_str() else {
                continue;
            };
            let Some(dependency) = dependency_ref(text, &versions) else {
                continue;
            };
            own.insert(dependency);
        }
        if is_root {
            root_deps = own;
        } else {
            depends.insert(reference, own);
        }
    }
    let mut dependencies: Vec<Value> = Vec::new();
    for (reference, own) in &depends {
        dependencies.push(json!({
            "ref": reference,
            "dependsOn": own.iter().cloned().collect::<Vec<String>>(),
        }));
    }
    dependencies.push(json!({
        "ref": root,
        "dependsOn": root_deps.into_iter().collect::<Vec<String>>(),
    }));
    let component_version = env!("CARGO_PKG_VERSION");
    components.sort_by(|left, right| {
        let key = |value: &Value| {
            (
                value
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
                value
                    .get("version")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned(),
            )
        };
        key(left).cmp(&key(right))
    });
    Ok(json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.7",
        "version": 1,
        "metadata": {
            "timestamp": ownsearch_core::now_rfc3339(),
            "tools": [{
                "type": "application",
                "name": "ownsearch",
                "version": component_version,
            }],
            "component": {
                "type": "application",
                "bom-ref": root,
                "name": "ownsearch",
                "version": component_version,
                "purl": root,
            },
        },
        "components": components,
        "dependencies": dependencies,
    }))
}

fn purl(name: &str, version: &str) -> String {
    let encode = |text: &str| {
        text.chars()
            .map(|character| match character {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '.' | '_' | '~' | '/' | ':' | '@' => {
                    character.to_string()
                }
                other => format!("%{:02X}", other as u32),
            })
            .collect::<String>()
    };
    format!("pkg:cargo/{}@{}", encode(name), encode(version))
}

fn dependency_ref(text: &str, versions: &BTreeMap<String, String>) -> Option<String> {
    let mut parts = text.split_whitespace();
    let name = parts.next()?;
    match parts.next() {
        Some(version) => Some(purl(name, version)),
        None => versions.get(name).map(|version| purl(name, version)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_reference_keeps_the_cargo_shape() {
        assert_eq!(purl("serde_json", "1.0.0"), "pkg:cargo/serde_json@1.0.0");
    }

    #[test]
    fn a_dependency_line_takes_its_version_from_the_lock_when_missing() {
        let mut versions = BTreeMap::new();
        versions.insert("serde".to_owned(), "1.0.0".to_owned());
        assert_eq!(
            dependency_ref("serde", &versions).as_deref(),
            Some("pkg:cargo/serde@1.0.0")
        );
        assert_eq!(
            dependency_ref("anyhow 1.0.95", &versions).as_deref(),
            Some("pkg:cargo/anyhow@1.0.95")
        );
        assert_eq!(dependency_ref("missing-crate", &versions), None);
    }

    #[test]
    fn every_dependency_reference_resolves_to_a_declared_component() {
        let lock = find_lock();
        let document = build(&lock).expect("document");
        let mut known: BTreeSet<String> = document
            .get("components")
            .and_then(Value::as_array)
            .map(|components| {
                components
                    .iter()
                    .filter_map(|item| item.get("bom-ref").and_then(Value::as_str))
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        let root = document
            .get("metadata")
            .and_then(|metadata| metadata.get("component"))
            .and_then(|component| component.get("bom-ref"))
            .and_then(Value::as_str)
            .expect("root ref")
            .to_owned();
        known.insert(root.clone());
        assert!(known.contains(&format!(
            "pkg:cargo/ownsearch-core@{}",
            env!("CARGO_PKG_VERSION")
        )));
        let dependencies = document
            .get("dependencies")
            .and_then(Value::as_array)
            .expect("dependencies");
        for entry in dependencies {
            let reference = entry.get("ref").and_then(Value::as_str).unwrap_or_default();
            assert!(known.contains(reference), "{reference} is not a component");
            for target in entry
                .get("dependsOn")
                .and_then(Value::as_array)
                .map(Vec::as_slice)
                .unwrap_or_default()
            {
                let target = target.as_str().unwrap_or_default();
                assert!(known.contains(target), "{target} is not a component");
            }
        }
    }

    #[test]
    fn the_lock_of_this_workspace_produces_a_complete_document() {
        let lock = find_lock();
        let document = build(&lock).expect("document");
        assert_eq!(
            document.get("specVersion").and_then(Value::as_str),
            Some("1.7")
        );
        let components = document
            .get("components")
            .and_then(Value::as_array)
            .expect("components");
        assert!(components.len() > 10);
        assert!(
            components
                .iter()
                .any(|item| item.get("name").and_then(Value::as_str) == Some("serde"))
        );
    }
}
