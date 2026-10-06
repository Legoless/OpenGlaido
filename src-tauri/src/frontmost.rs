//! The app the user is dictating into: frontmost app + browser URL, selected text, installed
//! apps and their icons. macOS via AppKit + Accessibility; Windows gets the foreground process.

use serde::Serialize;

/// Frontmost app when dictation starts; `url` is the active tab's URL for browsers.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppContext {
    pub bundle_id: String,
    pub name: String,
    pub url: Option<String>,
}

/// A cheap foreground snapshot. Accessibility enrichment happens later, against this process only.
pub struct ContextSnapshot {
    pub context: AppContext,
    #[cfg(target_os = "macos")]
    pid: i32,
}

/// Native app, window and focused-control identity; text, titles and caret position are excluded.
#[derive(Clone)]
pub struct FocusTarget {
    pid: u32,
    #[cfg(target_os = "macos")]
    window: mac::FocusElement,
    #[cfg(target_os = "macos")]
    field: mac::FocusElement,
    #[cfg(not(target_os = "macos"))]
    window: usize,
    #[cfg(not(target_os = "macos"))]
    field: usize,
}

impl FocusTarget {
    pub fn same_focus(&self, other: &Self) -> bool {
        self.pid == other.pid && self.window == other.window && self.field == other.field
    }
}

#[cfg(any(target_os = "macos", test))]
const CONTEXT_BUDGET: std::time::Duration = std::time::Duration::from_millis(250);

#[cfg(any(target_os = "macos", test))]
fn lookup_timeout(deadline: std::time::Instant, now: std::time::Instant) -> Option<f32> {
    let remaining = deadline.checked_duration_since(now)?;
    (!remaining.is_zero()).then(|| remaining.as_secs_f32().min(0.05))
}

/// An installed app, as returned by `list_apps` / `recent_apps`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AppInfo {
    pub bundle_id: String,
    pub name: String,
    pub path: String,
}

/// Browsers get website rules instead of app rules, so the app picker leaves them out.
pub const BROWSERS: &[&str] = &[
    "com.apple.Safari",
    "com.apple.SafariTechnologyPreview",
    "com.google.Chrome",
    "com.google.Chrome.canary",
    "org.chromium.Chromium",
    "com.brave.Browser",
    "com.microsoft.edgemac",
    "company.thebrowser.Browser",
    "com.vivaldi.Vivaldi",
    "com.operasoftware.Opera",
    "org.mozilla.firefox",
    "app.zen-browser.zen",
    "chrome.exe",
    "msedge.exe",
    "firefox.exe",
    "brave.exe",
];

pub fn is_browser(bundle_id: &str) -> bool {
    BROWSERS.contains(&bundle_id)
}

/// Webmail tab titles, for browsers that don't expose the URL through Accessibility.
#[cfg(any(target_os = "macos", windows, test))]
fn url_from_title(title: &str) -> Option<String> {
    let t = title.to_ascii_lowercase();
    if t.contains("gmail") {
        Some("https://mail.google.com/".into())
    } else if t.contains("outlook") {
        Some("https://outlook.live.com/".into())
    } else {
        None
    }
}

#[cfg(target_os = "macos")]
pub use mac::{app_icon, capture_context, capture_focus, current_app_id, current_context, is_installed, list_apps, resolve_context, selected_text};

#[cfg(not(target_os = "macos"))]
pub use other::{app_icon, capture_context, capture_focus, current_app_id, current_context, is_installed, list_apps, resolve_context, selected_text};

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use objc2::rc::{autoreleasepool, Retained};
    use objc2::runtime::AnyObject;
    use objc2::AnyThread;
    use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep, NSWorkspace};
    use objc2_foundation::{NSBundle, NSDictionary, NSFileManager, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL};
    use std::collections::{HashMap, HashSet};
    use std::ffi::c_void;
    use std::sync::{Mutex, OnceLock};
    use std::time::{Duration, Instant};

    const SELF_ID: &str = "com.openglaido.app";

    type AXUIElementRef = *const c_void;

    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        fn AXUIElementCreateApplication(pid: i32) -> AXUIElementRef;
        fn AXUIElementCreateSystemWide() -> AXUIElementRef;
        fn AXUIElementCopyAttributeValue(element: AXUIElementRef, attribute: *const c_void, value: *mut *const c_void) -> i32;
        fn AXUIElementSetAttributeValue(element: AXUIElementRef, attribute: *const c_void, value: *const c_void) -> i32;
        fn AXUIElementSetMessagingTimeout(element: AXUIElementRef, timeout: f32) -> i32;
        fn AXUIElementGetPid(element: AXUIElementRef, pid: *mut i32) -> i32;
        fn AXUIElementGetTypeID() -> usize;
    }

    #[link(name = "CoreFoundation", kind = "framework")]
    extern "C" {
        fn CFEqual(first: *const c_void, second: *const c_void) -> u8;
        fn CFGetTypeID(value: *const c_void) -> usize;
    }

    /// An owned CF/AX object (toll-free bridged, so objc2 can release and downcast it).
    type Ax = Retained<AnyObject>;

    #[derive(Clone)]
    pub(super) struct FocusElement(Ax);

    // SAFETY: these owned references are AXUIElement CF identity objects, not arbitrary Cocoa
    // objects. After capture they are only retained/released or compared with CFEqual, never
    // mutated or used for AX messaging. Core Foundation permits immutable objects across threads.
    unsafe impl Send for FocusElement {}
    unsafe impl Sync for FocusElement {}

    impl PartialEq for FocusElement {
        fn eq(&self, other: &Self) -> bool {
            unsafe { CFEqual(Retained::as_ptr(&self.0).cast(), Retained::as_ptr(&other.0).cast()) != 0 }
        }
    }

    impl FocusElement {
        fn for_process(value: Ax, expected_pid: i32) -> Option<Self> {
            let ptr = Retained::as_ptr(&value).cast();
            let mut pid = 0;
            let valid = unsafe {
                CFGetTypeID(ptr) == AXUIElementGetTypeID() && AXUIElementGetPid(ptr, &mut pid) == 0 && pid == expected_pid
            };
            valid.then_some(Self(value))
        }
    }

    /// Takes ownership of a +1 CF reference.
    fn own(ptr: *const c_void) -> Option<Ax> {
        unsafe { Retained::from_raw(ptr as *mut AnyObject) }
    }

    fn attr(el: &Ax, name: &str, deadline: Instant) -> Option<Ax> {
        let timeout = lookup_timeout(deadline, Instant::now())?;
        let name = NSString::from_str(name);
        let mut value: *const c_void = std::ptr::null();
        let ok = unsafe {
            AXUIElementSetMessagingTimeout(Retained::as_ptr(el).cast(), timeout);
            AXUIElementCopyAttributeValue(Retained::as_ptr(el).cast(), Retained::as_ptr(&name).cast(), &mut value)
        };
        if ok == 0 {
            own(value)
        } else {
            None
        }
    }

    fn string_attr(el: &Ax, name: &str, deadline: Instant) -> Option<String> {
        attr(el, name, deadline)?.downcast::<NSString>().ok().map(|s| s.to_string())
    }

    fn app_element(pid: i32) -> Option<Ax> {
        own(unsafe { AXUIElementCreateApplication(pid) })
    }

    /// Chromium/Electron only build their accessibility tree when asked; ask once per process.
    /// (AXManualAccessibility only: AXEnhancedUserInterface breaks window animations/managers.)
    fn enable_web_accessibility(app: &Ax, pid: i32, deadline: Instant) {
        static DONE: OnceLock<Mutex<HashSet<i32>>> = OnceLock::new();
        if DONE.get_or_init(Default::default).lock().unwrap().contains(&pid) {
            return;
        }
        let Some(timeout) = lookup_timeout(deadline, Instant::now()) else { return };
        let yes = NSNumber::new_bool(true);
        let name = NSString::from_str("AXManualAccessibility");
        let result = unsafe {
            AXUIElementSetMessagingTimeout(Retained::as_ptr(app).cast(), timeout);
            AXUIElementSetAttributeValue(Retained::as_ptr(app).cast(), Retained::as_ptr(&name).cast(), Retained::as_ptr(&yes).cast())
        };
        if result == 0 {
            DONE.get_or_init(Default::default).lock().unwrap().insert(pid);
        }
    }

    fn url_of(el: &Ax, deadline: Instant) -> Option<String> {
        let value = attr(el, "AXURL", deadline)?;
        match value.downcast::<NSURL>() {
            Ok(url) => url.absoluteString().map(|s| s.to_string()),
            Err(value) => value.downcast::<NSString>().ok().map(|s| s.to_string()),
        }
    }

    /// The URL of the web area around the focused element, else inside the focused window.
    fn browser_url(app: &Ax, deadline: Instant) -> Option<String> {
        let mut el = attr(app, "AXFocusedUIElement", deadline);
        for _ in 0..40 {
            let Some(cur) = el else { break };
            if string_attr(&cur, "AXRole", deadline).as_deref() == Some("AXWebArea") {
                if let Some(url) = url_of(&cur, deadline) {
                    return Some(url);
                }
            }
            el = attr(&cur, "AXParent", deadline);
        }
        let window = attr(app, "AXFocusedWindow", deadline)?;
        // Breadth-first, bounded: the web area sits a few levels below the window.
        let mut queue = vec![window.clone()];
        let mut seen = 0;
        while let Some(cur) = (!queue.is_empty()).then(|| queue.remove(0)) {
            seen += 1;
            if seen > 300 || Instant::now() >= deadline {
                break;
            }
            if string_attr(&cur, "AXRole", deadline).as_deref() == Some("AXWebArea") {
                if let Some(url) = url_of(&cur, deadline) {
                    return Some(url);
                }
            }
            if let Some(children) = attr(&cur, "AXChildren", deadline).and_then(|c| c.downcast::<objc2_foundation::NSArray>().ok()) {
                queue.extend(children.iter());
            }
        }
        string_attr(&window, "AXTitle", deadline).and_then(|t| url_from_title(&t))
    }

    pub fn current_app_id() -> Option<String> {
        autoreleasepool(|_| NSWorkspace::sharedWorkspace().frontmostApplication()?.bundleIdentifier().map(|id| id.to_string()))
    }

    pub fn capture_context() -> Option<ContextSnapshot> {
        autoreleasepool(|_| {
            let app = NSWorkspace::sharedWorkspace().frontmostApplication()?;
            let bundle_id = app.bundleIdentifier()?.to_string();
            let name = app.localizedName().map(|n| n.to_string()).unwrap_or_else(|| bundle_id.clone());
            Some(ContextSnapshot { context: AppContext { bundle_id, name, url: None }, pid: app.processIdentifier() })
        })
    }

    /// Missing permission and incomplete AX snapshots are inconclusive.
    pub fn capture_focus() -> Option<FocusTarget> {
        autoreleasepool(|_| {
            let pid = NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier();
            let deadline = Instant::now() + CONTEXT_BUDGET;
            let app = app_element(pid)?;
            // Initialize Chromium before recording its field identity. A later context lookup
            // must not be the operation that first exposes the browser's accessibility tree.
            enable_web_accessibility(&app, pid, deadline);
            let read = || {
                Some(FocusTarget {
                    pid: pid as u32,
                    window: FocusElement::for_process(attr(&app, "AXFocusedWindow", deadline)?, pid)?,
                    field: FocusElement::for_process(attr(&app, "AXFocusedUIElement", deadline)?, pid)?,
                })
            };
            let focus = read().or_else(|| {
                enable_web_accessibility(&app, pid, deadline);
                read()
            })?;
            // AX lookups can outlive the foreground app; discard a mixed snapshot.
            (NSWorkspace::sharedWorkspace().frontmostApplication()?.processIdentifier() == pid).then_some(focus)
        })
    }

    #[cfg(test)]
    pub(super) fn test_focus_target(pid: u32, window: i32, field: i32) -> FocusTarget {
        // App identity references exercise CFEqual without reading any live UI attributes.
        FocusTarget { pid, window: FocusElement(app_element(window).unwrap()), field: FocusElement(app_element(field).unwrap()) }
    }

    pub fn resolve_context(mut target: ContextSnapshot, selection: bool) -> (AppContext, Option<String>) {
        let deadline = Instant::now() + CONTEXT_BUDGET;
        autoreleasepool(|_| {
            let selected = selection.then(|| selected_text_for(target.pid, deadline)).flatten();
            if is_browser(&target.context.bundle_id) {
                target.context.url = app_element(target.pid).and_then(|el| {
                    enable_web_accessibility(&el, target.pid, deadline);
                    browser_url(&el, deadline)
                });
            }
            (target.context, selected)
        })
    }

    pub fn current_context() -> Option<AppContext> {
        capture_context().map(|target| resolve_context(target, false).0)
    }

    fn selected_text_for(pid: i32, deadline: Instant) -> Option<String> {
            let el = app_element(pid)?;
            let read = || {
                let focused = attr(&el, "AXFocusedUIElement", deadline).or_else(|| {
                    let system = own(unsafe { AXUIElementCreateSystemWide() })?;
                    let focused = attr(&system, "AXFocusedUIElement", deadline)?;
                    let mut actual_pid = 0;
                    let result = unsafe { AXUIElementGetPid(Retained::as_ptr(&focused).cast(), &mut actual_pid) };
                    (result == 0 && actual_pid == pid).then_some(focused)
                })?;
                string_attr(&focused, "AXSelectedText", deadline).filter(|t| !t.trim().is_empty())
            };
            // Chromium/Electron only expose text once asked to; native apps never need it.
            read().or_else(|| {
                enable_web_accessibility(&el, pid, deadline);
                read()
            })
    }

    /// Selected text in the captured process, never another app reached during an AX lookup.
    pub fn selected_text() -> Option<String> {
        autoreleasepool(|_| {
            selected_text_for(capture_context()?.pid, Instant::now() + CONTEXT_BUDGET)
        })
    }

    fn app_info(path: &str) -> Option<AppInfo> {
        let bundle = NSBundle::bundleWithPath(&NSString::from_str(path))?;
        let bundle_id = bundle.bundleIdentifier()?.to_string();
        if is_browser(&bundle_id) || bundle_id == SELF_ID {
            return None;
        }
        let name = NSFileManager::defaultManager().displayNameAtPath(&NSString::from_str(path)).to_string();
        let name = name.strip_suffix(".app").unwrap_or(&name).to_string();
        Some(AppInfo { bundle_id, name, path: path.to_string() })
    }

    /// Installed apps, sorted by name; scanned at most once a minute.
    pub fn list_apps() -> Vec<AppInfo> {
        type Cached = Option<(Instant, Vec<AppInfo>)>;
        static CACHE: OnceLock<Mutex<Cached>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some((at, apps)) = cache.lock().unwrap().as_ref() {
            if at.elapsed() < Duration::from_secs(60) {
                return apps.clone();
            }
        }
        let home = std::env::var("HOME").unwrap_or_default();
        let roots = ["/Applications".to_string(), "/Applications/Utilities".into(), "/System/Applications".into(),
            "/System/Applications/Utilities".into(), format!("{home}/Applications")];
        let mut paths = Vec::new();
        for root in roots {
            let Ok(entries) = std::fs::read_dir(&root) else { continue };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().is_some_and(|e| e == "app") {
                    paths.push(path);
                } else if path.is_dir() && !path.ends_with("Utilities") {
                    // One level of folders (e.g. /Applications/Setapp/*.app).
                    if let Ok(inner) = std::fs::read_dir(&path) {
                        paths.extend(inner.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|e| e == "app")));
                    }
                }
            }
        }
        let mut seen = HashSet::new();
        let mut apps: Vec<AppInfo> = autoreleasepool(|_| {
            paths.iter().filter_map(|p| app_info(&p.to_string_lossy())).filter(|a| seen.insert(a.bundle_id.clone())).collect()
        });
        apps.sort_by_key(|a| a.name.to_lowercase());
        *cache.lock().unwrap() = Some((Instant::now(), apps.clone()));
        apps
    }

    pub fn is_installed(bundle_id: &str) -> bool {
        autoreleasepool(|_| NSWorkspace::sharedWorkspace().URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id)).is_some())
    }

    /// 64 px PNG of the app's icon as a data URL; cached per bundle id.
    pub fn app_icon(bundle_id: &str) -> Option<String> {
        static CACHE: OnceLock<Mutex<HashMap<String, Option<String>>>> = OnceLock::new();
        let cache = CACHE.get_or_init(Default::default);
        if let Some(hit) = cache.lock().unwrap().get(bundle_id) {
            return hit.clone();
        }
        let icon = autoreleasepool(|_| {
            let ws = NSWorkspace::sharedWorkspace();
            let url = ws.URLForApplicationWithBundleIdentifier(&NSString::from_str(bundle_id))?;
            let path = url.path()?;
            let image = ws.iconForFile(&path);
            let mut rect = NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(64.0, 64.0));
            let cg = unsafe { image.CGImageForProposedRect_context_hints(&mut rect, None, None) }?;
            let rep = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &cg);
            let png = unsafe { rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new()) }?;
            use base64::Engine;
            Some(format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(png.to_vec())))
        });
        cache.lock().unwrap().insert(bundle_id.to_string(), icon.clone());
        icon
    }
}

#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;

    pub fn current_app_id() -> Option<String> {
        current_context().map(|context| context.bundle_id)
    }

    pub fn capture_context() -> Option<ContextSnapshot> {
        current_context().map(|context| ContextSnapshot { context })
    }

    pub fn resolve_context(target: ContextSnapshot, _selection: bool) -> (AppContext, Option<String>) {
        (target.context, None)
    }

    /// Window and HWND-backed focused-control identity, without reading text or caret state.
    pub fn capture_focus() -> Option<FocusTarget> {
        #[cfg(windows)]
        unsafe {
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetGUIThreadInfo, GetWindowThreadProcessId, GUITHREADINFO};
            let window = GetForegroundWindow();
            if window.0.is_null() {
                return None;
            }
            let mut pid = 0;
            let thread = GetWindowThreadProcessId(window, Some(&mut pid));
            if thread == 0 || pid == 0 {
                return None;
            }
            let mut info = GUITHREADINFO { cbSize: std::mem::size_of::<GUITHREADINFO>() as u32, ..Default::default() };
            GetGUIThreadInfo(thread, &mut info).ok()?;
            if info.hwndFocus.0.is_null() || info.hwndActive != window || GetForegroundWindow() != window {
                return None;
            }
            return Some(FocusTarget { pid, window: window.0 as usize, field: info.hwndFocus.0 as usize });
        }
        #[allow(unreachable_code)]
        None
    }

    /// Foreground process on Windows: "slack.exe" as the id, "slack" as the name. No URL.
    // ponytail: Windows has no browser URL / selection / app list / icons yet (UI Automation needed).
    pub fn current_context() -> Option<AppContext> {
        #[cfg(windows)]
        unsafe {
            use windows::core::PWSTR;
            use windows::Win32::Foundation::CloseHandle;
            use windows::Win32::System::Threading::{
                OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
            };
            use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId};
            let hwnd = GetForegroundWindow();
            let mut pid = 0u32;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            let process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
            let mut buf = [0u16; 1024];
            let mut len = buf.len() as u32;
            let ok = QueryFullProcessImageNameW(process, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
            let _ = CloseHandle(process);
            ok.ok()?;
            let path = String::from_utf16_lossy(&buf[..len as usize]);
            let file = path.rsplit(['\\', '/']).next()?.to_lowercase();
            let name = file.strip_suffix(".exe").unwrap_or(&file).to_string();
            let mut title = [0u16; 512];
            let n = GetWindowTextW(hwnd, &mut title);
            let url = if is_browser(&file) { url_from_title(&String::from_utf16_lossy(&title[..n.max(0) as usize])) } else { None };
            return Some(AppContext { bundle_id: file, name, url });
        }
        #[allow(unreachable_code)]
        None
    }

    pub fn selected_text() -> Option<String> {
        None
    }

    pub fn list_apps() -> Vec<AppInfo> {
        Vec::new()
    }

    pub fn is_installed(_bundle_id: &str) -> bool {
        true
    }

    pub fn app_icon(_bundle_id: &str) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn focus_identity_distinguishes_apps_windows_and_fields() {
        fn target(pid: u32, window: usize, field: usize) -> FocusTarget {
            #[cfg(target_os = "macos")]
            {
                mac::test_focus_target(pid, window as i32, field as i32)
            }
            #[cfg(not(target_os = "macos"))]
            FocusTarget { pid, window, field }
        }
        let original = target(1, 2, 3);
        assert!(original.same_focus(&target(1, 2, 3)), "fresh references to the same focus stay equal");
        assert!(!original.same_focus(&target(4, 2, 3)), "another app changes focus");
        assert!(!original.same_focus(&target(1, 4, 3)), "another window in the same app changes focus");
        assert!(!original.same_focus(&target(1, 2, 4)), "another field in the same window changes focus");
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<FocusTarget>();
    }

    #[test]
    fn accessibility_calls_share_one_overall_deadline() {
        let started = std::time::Instant::now();
        let deadline = started + CONTEXT_BUDGET;
        assert_eq!(lookup_timeout(deadline, started), Some(0.05));
        assert_eq!(lookup_timeout(deadline, deadline - std::time::Duration::from_millis(5)), Some(0.005));
        assert_eq!(lookup_timeout(deadline, deadline), None);
        assert_eq!(lookup_timeout(deadline, deadline + std::time::Duration::from_millis(1)), None);
    }

    #[test]
    fn webmail_titles() {
        assert_eq!(url_from_title("Inbox (3) - me@gmail.com - Gmail").as_deref(), Some("https://mail.google.com/"));
        assert_eq!(url_from_title("Mail - Dal - Outlook").as_deref(), Some("https://outlook.live.com/"));
        assert_eq!(url_from_title("GitHub"), None);
        assert!(is_browser("com.google.Chrome") && !is_browser("com.apple.Notes"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn lists_real_apps_without_browsers() {
        let apps = list_apps();
        assert!(apps.iter().any(|a| a.bundle_id == "com.apple.TextEdit"), "TextEdit missing");
        assert!(!apps.iter().any(|a| is_browser(&a.bundle_id)));
        assert!(is_installed("com.apple.TextEdit"));
        let icon = app_icon("com.apple.TextEdit").expect("icon");
        assert!(icon.starts_with("data:image/png;base64,") && icon.len() < 200_000, "{}", icon.len());
    }
}
