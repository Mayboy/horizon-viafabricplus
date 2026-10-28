//! Which protocol version to join a server with (`horizon-multiversion`):
//! a version saved for the server's address, or "auto", which asks the
//! server for its version first (ViaFabricPlus' auto detect) and translates
//! when it is one Horizon can join. Choices live in horizon.txt
//! (`protocolVersion.<address>`), so servers.dat stays vanilla's.
use horizon_protocol::translate::Shared;
use std::time::Duration;

pub const AUTO: &str = "auto";

fn key(address: &str) -> String {
    // Preferences split at the first colon: keep the port out of the key's.
    format!("protocolVersion.{}", address.trim().replace(':', "_"))
}

/// The choice saved for `address`: "auto" or a version name.
pub fn saved(address: &str) -> String {
    std::env::current_dir()
        .ok()
        .and_then(|dir| crate::ui_theme::preference(&dir, &key(address)))
        .filter(|v| v == AUTO || horizon_multiversion::by_name(v).is_some())
        .unwrap_or_else(|| AUTO.to_owned())
}
pub fn save(address: &str, choice: &str) {
    if saved(address) == choice {
        return;
    }
    if let Ok(dir) = std::env::current_dir() {
        let value = (choice != AUTO).then_some(choice);
        crate::ui_theme::set_preference(&dir, &key(address), value);
    }
}
/// The choice after `choice`: auto, then every version newest first.
pub fn next(choice: &str) -> String {
    let names: Vec<&str> = std::iter::once(AUTO)
        .chain(horizon_multiversion::VERSIONS.iter().map(|v| v.name))
        .collect();
    let i = names.iter().position(|n| *n == choice).unwrap_or(0);
    names[(i + 1) % names.len()].to_owned()
}
/// The version button's label.
pub fn label(choice: &str) -> String {
    let value = if choice == AUTO { "Auto" } else { choice };
    format!("Protocol: {value}")
}

/// The translator to join `address` with, and the version it targets
/// (None: this client's own version).
pub fn target(address: &str) -> Option<(Shared, &'static str)> {
    let choice = saved(address);
    let version = if choice == AUTO {
        detect(address)?
    } else {
        horizon_multiversion::by_name(&choice)?
    };
    let translator = horizon_multiversion::translator(version.protocol)?;
    Some((translator, version.name))
}

/// The server's version from its status, if Horizon can join it.
fn detect(address: &str) -> Option<horizon_multiversion::Version> {
    let (json, _) = crate::net::ping(address, Duration::from_secs(5)).ok()?;
    let protocol = i32::try_from(json["version"]["protocol"].as_i64()?).ok()?;
    horizon_multiversion::version(protocol)
}
