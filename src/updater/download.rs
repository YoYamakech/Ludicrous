// download.rs: fetch the release asset, verify it, swap it in.
//
// Every step is blocking and runs on a worker thread. The whole file is written
// so that a failure at any point leaves the existing executable untouched:
//   temp file next to the executable -> verify -> fsync -> rename
//
// The temp file lives beside the executable (not in /tmp) because `rename` is
// only atomic within one filesystem, and because a /tmp that is a different
// mount would make the rename fail at the last moment.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

/// A corrupt or oversized download must never be written out in full. 100 MB is
/// far more than this editor will ever be (the release is ~1.5 MB).
pub const MAX_DOWNLOAD_BYTES: u64 = 100 * 1024 * 1024;

/// The asset URL for a version, on the release's own download path.
pub fn asset_url(version: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/v{}/{}\n",
        super::UPDATE_REPO,
        version,
        super::asset_name(version)
    )
    .trim()
    .to_string()
}

/// The checksum file published next to the assets.
pub fn sums_url(version: &str) -> String {
    format!(
        "https://github.com/{}/releases/download/v{}/SHA256SUMS",
        super::UPDATE_REPO,
        version
    )
}

/// Can we write next to the executable? A package-managed install (`/usr/bin`)
/// cannot, and the answer decides between self-update and the release page.
pub fn install_dir_writable(exe: &Path) -> bool {
    let Some(dir) = exe.parent() else {
        return false;
    };
    // Creating and removing a probe file is the only reliable test: permissions
    // can come from ACLs, mount flags, or an immutable directory.
    let probe = dir.join(".ludicrous-write-test");
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Download `url` to a byte vector, refusing anything over the cap.
///
/// `progress` is called with a 0..=100 percentage, rate limited by the caller:
/// this function reports after every chunk and does not sleep.
pub fn fetch_bytes(
    url: &str,
    expected_len: Option<u64>,
    mut progress: impl FnMut(u8),
) -> Result<Vec<u8>, String> {
    if !super::host_allowed(url) {
        return Err("refusing a non-GitHub URL".to_string());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(10)))
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .build()
        .new_agent();
    let mut response = agent
        .get(url)
        .header("User-Agent", &super::check::user_agent())
        .call()
        .map_err(|e| e.to_string())?;

    // Trust the header only as an early rejection; the real guard is the count
    // below, because a server may lie or omit it.
    if let Some(len) = response.body().content_length() {
        if len > MAX_DOWNLOAD_BYTES {
            return Err("asset is larger than the limit".to_string());
        }
    }
    let total = expected_len.or_else(|| response.body().content_length());
    let mut reader = response.body_mut().as_reader();
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut last_pct = u8::MAX;
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        if out.len() as u64 > MAX_DOWNLOAD_BYTES {
            return Err("asset is larger than the limit".to_string());
        }
        if let Some(total) = total.filter(|t| *t > 0) {
            let pct = ((out.len() as u64 * 100) / total).min(100) as u8;
            if pct != last_pct {
                last_pct = pct;
                progress(pct);
            }
        }
    }
    Ok(out)
}

/// SHA-256 of a byte slice, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Pull the checksum for `name` out of a `SHA256SUMS` file.
///
/// The format is `<hex>  <name>` per line; a leading `*` (binary marker) is
/// accepted. A missing entry is an error, because an unverified binary is not
/// worth installing.
pub fn checksum_for(sums: &str, name: &str) -> Option<String> {
    for line in sums.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut parts = line.split_whitespace();
        let hex = parts.next()?;
        let file = parts.next().unwrap_or("").trim_start_matches('*');
        if file == name && hex.len() == 64 {
            return Some(hex.to_ascii_lowercase());
        }
    }
    None
}

/// Verify bytes against an expected checksum. Fails closed.
pub fn verify(bytes: &[u8], expected: &str) -> Result<(), String> {
    let got = sha256_hex(bytes);
    if got.eq_ignore_ascii_case(expected) {
        Ok(())
    } else {
        Err("checksum mismatch".to_string())
    }
}

/// The temp file we download into: beside the executable, so the final rename
/// stays on one filesystem.
pub fn temp_path(exe: &Path) -> PathBuf {
    let dir = exe.parent().unwrap_or_else(|| Path::new("."));
    dir.join(".ludicrous.new")
}

/// Write the verified bytes to `temp_path`, fsync them, set the executable bit.
pub fn stage(bytes: &[u8], exe: &Path) -> Result<PathBuf, String> {
    let temp = temp_path(exe);
    let mut file = std::fs::File::create(&temp).map_err(|e| e.to_string())?;
    file.write_all(bytes).map_err(|e| e.to_string())?;
    // fsync before the rename: a rename of unflushed data can leave an empty
    // executable after a power loss.
    file.sync_all().map_err(|e| e.to_string())?;
    drop(file);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&temp, std::fs::Permissions::from_mode(0o755))
            .map_err(|e| e.to_string())?;
    }
    Ok(temp)
}

/// Keep one copy of the binary we are about to replace, in the state directory.
pub fn backup_current(exe: &Path) -> Result<(), String> {
    let Some(state) =
        crate::config::WindowState::path().and_then(|p| p.parent().map(|d| d.to_path_buf()))
    else {
        return Ok(()); // no state dir: skip the backup rather than fail the update
    };
    std::fs::create_dir_all(&state).map_err(|e| e.to_string())?;
    let dest = state.join("ludicrous.prev");
    std::fs::copy(exe, &dest).map_err(|e| e.to_string())?;
    Ok(())
}

/// Swap the staged file over the running executable.
///
/// On Linux this is safe while running: the process keeps the old inode open,
/// so the editor does not break under itself.
pub fn swap(temp: &Path, exe: &Path) -> Result<(), String> {
    std::fs::rename(temp, exe).map_err(|e| e.to_string())
}

/// Remove the staged file after a failure, so a half download never lingers.
pub fn cleanup(temp: &Path) {
    let _ = std::fs::remove_file(temp);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_asset_url_follows_the_release_convention() {
        assert_eq!(
            asset_url("1.2.1"),
            "https://github.com/YoYamakech/Ludicrous/releases/download/v1.2.1/ludicrous-1.2.1-x86_64-linux"
        );
        assert_eq!(
            sums_url("1.2.1"),
            "https://github.com/YoYamakech/Ludicrous/releases/download/v1.2.1/SHA256SUMS"
        );
    }

    #[test]
    fn a_checksum_line_is_found_by_name() {
        let sums = "\
            aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  other\n\
            bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb  ludicrous-1.2.1-x86_64-linux\n";
        assert_eq!(
            checksum_for(sums, "ludicrous-1.2.1-x86_64-linux"),
            Some("b".repeat(64))
        );
        // The binary marker is accepted too.
        let star = format!("{}  *ludicrous-1.2.1-x86_64-linux", "c".repeat(64));
        assert_eq!(
            checksum_for(&star, "ludicrous-1.2.1-x86_64-linux"),
            Some("c".repeat(64))
        );
        // A missing entry must be `None`, not a guess.
        assert_eq!(checksum_for(sums, "nope"), None);
        // A short "hex" is not a checksum.
        assert_eq!(checksum_for("abc  f", "f"), None);
    }

    #[test]
    fn verification_fails_closed() {
        let bytes = b"hello";
        let good = sha256_hex(bytes);
        assert!(verify(bytes, &good).is_ok());
        assert!(verify(bytes, &good.to_ascii_uppercase()).is_ok());
        assert_eq!(
            verify(b"tampered", &good),
            Err("checksum mismatch".to_string())
        );
        // An empty expected checksum must never match.
        assert!(verify(bytes, "").is_err());
    }

    #[test]
    fn sha256_matches_a_known_vector() {
        // SHA-256 of the empty string.
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn the_temp_file_sits_beside_the_executable() {
        let exe = Path::new("/home/u/bin/ludicrous");
        assert_eq!(temp_path(exe), Path::new("/home/u/bin/.ludicrous.new"));
    }

    #[test]
    fn staging_creates_an_executable_file() {
        let dir = std::env::temp_dir().join("ludicrous-stage-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ludicrous");
        std::fs::write(&exe, b"old").unwrap();
        let temp = stage(b"new binary", &exe).unwrap();
        assert_eq!(std::fs::read(&temp).unwrap(), b"new binary");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&temp).unwrap().permissions().mode();
            assert_eq!(mode & 0o111, 0o111, "must be executable");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_writable_directory_is_detected_and_a_bad_one_is_not() {
        let dir = std::env::temp_dir().join("ludicrous-writable-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ludicrous");
        std::fs::write(&exe, b"x").unwrap();
        assert!(install_dir_writable(&exe));
        // The probe file must not be left behind.
        assert!(!dir.join(".ludicrous-write-test").exists());
        let _ = std::fs::remove_dir_all(&dir);
        // A directory that does not exist cannot be written.
        assert!(!install_dir_writable(Path::new("/nope/ludicrous")));
    }

    #[test]
    fn swapping_replaces_the_file() {
        let dir = std::env::temp_dir().join("ludicrous-swap-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let exe = dir.join("ludicrous");
        std::fs::write(&exe, b"old").unwrap();
        let temp = stage(b"new", &exe).unwrap();
        swap(&temp, &exe).unwrap();
        assert_eq!(std::fs::read(&exe).unwrap(), b"new");
        assert!(!temp.exists(), "the temp file is gone after the rename");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
