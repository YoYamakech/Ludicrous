// updater: check GitHub for a newer release, download it, verify it, swap it in.
//
// No winit, no GL, no async runtime: this module is plain blocking code that
// always runs on a background thread and reports through a closure the app
// builds from its `EventLoopProxy` (v1.2 section 9). Every network entry point
// here is called from a worker, never from the event loop.
//
// Safety rules that are not negotiable:
//   - HTTPS only, and only GitHub hosts, for both the API and the asset.
//   - The download must match `SHA256SUMS` from the same release, or it is
//     deleted and the update fails closed.
//   - A directory we cannot write means "package-managed install": offer the
//     release page instead of trying to replace ourselves.
//   - Nothing here reports an error for a failed *check*: being offline is
//     normal and must stay silent.

#![cfg(feature = "updater")]

pub mod check;
#[cfg(feature = "updater")]
pub mod download;

/// The repository releases come from. Confirmed with the project owner.
pub const UPDATE_REPO: &str = "YoYamakech/Ludicrous";

/// The release page, used when we cannot or must not self-update.
pub fn releases_url() -> String {
    format!("https://github.com/{UPDATE_REPO}/releases")
}

/// Asset name convention (v1.2 section 9). The version has no leading `v`.
pub fn asset_name(version: &str) -> String {
    format!("ludicrous-{version}-x86_64-linux")
}

/// What the UI is told about the update process. Explicit states so the status
/// bar and the Settings row cannot disagree.
#[derive(Clone, Debug, PartialEq)]
pub enum UpdateEvent {
    /// A newer release exists. `notes_url` is the release page.
    Available { version: String, notes_url: String },
    /// The check finished and there is nothing to do. Never an error.
    UpToDate,
    /// Download progress, 0..=100, rate limited by the sender.
    Progress(u8),
    /// Verified and swapped in; a restart applies it.
    Ready { version: String },
    /// Something failed. `message` is short enough for the status bar.
    Failed(String),
}

/// The whole state machine, kept as data so it can be tested without a network.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum UpdateState {
    /// Nothing known yet: no check has finished in this session.
    #[default]
    Idle,
    Checking,
    UpToDate,
    Available {
        version: String,
        notes_url: String,
    },
    Downloading {
        version: String,
        percent: u8,
    },
    /// Installed; waiting for the user to restart.
    Ready {
        version: String,
    },
    Failed {
        message: String,
    },
}

impl UpdateState {
    /// Should the status bar show the little download badge?
    pub fn badge(&self) -> bool {
        matches!(
            self,
            UpdateState::Available { .. } | UpdateState::Ready { .. }
        )
    }

    /// One short line for the status bar, or `None` when there is nothing to
    /// say. A failed check never produces a line (only a failed download does).
    pub fn status_line(&self) -> Option<String> {
        match self {
            UpdateState::Idle | UpdateState::Checking | UpdateState::UpToDate => None,
            UpdateState::Available { version, .. } => Some(format!("Update available: v{version}")),
            UpdateState::Downloading { percent, .. } => {
                Some(format!("Downloading update: {percent}%"))
            }
            UpdateState::Ready { version } => {
                Some(format!("Updated to v{version}. Restart to apply"))
            }
            UpdateState::Failed { message } => Some(format!("Update failed: {message}")),
        }
    }

    /// Apply an event. Keeping this pure is what makes the flow testable.
    pub fn apply(&mut self, event: UpdateEvent) {
        match event {
            UpdateEvent::Available { version, notes_url } => {
                *self = UpdateState::Available { version, notes_url };
            }
            UpdateEvent::UpToDate => *self = UpdateState::UpToDate,
            UpdateEvent::Progress(p) => {
                if let UpdateState::Downloading { version, .. } = self.clone() {
                    *self = UpdateState::Downloading {
                        version,
                        percent: p.min(100),
                    };
                }
            }
            UpdateEvent::Ready { version } => *self = UpdateState::Ready { version },
            UpdateEvent::Failed(message) => *self = UpdateState::Failed { message },
        }
    }
}

/// Parse the tag from the releases API JSON and decide whether it is newer.
///
/// Hand-parsed because the payload is one flat object and a JSON crate for two
/// fields is not worth the dependency. Any missing or malformed field means
/// "no update", never an error shown to the user.
pub fn newer_release(json: &str, current: &str) -> Option<(String, String)> {
    let tag = json_string_field(json, "tag_name")?;
    let html_url = json_string_field(json, "html_url").unwrap_or_else(releases_url);
    let version = tag.trim_start_matches('v').to_string();
    if is_newer(&version, current) {
        Some((version, html_url))
    } else {
        None
    }
}

/// `candidate > current`, comparing numeric components only.
///
/// Deliberately simple: tags are `vMAJOR.MINOR.PATCH`. Anything non-numeric
/// counts as 0, so a pre-release never wins over a release.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let parse = |v: &str| -> Vec<u64> {
        v.split(['.', '-', '+'])
            .take(3)
            .map(|p| p.parse::<u64>().unwrap_or(0))
            .collect()
    };
    let (a, b) = (parse(candidate), parse(current));
    for i in 0..3 {
        let x = a.get(i).copied().unwrap_or(0);
        let y = b.get(i).copied().unwrap_or(0);
        if x != y {
            return x > y;
        }
    }
    false
}

/// Read one string field from a flat JSON object. Only what we need: a key, a
/// colon, and a double-quoted value with backslash escapes.
fn json_string_field(json: &str, key: &str) -> Option<String> {
    let needle = format!("\"{key}\"");
    let mut rest = &json[json.find(&needle)? + needle.len()..];
    rest = rest.trim_start();
    rest = rest.strip_prefix(':')?.trim_start();
    rest = rest.strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    // \uXXXX: only ASCII matters for a URL or a version tag.
                    let hex: String = (0..4).filter_map(|_| chars.next()).collect();
                    if let Ok(cp) = u32::from_str_radix(&hex, 16) {
                        if let Some(ch) = char::from_u32(cp) {
                            out.push(ch);
                        }
                    }
                }
                other => out.push(other),
            },
            other => out.push(other),
        }
    }
    None
}

/// Should this binary update itself at all?
///
/// Debug builds never do (they would replace the developer's `cargo run`
/// output), and neither does anything running out of a `target/` directory.
pub fn self_update_allowed(is_debug: bool, exe: &std::path::Path) -> bool {
    if is_debug {
        return false;
    }
    !exe.components()
        .any(|c| c.as_os_str() == std::ffi::OsStr::new("target"))
}

/// Hosts a URL may point at. An allow-list, not a deny-list: the updater must
/// never talk to anything but GitHub (v1.2 section 9).
pub fn host_allowed(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false; // plain HTTP is refused outright
    };
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .rsplit('@') // a userinfo prefix must not change the host
        .next()
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    matches!(
        host.as_str(),
        "api.github.com" | "github.com" | "objects.githubusercontent.com" | "codeload.github.com"
    )
}

#[cfg(test)]
mod tests;
