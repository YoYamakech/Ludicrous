// app/update.rs: the app's side of the updater.
//
// The rules live in `updater/` (pure, tested, no winit). This file is the glue:
// when to start a check, how to start a download, and what the user clicked.
//
// Timing rules from v1.2 section 9:
//   - the automatic check starts at least 10 s after the first frame,
//   - at most once per 24 h (the timestamp lives in the state file),
//   - never in a debug build or from a `target/` directory,
//   - nothing blocks the event loop: every network step is a thread that reports
//     through `UserEvent::Update`.

#![cfg(feature = "updater")]

use crate::app::{App, UserEvent};
use crate::updater::{UpdateEvent, UpdateState};

/// Wait this long after the first frame before checking: cold start stays clean
/// and a user who quits immediately never triggers a request.
const CHECK_DELAY: std::time::Duration = std::time::Duration::from_secs(10);
/// At most one automatic check per day.
const CHECK_INTERVAL_SECS: u64 = 24 * 60 * 60;

impl App {
    /// What the Settings rows and their button should say. Pure: it reads the
    /// state and the install directory, never the network.
    pub(crate) fn update_info(&self) -> crate::ui::settings::UpdateInfo {
        use crate::ui::settings::UpdateInfo;
        // Whether we may replace our own binary decides the button's verb: a
        // package-managed install can only open the release page.
        let exe = std::env::current_exe().unwrap_or_default();
        let writable =
            self.update_allowed() && crate::updater::download::install_dir_writable(&exe);
        match &self.update {
            UpdateState::Idle | UpdateState::UpToDate => UpdateInfo {
                label: "Check now".to_string(),
                action: "Check now".to_string(),
                enabled: true,
            },
            UpdateState::Checking => UpdateInfo {
                label: "Checking for updates…".to_string(),
                action: "Checking…".to_string(),
                enabled: false,
            },
            UpdateState::Available { version, .. } => UpdateInfo {
                label: format!("Update available: v{version}"),
                action: if writable {
                    "Download and install".to_string()
                } else {
                    "Open release page".to_string()
                },
                enabled: true,
            },
            UpdateState::Downloading { version, percent } => UpdateInfo {
                label: format!("Downloading v{version}: {percent}%"),
                action: "Downloading…".to_string(),
                enabled: false,
            },
            UpdateState::Ready { version } => UpdateInfo {
                label: format!("Updated to v{version}"),
                action: "Restart".to_string(),
                enabled: true,
            },
            UpdateState::Failed { message } => UpdateInfo {
                label: format!("Update failed: {message}"),
                action: "Retry".to_string(),
                enabled: true,
            },
        }
    }

    /// Run the Settings action row. This is the only place a click and the
    /// Enter key end up, so both do exactly the same thing.
    pub(crate) fn run_update_action(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        match self.update.clone() {
            UpdateState::Idle | UpdateState::UpToDate | UpdateState::Failed { .. } => {
                self.start_check();
            }
            UpdateState::Available { .. } => {
                let exe = std::env::current_exe().unwrap_or_default();
                if !self.update_allowed() || !crate::updater::download::install_dir_writable(&exe) {
                    self.open_release_page();
                } else {
                    self.start_update_download();
                }
            }
            UpdateState::Ready { .. } => self.restart_now(event_loop),
            // Nothing to press: the button is disabled in these states anyway.
            UpdateState::Checking | UpdateState::Downloading { .. } => {}
        }
    }

    /// Dev aid in the same spirit as `LUDICROUS_DEBUG_DROP`: pretend a release
    /// exists so the badge and the Settings row can be looked at without
    /// publishing a real one. Debug builds only, and no request is made.
    pub(crate) fn fake_update_from_env(&mut self) {
        if !cfg!(debug_assertions) {
            return;
        }
        let Some(version) = std::env::var_os("LUDICROUS_FAKE_UPDATE") else {
            return;
        };
        self.update = UpdateState::Available {
            version: version.to_string_lossy().into_owned(),
            notes_url: crate::updater::releases_url(),
        };
    }

    /// Is self-updating even allowed here? Debug builds and `target/` are not.
    pub(crate) fn update_allowed(&self) -> bool {
        let exe = std::env::current_exe().unwrap_or_default();
        crate::updater::self_update_allowed(cfg!(debug_assertions), &exe)
    }

    /// Called from `about_to_wait`. Returns the instant it wants to be woken at
    /// when the ten-second delay is still running, `None` otherwise. It never
    /// polls and never blocks.
    pub(crate) fn maybe_check_updates(&mut self) -> Option<std::time::Instant> {
        if self.update_check_started || !self.update_allowed() {
            return None;
        }
        // `check_updates` off means never ask GitHub anything at all.
        if !self.config.check_updates {
            return None;
        }
        let first_frame = self.first_frame_at?;
        let due = first_frame + CHECK_DELAY;
        let now = std::time::Instant::now();
        if now < due {
            return Some(due);
        }
        // Once per day, measured in wall-clock seconds so it survives restarts.
        let now_secs = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        if let Some(last) = self.state.last_update_check {
            if now_secs.saturating_sub(last) < CHECK_INTERVAL_SECS {
                // Start a new day's wait from here rather than waking forever.
                self.update_check_started = true;
                return None;
            }
        }
        self.update_check_started = true;
        self.start_check();
        None
    }

    /// Force a check, from the Settings row. Ignores the 24 h rule.
    pub(crate) fn start_check(&mut self) {
        self.update = UpdateState::Checking;
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let event = match crate::updater::check::fetch_latest() {
                // A failed check is silent: offline is normal. `UpToDate`
                // simply produces no message.
                Err(_) => UpdateEvent::UpToDate,
                Ok(body) => crate::updater::check::interpret(&body),
            };
            let _ = proxy.send_event(UserEvent::Update(event));
        });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// Received on the UI thread.
    pub(crate) fn update_event(&mut self, ev: UpdateEvent) {
        // The check finished: record the time so the next automatic one waits a
        // day. Done here, not on the worker, because only the UI thread writes
        // the state file at quit.
        if matches!(ev, UpdateEvent::Available { .. } | UpdateEvent::UpToDate) {
            self.state.last_update_check = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .ok();
            self.state.save();
        }
        // A progress event needs the download state that the click created.
        if let UpdateEvent::Progress(p) = ev {
            if let UpdateState::Downloading { version, .. } = self.update.clone() {
                // Rate limit to ~4/s as the spec asks, so the status bar does
                // not force a redraw per 64 KB.
                let now = std::time::Instant::now();
                if now.duration_since(self.last_progress_at).as_millis() < 250 && p < 100 {
                    return;
                }
                self.last_progress_at = now;
                self.update = UpdateState::Downloading {
                    version,
                    percent: p,
                };
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
            return;
        }
        self.update.apply(ev.clone());
        // A finished install tells the user to restart; nothing else does.
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// The user clicked `Download and install`.
    pub(crate) fn start_update_download(&mut self) {
        let UpdateState::Available { version, .. } = self.update.clone() else {
            return;
        };
        let exe = std::env::current_exe().unwrap_or_default();
        if !crate::updater::download::install_dir_writable(&exe) {
            // Package-managed install: never try to replace ourselves.
            self.update = UpdateState::Failed {
                message: "install directory is read-only".to_string(),
            };
            self.open_release_page();
            return;
        }
        self.update = UpdateState::Downloading {
            version: version.clone(),
            percent: 0,
        };
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let result = run_download(&version, |pct| {
                let _ = proxy.send_event(UserEvent::Update(UpdateEvent::Progress(pct)));
            });
            let event = match result {
                Ok(()) => UpdateEvent::Ready { version },
                Err(message) => UpdateEvent::Failed(message),
            };
            let _ = proxy.send_event(UserEvent::Update(event));
        });
        if let Some(w) = self.window.as_ref() {
            w.request_redraw();
        }
    }

    /// The user clicked `Open release page`, or we cannot self-update.
    pub(crate) fn open_release_page(&mut self) {
        let url = match &self.update {
            UpdateState::Available { notes_url, .. } => notes_url.clone(),
            _ => crate::updater::releases_url(),
        };
        // Never block the loop on a browser launch.
        std::thread::spawn(move || {
            let _ = std::process::Command::new("xdg-open")
                .arg(&url)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn();
        });
    }

    /// Restart into the new binary. Goes through the quit guard, exactly like
    /// Ctrl+Q, so unsaved work is never lost. Never automatic.
    pub(crate) fn restart_now(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        // Reuse the existing guard: if it warns, the restart waits.
        let control = match self.workspace.as_mut() {
            Some(ws) => ws.request_quit(),
            None => crate::editor::Control::Quit,
        };
        if control == crate::editor::Control::Quit {
            self.restart_with_exec(event_loop);
        }
    }

    /// Spawn the (already replaced) executable with the same arguments, then
    /// exit. Separated so the guard can call it later.
    pub(crate) fn restart_with_exec(&mut self, event_loop: &winit::event_loop::ActiveEventLoop) {
        let exe = std::env::current_exe().unwrap_or_default();
        let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
        // Spawn before exiting: if the spawn fails we still have a window to
        // show the error in.
        match std::process::Command::new(&exe).args(&args).spawn() {
            Ok(_) => {
                self.state.save();
                event_loop.exit();
            }
            Err(e) => {
                self.update = UpdateState::Failed {
                    message: format!("could not restart: {e}"),
                };
                if let Some(w) = self.window.as_ref() {
                    w.request_redraw();
                }
            }
        }
    }
}

/// Download, verify, stage and swap. Runs on a worker thread.
///
/// Every failure deletes the temp file and returns a short message; there is no
/// state in which a half-verified binary sits next to the executable.
fn run_download(version: &str, mut progress: impl FnMut(u8)) -> Result<(), String> {
    use crate::updater::download as dl;
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let name = crate::updater::asset_name(version);

    // 1. The checksum list. Without it we do not install anything: a download
    //    we cannot verify is worse than no update.
    let sums = dl::fetch_bytes(&dl::sums_url(version), None, |_| {})
        .map_err(|_| "could not fetch SHA256SUMS".to_string())?;
    let sums = String::from_utf8(sums).map_err(|_| "SHA256SUMS is not UTF-8".to_string())?;
    let expected = dl::checksum_for(&sums, &name)
        .ok_or_else(|| format!("no checksum for {name} in SHA256SUMS"))?;

    // 2. The asset itself, with progress.
    let bytes = dl::fetch_bytes(&dl::asset_url(version), None, |p| progress(p / 2))?;
    progress(60);

    // 3. Verify before touching the disk. Fails closed.
    dl::verify(&bytes, &expected)?;
    progress(75);

    // 4. Back up, stage, swap.
    dl::backup_current(&exe)?;
    let temp = dl::stage(&bytes, &exe)?;
    progress(90);
    if let Err(e) = dl::swap(&temp, &exe) {
        dl::cleanup(&temp);
        return Err(e);
    }
    progress(100);
    Ok(())
}
