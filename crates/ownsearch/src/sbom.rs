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
        let local = is_workspace_member(package);
        if !local {
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
            let Some(dependency) = dependency_ref(text) else {
                continue;
            };
            own.insert(dependency.clone());
            if local {
                root_deps.insert(dependency);
            }
        }
        depends.insert(reference, own);
    }
    let mut dependencies: Vec<Value> = Vec::new();
    for (reference, own) in &depends {
        dependencies.push(json!({
            "ref": reference,
            "dependsOn": own.iter().cloned().collect::<Vec<String>>(),
        }));
    }
    let root = purl("ownsearch", env!("CARGO_PKG_VERSION"));
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

fn is_workspace_member(package: &toml::Value) -> bool {
    package
        .get("source")
        .and_then(toml::Value::as_str)
        .is_none()
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

fn dependency_ref(text: &str) -> Option<String> {
    let mut parts = text.split_whitespace();
    let name = parts.next()?;
    let version = parts.next();
    match version {
        Some(version) => Some(purl(name, version)),
        None => Some(format!("pkg:cargo/{name}")),
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
    fn a_dependency_line_without_a_version_keeps_the_name() {
        assert_eq!(dependency_ref("serde").as_deref(), Some("pkg:cargo/serde"));
        assert_eq!(
            dependency_ref("anyhow 1.0.95").as_deref(),
            Some("pkg:cargo/anyhow@1.0.95")
        );
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
