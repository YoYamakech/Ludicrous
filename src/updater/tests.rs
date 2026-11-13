// Tests for the updater's pure parts: version comparison, the release JSON
// parser, the state machine and the host allow-list. Nothing here touches the
// network; the network code is exercised by hand against a real release.

use super::*;

#[test]
fn version_comparison_is_numeric_not_lexical() {
    assert!(is_newer("1.2.1", "1.2.0"));
    assert!(is_newer("1.10.0", "1.9.9"), "10 > 9, not '1' < '9'");
    assert!(is_newer("2.0.0", "1.99.99"));
    assert!(!is_newer("1.2.0", "1.2.0"));
    assert!(!is_newer("1.2.0", "1.2.1"));
    assert!(!is_newer("1.2.0", "1.2.1-beta"), "a pre-release never wins");
}

#[test]
fn the_release_tag_is_stripped_of_its_v() {
    let json = r#"{"tag_name":"v1.3.0","html_url":"https://github.com/YoYamakech/Ludicrous/releases/tag/v1.3.0"}"#;
    let (version, url) = newer_release(json, "1.2.0").expect("newer");
    assert_eq!(version, "1.3.0");
    assert!(url.contains("v1.3.0"));
}

#[test]
fn a_json_object_without_a_tag_is_not_an_update() {
    assert_eq!(newer_release(r#"{"message":"Not Found"}"#, "1.2.0"), None);
    assert_eq!(newer_release("", "1.2.0"), None);
    assert_eq!(newer_release("<html>", "1.2.0"), None);
    // A tag that is not a version is treated as 0.0.0: never newer.
    assert_eq!(newer_release(r#"{"tag_name":"nightly"}"#, "1.2.0"), None);
}

#[test]
fn a_missing_release_url_falls_back_to_the_releases_page() {
    let json = r#"{"tag_name":"v2.0.0"}"#;
    let (_, url) = newer_release(json, "1.2.0").expect("newer");
    assert_eq!(url, format!("https://github.com/{UPDATE_REPO}/releases"));
}

#[test]
fn json_escapes_are_understood() {
    // A URL with an escaped slash must not corrupt the value.
    let json = r#"{"tag_name":"v1.5.0","html_url":"https:\/\/github.com\/a\/b"}"#;
    let (_, url) = newer_release(json, "1.2.0").unwrap();
    assert_eq!(url, "https://github.com/a/b");
}

#[test]
fn the_state_machine_reaches_ready_only_through_progress() {
    let mut st = UpdateState::default();
    assert_eq!(st, UpdateState::Idle);
    assert!(!st.badge());
    assert_eq!(st.status_line(), None);

    st.apply(UpdateEvent::Available {
        version: "1.3.0".into(),
        notes_url: "https://example".into(),
    });
    assert!(st.badge(), "the badge appears for an available update");
    assert!(st.status_line().unwrap().contains("v1.3.0"));

    // A progress event before a download started must not invent one.
    st.apply(UpdateEvent::Progress(50));
    assert!(matches!(st, UpdateState::Available { .. }));

    // The download itself is started by the app, which sets this state.
    st = UpdateState::Downloading {
        version: "1.3.0".into(),
        percent: 0,
    };
    st.apply(UpdateEvent::Progress(50));
    assert_eq!(
        st,
        UpdateState::Downloading {
            version: "1.3.0".into(),
            percent: 50
        }
    );
    // Progress past 100 is clamped rather than showing 120%.
    st.apply(UpdateEvent::Progress(250));
    assert_eq!(
        st,
        UpdateState::Downloading {
            version: "1.3.0".into(),
            percent: 100
        }
    );

    st.apply(UpdateEvent::Ready {
        version: "1.3.0".into(),
    });
    assert!(st.badge());
    assert!(st.status_line().unwrap().contains("Restart"));
}

#[test]
fn up_to_date_and_failures_do_not_show_a_badge() {
    let mut st = UpdateState::default();
    st.apply(UpdateEvent::UpToDate);
    assert!(!st.badge());
    assert_eq!(st.status_line(), None, "no news is no message");

    st.apply(UpdateEvent::Failed("checksum mismatch".into()));
    assert!(!st.badge());
    assert_eq!(
        st.status_line().unwrap(),
        "Update failed: checksum mismatch"
    );
}

#[test]
fn only_github_over_https_is_allowed() {
    assert!(host_allowed("https://api.github.com/repos/x/y"));
    assert!(host_allowed(
        "https://github.com/x/y/releases/download/v1/a"
    ));
    assert!(host_allowed("https://objects.githubusercontent.com/a"));
    for bad in [
        "http://github.com/x/y",
        "https://evil.com/github.com",
        "https://github.com.evil.com/x",
        "https://github.com@evil.com/x",
        "ftp://github.com/x",
        "",
        "github.com/x",
    ] {
        assert!(!host_allowed(bad), "{bad} must be refused");
    }
}

#[test]
fn self_update_rules() {
    use std::path::Path;
    assert!(self_update_allowed(
        false,
        Path::new("/home/u/bin/ludicrous")
    ));
    // A debug build must never replace itself: it would eat cargo's output.
    assert!(!self_update_allowed(
        true,
        Path::new("/home/u/bin/ludicrous")
    ));
    // Neither must anything running out of target/.
    assert!(!self_update_allowed(
        false,
        Path::new("/home/u/proj/target/debug/ludicrous")
    ));
    assert!(!self_update_allowed(
        false,
        Path::new("/home/u/target/release/ludicrous")
    ));
}

#[test]
fn the_asset_name_matches_the_published_convention() {
    assert_eq!(asset_name("1.2.1"), "ludicrous-1.2.1-x86_64-linux");
}
