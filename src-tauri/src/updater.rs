//! Signed GitHub updates. Download freely; reserve installation only after all work is idle.
use std::sync::{Arc, Mutex, atomic::{AtomicBool, Ordering}};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, WebviewWindow};
use tauri_plugin_updater::UpdaterExt;
use crate::{AppState, Mode};

const CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
const MAX_DEFER: Duration = Duration::from_secs(30 * 60);
const RESTART_FILE: &str = ".update-restart.json";
const INSTALLING: &str = "OpenGlaido is installing an update. Try again shortly.";

#[derive(Clone, Debug, Serialize)]
pub struct UpdateStatus {
    phase: &'static str,
    current_version: String,
    next_version: Option<String>,
    message: Option<&'static str>,
    revision: u64,
}

struct Inner {
    status: UpdateStatus,
    checking: bool,
    installing: bool,
    extra_jobs: usize,
}

pub struct UpdateState {
    inner: Arc<Mutex<Inner>>,
    #[cfg(target_os = "macos")]
    quiet_reopen_until: Mutex<Option<Instant>>,
}

impl UpdateState {
    pub fn new(version: String) -> Self {
        Self {
            inner: Arc::new(Mutex::new(Inner {
                status: UpdateStatus { phase: "idle", current_version: version, next_version: None, message: None, revision: 0 },
                checking: false, installing: false, extra_jobs: 0,
            })),
            #[cfg(target_os = "macos")]
            quiet_reopen_until: Mutex::new(None),
        }
    }

    fn status(&self) -> UpdateStatus { self.inner.lock().unwrap().status.clone() }

    pub fn is_installing(&self) -> bool { self.inner.lock().unwrap().installing }

    pub fn activity(&self) -> Result<UpdateActivity, String> {
        let mut inner = self.inner.lock().unwrap();
        if inner.installing { return Err(INSTALLING.into()); }
        inner.extra_jobs += 1;
        Ok(UpdateActivity(self.inner.clone()))
    }

    /// Caller holds AppState::mode on a worker thread, so recording cannot start in this gap.
    pub fn try_install(&self, mode_idle: bool, jobs: usize) -> Option<InstallPermit> {
        let mut inner = self.inner.lock().unwrap();
        if !mode_idle || jobs != 0 || inner.extra_jobs != 0 || inner.installing { return None; }
        inner.installing = true;
        Some(InstallPermit { inner: self.inner.clone(), release: true })
    }

    fn begin_check(&self) -> Option<UpdateStatus> {
        let mut inner = self.inner.lock().unwrap();
        if inner.checking || inner.installing { return None; }
        inner.checking = true;
        inner.status.phase = "checking";
        inner.status.next_version = None;
        inner.status.message = None;
        inner.status.revision += 1;
        Some(inner.status.clone())
    }

    fn publish(&self, app: &AppHandle, phase: &'static str, version: Option<String>, message: Option<&'static str>, finished: bool) {
        let status = {
            let mut inner = self.inner.lock().unwrap();
            inner.status.phase = phase;
            inner.status.next_version = version;
            inner.status.message = message;
            inner.status.revision += 1;
            if finished { inner.checking = false; }
            inner.status.clone()
        };
        let _ = app.emit("app-update-status", status);
    }

    #[cfg(target_os = "macos")]
    pub fn suppress_initial_reopen(&self) -> bool {
        self.quiet_reopen_until.lock().unwrap().take().is_some_and(|until| Instant::now() <= until)
    }
}

pub struct UpdateActivity(Arc<Mutex<Inner>>);
impl Drop for UpdateActivity {
    fn drop(&mut self) { self.0.lock().unwrap().extra_jobs -= 1; }
}

pub struct InstallPermit { inner: Arc<Mutex<Inner>>, release: bool }
impl InstallPermit {
    fn commit(mut self) { self.release = false; }
}
impl Drop for InstallPermit {
    fn drop(&mut self) {
        if self.release { self.inner.lock().unwrap().installing = false; }
    }
}

#[derive(Deserialize, Serialize)]
struct RestartMarker { version: String, created: u64, #[serde(default)] failed: bool }
impl RestartMarker {
    fn valid(&self, version: &str, now: u64) -> bool {
        self.version == version && now.checked_sub(self.created).is_some_and(|age| age <= 300)
    }
}

fn now() -> u64 { SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_secs() }

fn consume_restart(app: &AppHandle) -> Option<RestartMarker> {
    let path = app.state::<AppState>().db.app_dir.join(RESTART_FILE);
    let marker = std::fs::read(&path).ok().and_then(|bytes| serde_json::from_slice::<RestartMarker>(&bytes).ok());
    let _ = std::fs::remove_file(path);
    marker.filter(|marker| marker.valid(&app.package_info().version.to_string(), now()))
}

fn write_restart(app: &AppHandle, version: String, failed: bool) -> Result<(), &'static str> {
    let marker = app.state::<AppState>().db.app_dir.join(RESTART_FILE);
    let temporary = marker.with_extension("tmp");
    let data = serde_json::to_vec(&RestartMarker { version, created: now(), failed })
        .map_err(|_| "Couldn’t install the update. Try again later.")?;
    if std::fs::write(&temporary, data).and_then(|_| std::fs::rename(&temporary, &marker)).is_err() {
        let _ = std::fs::remove_file(temporary);
        return Err("Couldn’t install the update. Try again later.");
    }
    Ok(())
}

fn installable_build(debug: bool, macos: bool, executable: &std::path::Path) -> bool {
    if debug || !executable.is_absolute() { return false; }
    if !macos { return true; }
    executable.parent().is_some_and(|bin| bin.file_name().is_some_and(|name| name == "MacOS")
        && bin.parent().is_some_and(|contents| contents.file_name().is_some_and(|name| name == "Contents")
            && contents.parent().is_some_and(|bundle| bundle.extension().is_some_and(|ext| ext == "app"))))
}

fn updates_enabled() -> bool {
    std::env::current_exe().is_ok_and(|path| installable_build(cfg!(debug_assertions), cfg!(target_os = "macos"), &path))
}

pub fn start(app: &AppHandle) {
    // The window starts hidden, preventing a flash during a background update restart.
    let marker = consume_restart(app);
    let failed_install = marker.as_ref().is_some_and(|marker| marker.failed);
    if marker.is_some() {
        #[cfg(target_os = "macos")]
        { *app.state::<UpdateState>().quiet_reopen_until.lock().unwrap() = Some(Instant::now() + Duration::from_secs(3)); }
    } else if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
    }
    if failed_install {
        app.state::<UpdateState>().publish(app, "error", None, Some("Couldn’t install the update. Try again later."), true);
    }
    if updates_enabled() {
        let app = app.clone();
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(if failed_install { CHECK_INTERVAL } else { Duration::from_secs(60) }).await;
            loop {
                schedule_check(&app);
                tokio::time::sleep(CHECK_INTERVAL).await;
            }
        });
    }
}

fn ensure_main(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == "main" { Ok(()) } else { Err("Updates are only available from the main window".into()) }
}

#[tauri::command]
pub fn get_update_status(app: AppHandle, window: WebviewWindow) -> Result<UpdateStatus, String> {
    ensure_main(&window)?;
    Ok(app.state::<UpdateState>().status())
}

#[tauri::command]
pub fn check_for_updates(app: AppHandle, window: WebviewWindow) -> Result<UpdateStatus, String> {
    ensure_main(&window)?;
    schedule_check(&app);
    Ok(app.state::<UpdateState>().status())
}

fn schedule_check(app: &AppHandle) {
    if !updates_enabled() {
        app.state::<UpdateState>().publish(app, "idle", None, Some("Updates are available in packaged builds only."), true);
        return;
    }
    let Some(status) = app.state::<UpdateState>().begin_check() else { return; };
    let _ = app.emit("app-update-status", status);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(message) = check_and_install(&app).await {
            app.state::<UpdateState>().publish(&app, "error", None, Some(message), true);
        }
    });
}

fn windows_closed(app: &AppHandle) -> bool {
    ["main", "command"].into_iter().all(|label| {
        app.get_webview_window(label).is_none_or(|window| window.is_visible().is_ok_and(|visible| !visible))
    })
}

async fn check_and_install(app: &AppHandle) -> Result<(), &'static str> {
    const CHECK_ERROR: &str = "Couldn’t check for updates. Try again later.";
    const INSTALL_ERROR: &str = "Couldn’t install the update. Try again later.";
    let state = app.state::<UpdateState>();
    let cleanup = app.clone();
    let exit_prepared = Arc::new(AtomicBool::new(false));
    let exit_hook = exit_prepared.clone();
    let updater = app.updater_builder().timeout(Duration::from_secs(300))
        .on_before_exit(move || {
            // Windows installers exit without RunEvent::Exit. Keep the existing cleanup there too.
            exit_hook.store(true, Ordering::SeqCst);
            crate::prepare_exit(&cleanup);
            cleanup.cleanup_before_exit();
        }).build().map_err(|_| CHECK_ERROR)?;
    let update = match tokio::time::timeout(Duration::from_secs(30), updater.check()).await {
        Ok(Ok(Some(update))) => update,
        Ok(Ok(None)) => { state.publish(app, "up_to_date", None, None, true); return Ok(()); }
        Ok(Err(tauri_plugin_updater::Error::ReleaseNotFound)) => {
            // GitHub serves no public manifest until the first stable release is published.
            state.publish(app, "idle", None, Some("No public updates are available yet."), true);
            return Ok(());
        }
        _ => return Err(CHECK_ERROR),
    };
    let version = update.version.clone();
    state.publish(app, "downloading", Some(version.clone()), None, false);
    // Tauri verifies both the signature and its bound version before returning these bytes.
    let bytes = update.download(|_, _| {}, || {}).await
        .map_err(|_| "Couldn’t download a verified update. Try again later.")?;
    state.publish(app, "ready", Some(version.clone()), None, false);
    let deadline = Instant::now() + MAX_DEFER;
    let permit = loop {
        if Instant::now() >= deadline {
            state.publish(app, "idle", None, None, true);
            return Ok(());
        }
        let handle = app.clone();
        let permit = tauri::async_runtime::spawn_blocking(move || {
            // Query windows before mode: window operations can dispatch onto the main thread.
            if !windows_closed(&handle) { return None; }
            let app_state = handle.state::<AppState>();
            let mode = app_state.mode.lock().unwrap();
            handle.state::<UpdateState>().try_install(*mode == Mode::Idle, app_state.jobs.load(Ordering::SeqCst))
        }).await.map_err(|_| INSTALL_ERROR)?;
        if let Some(permit) = permit {
            // Window opening holds an activity guard. Recheck after the reservation, without
            // mode held, to catch a window opened between the first snapshot and reservation.
            if windows_closed(app) { break permit; }
            drop(permit);
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    };
    state.publish(app, "installing", Some(version.clone()), None, false);
    write_restart(app, version, false)?;
    let handle = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let result = update.install(&bytes);
        if result.is_ok() {
            // Keep admission closed through the asynchronous Exit event and process restart.
            permit.commit();
            handle.request_restart();
        } else {
            let marker = handle.state::<AppState>().db.app_dir.join(RESTART_FILE);
            let _ = std::fs::remove_file(marker);
            if exit_prepared.load(Ordering::SeqCst) {
                // Windows can reject the installer after the plugin has dismantled the app.
                // Restart the existing version instead of leaving a dead tray/window behind.
                let _ = write_restart(&handle, handle.package_info().version.to_string(), true);
                permit.commit();
                handle.request_restart();
                return Ok(());
            }
        }
        result.map_err(|_| INSTALL_ERROR)
    }).await.map_err(|_| INSTALL_ERROR)??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn install_admission_waits_for_all_work_and_blocks_new_work() {
        let state = UpdateState::new("0.1.0".into());
        assert!(state.try_install(false, 0).is_none());
        assert!(state.try_install(true, 1).is_none());
        let first = state.activity().unwrap();
        let second = state.activity().unwrap();
        assert!(state.try_install(true, 0).is_none());
        drop(first);
        assert!(state.try_install(true, 0).is_none());
        drop(second);
        let permit = state.try_install(true, 0).unwrap();
        assert!(state.is_installing());
        assert!(state.activity().is_err());
        assert!(state.try_install(true, 0).is_none());
        drop(permit); // Failed installation permits normal work and retry.
        assert!(!state.is_installing());
        drop(state.activity().unwrap());
        state.try_install(true, 0).unwrap().commit();
        assert!(state.activity().is_err());
        assert!(state.is_installing());
    }

    #[test]
    fn update_checks_do_not_overlap_or_start_while_installing() {
        let state = UpdateState::new("0.1.0".into());
        let status = state.begin_check().unwrap();
        assert_eq!(status.phase, "checking");
        assert_eq!(status.revision, 1);
        assert!(state.begin_check().is_none());
        state.inner.lock().unwrap().checking = false;
        let _permit = state.try_install(true, 0).unwrap();
        assert!(state.begin_check().is_none());
    }

    #[test]
    fn racing_activity_and_install_reservations_are_exclusive() {
        // Keep both results alive until both contenders have attempted admission.
        for _ in 0..32 {
            let state = UpdateState::new("0.1.0".into());
            let start = std::sync::Barrier::new(2);
            let admitted = std::sync::Barrier::new(2);
            std::thread::scope(|scope| {
                let activity = scope.spawn(|| {
                    start.wait();
                    let guard = state.activity();
                    admitted.wait();
                    guard.is_ok()
                });
                start.wait();
                let permit = state.try_install(true, 0);
                admitted.wait();
                assert_ne!(activity.join().unwrap(), permit.is_some());
            });
            assert!(!state.is_installing());
            assert!(state.try_install(true, 0).is_some());
        }
    }

    #[test]
    fn hidden_restart_requires_matching_version_and_fresh_timestamp() {
        let marker = RestartMarker { version: "0.2.0".into(), created: 1000, failed: false };
        assert!(marker.valid("0.2.0", 1000));
        assert!(marker.valid("0.2.0", 1300));
        assert!(!marker.valid("0.1.0", 1001));
        assert!(!marker.valid("0.2.0", 1301));
        assert!(!marker.valid("0.2.0", 999));
    }

    #[test]
    fn updates_never_install_into_dev_or_unbundled_mac_paths() {
        use std::path::Path;
        let bundle = std::env::temp_dir().join("OpenGlaido.app/Contents/MacOS/openglaido");
        assert!(installable_build(false, true, &bundle));
        assert!(!installable_build(true, true, &bundle));
        for path in ["/project/target/debug/openglaido", "/project/target/release/openglaido", "/tmp/Contents/MacOS/openglaido"] {
            assert!(!installable_build(false, true, Path::new(path)));
        }
        assert!(!installable_build(false, false, Path::new("relative.exe")));
        assert!(!installable_build(true, false, &bundle));
    }
}
