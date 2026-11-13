// check.rs: one HTTPS GET to the GitHub releases API, on a worker thread.
//
// Blocking by design: `ureq` has no runtime and no async, which is why it was
// chosen. This file never runs on the event loop.
//
// Failure policy (v1.2 section 9): a failed check is silent. Being offline, a
// rate limit, a proxy saying no, malformed JSON — all of it means "we learned
// nothing", never a message to the user.

/// Where the latest release lives.
pub fn latest_url() -> String {
    format!(
        "https://api.github.com/repos/{}/releases/latest",
        super::UPDATE_REPO
    )
}

/// Fetch the latest release and return its JSON body.
///
/// Short timeouts: the caller is a background thread, but there is no reason to
/// hold one open for a minute when the user is offline.
pub fn fetch_latest() -> Result<String, String> {
    let url = latest_url();
    if !super::host_allowed(&url) {
        return Err("refusing a non-GitHub URL".to_string());
    }
    let agent = ureq::Agent::config_builder()
        .timeout_connect(Some(std::time::Duration::from_secs(10)))
        .timeout_global(Some(std::time::Duration::from_secs(30)))
        .build()
        .new_agent();
    let body = agent
        .get(&url)
        // GitHub rejects requests without a User-Agent.
        .header("User-Agent", &user_agent())
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_to_string()
        .map_err(|e| e.to_string())?;
    Ok(body)
}

/// `ludicrous/<version>`, the only identifying string we send.
pub fn user_agent() -> String {
    format!("ludicrous/{}", env!("CARGO_PKG_VERSION"))
}

/// What a finished check means for the UI.
pub fn interpret(body: &str) -> super::UpdateEvent {
    match super::newer_release(body, env!("CARGO_PKG_VERSION")) {
        Some((version, notes_url)) => super::UpdateEvent::Available { version, notes_url },
        None => super::UpdateEvent::UpToDate,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_url_is_the_releases_api_of_the_confirmed_repo() {
        assert_eq!(
            latest_url(),
            "https://api.github.com/repos/YoYamakech/Ludicrous/releases/latest"
        );
    }

    #[test]
    fn the_user_agent_carries_the_version() {
        let ua = user_agent();
        assert!(ua.starts_with("ludicrous/"));
        assert!(ua.contains(env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn a_newer_tag_becomes_an_available_event() {
        let body =
            r#"{"tag_name":"v9.9.9","html_url":"https://github.com/x/y/releases/tag/v9.9.9"}"#;
        match interpret(body) {
            super::super::UpdateEvent::Available { version, notes_url } => {
                assert_eq!(version, "9.9.9");
                assert!(notes_url.contains("v9.9.9"));
            }
            other => panic!("expected Available, got {other:?}"),
        }
    }

    #[test]
    fn the_current_version_is_not_an_update() {
        let body = format!(
            r#"{{"tag_name":"v{}","html_url":"https://github.com/x/y"}}"#,
            env!("CARGO_PKG_VERSION")
        );
        assert_eq!(interpret(&body), super::super::UpdateEvent::UpToDate);
    }

    #[test]
    fn garbage_is_up_to_date_not_an_error() {
        // A rate-limit body, an HTML error page, an empty string: all mean
        // "nothing learned", and the user must not see a message.
        for body in [
            "",
            "{}",
            "<html>nope</html>",
            r#"{"message":"rate limited"}"#,
        ] {
            assert_eq!(
                interpret(body),
                super::super::UpdateEvent::UpToDate,
                "{body:?}"
            );
        }
    }
}

/// A real request, run only when asked to. Kept `#[ignore]` because the test
/// suite must pass with the network off.
#[cfg(test)]
mod live {
    #[test]
    #[ignore = "network"]
    fn a_real_check_talks_to_github_and_is_silent_on_failure() {
        match super::fetch_latest() {
            Ok(body) => {
                // Against the real repository this must be a well-formed answer:
                // either "up to date" or an available update, never a panic.
                let event = super::interpret(&body);
                eprintln!("live check: {event:?}");
                assert!(body.contains("tag_name") || body.contains("message"));
            }
            Err(e) => eprintln!("offline, which is fine: {e}"),
        }
    }
}
