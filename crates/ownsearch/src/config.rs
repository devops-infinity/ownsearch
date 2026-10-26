use std::io::{IsTerminal, Read};

use anyhow::Context;
use ownsearch_core::{Settings, keychain_delete, keychain_set};
use secrecy::SecretString;
use serde_json::json;
use zeroize::Zeroizing;

use crate::cli::{ConfigAction, ConfigArgs};

pub(crate) fn run(args: ConfigArgs) -> anyhow::Result<()> {
    let settings = Settings::load(args.common.overrides()).context("resolving settings")?;
    match args.action {
        ConfigAction::Show { json: as_json } => {
            let rows = settings.describe();
            if as_json {
                let mut map = serde_json::Map::new();
                for (key, value) in rows {
                    map.insert(key, json!(value));
                }
                println!("{}", serde_json::to_string_pretty(&map)?);
            } else {
                for (key, value) in rows {
                    println!("{key}: {value}");
                }
            }
            Ok(())
        }
        ConfigAction::SetBearerToken => {
            let mut raw = Zeroizing::new(String::new());
            if std::io::stdin().is_terminal() {
                eprint!("bearer token (input hidden): ");
                let line = rpassword::read_password().context("reading the token")?;
                raw.push_str(&line);
            } else {
                std::io::stdin()
                    .read_to_string(&mut raw)
                    .context("reading the token from stdin")?;
            }
            let token = Zeroizing::new(raw.trim().to_owned());
            if token.chars().count() < 32 {
                anyhow::bail!("a bearer token needs at least 32 characters");
            }
            if token.contains(',') {
                anyhow::bail!(
                    "a bearer token cannot contain a comma; the stored list is comma-separated"
                );
            }
            let secret = SecretString::from(token.as_str().to_owned());
            keychain_set(&secret).context("storing the token in the OS keychain")?;
            println!("the bearer token list is stored in the OS keychain");
            Ok(())
        }
        ConfigAction::DeleteBearerToken => {
            keychain_delete().context("removing the token from the OS keychain")?;
            println!("the stored bearer token list was removed");
            Ok(())
        }
    }
}
