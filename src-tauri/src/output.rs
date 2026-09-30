//! "Mute background": mutes the default output device while recording and restores it after.
//! A marker file survives a crash mid-recording so the next launch unmutes.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const MARKER: &str = "muted-by-openglaido";

/// Bumped by every restore, so a delayed mute that hasn't fired yet is cancelled.
static GENERATION: AtomicU64 = AtomicU64::new(0);
/// The device we muted (it was unmuted before); restore unmutes exactly that one, even if the
/// default output changed meanwhile.
static MUTED: Mutex<Option<platform::Device>> = Mutex::new(None);

fn marker(dir: &Path) -> PathBuf {
    dir.join(MARKER)
}

/// Mutes after `delay` (lets the start chime play) unless `restore` runs first.
pub fn mute_after(dir: &Path, delay: Duration) {
    let generation = GENERATION.load(Ordering::SeqCst);
    let dir = dir.to_path_buf();
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        let mut muted = MUTED.lock().unwrap();
        if GENERATION.load(Ordering::SeqCst) != generation || muted.is_some() {
            return;
        }
        let Some(device) = platform::default_device() else { return };
        // Already muted by the user, or not controllable: leave it alone.
        if platform::is_muted(device) == Some(false) {
            let _ = std::fs::write(marker(&dir), device.to_string());
            if platform::set_muted(device, true) {
                *muted = Some(device);
            } else {
                let _ = std::fs::remove_file(marker(&dir));
            }
        }
    });
}

/// Undoes our mute (if any) and cancels a pending one.
pub fn restore(dir: &Path) {
    GENERATION.fetch_add(1, Ordering::SeqCst);
    if let Some(device) = MUTED.lock().unwrap().take() {
        platform::set_muted(device, false);
    }
    let _ = std::fs::remove_file(marker(dir));
}

/// At startup: a marker means we crashed while muted.
pub fn recover(dir: &Path) {
    if let Ok(text) = std::fs::read_to_string(marker(dir)) {
        let device = text.trim().parse().ok().or_else(platform::default_device);
        if let Some(device) = device {
            platform::set_muted(device, false);
        }
        let _ = std::fs::remove_file(marker(dir));
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::c_void;

    #[repr(C)]
    struct Address {
        selector: u32,
        scope: u32,
        element: u32,
    }

    #[link(name = "CoreAudio", kind = "framework")]
    extern "C" {
        fn AudioObjectGetPropertyData(id: u32, addr: *const Address, qs: u32, q: *const c_void, size: *mut u32, data: *mut c_void) -> i32;
        fn AudioObjectSetPropertyData(id: u32, addr: *const Address, qs: u32, q: *const c_void, size: u32, data: *const c_void) -> i32;
    }

    const fn fourcc(s: &[u8; 4]) -> u32 {
        u32::from_be_bytes(*s)
    }
    const SYSTEM_OBJECT: u32 = 1;

    fn get_u32(id: u32, addr: &Address) -> Option<u32> {
        let (mut value, mut size) = (0u32, 4u32);
        let status = unsafe { AudioObjectGetPropertyData(id, addr, 0, std::ptr::null(), &mut size, (&mut value as *mut u32).cast()) };
        (status == 0).then_some(value)
    }

    /// CoreAudio device id.
    pub type Device = u32;

    pub fn default_device() -> Option<Device> {
        let addr = Address { selector: fourcc(b"dOut"), scope: fourcc(b"glob"), element: 0 };
        get_u32(SYSTEM_OBJECT, &addr).filter(|id| *id != 0)
    }

    fn mute_address() -> Address {
        Address { selector: fourcc(b"mute"), scope: fourcc(b"outp"), element: 0 }
    }

    pub fn is_muted(device: Device) -> Option<bool> {
        get_u32(device, &mute_address()).map(|v| v != 0)
    }

    pub fn set_muted(device: Device, muted: bool) -> bool {
        let value: u32 = muted.into();
        let status = unsafe { AudioObjectSetPropertyData(device, &mute_address(), 0, std::ptr::null(), 4, (&value as *const u32).cast()) };
        status == 0
    }
}

#[cfg(windows)]
mod platform {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    fn volume() -> Option<IAudioEndpointVolume> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let enumerator: IMMDeviceEnumerator = CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
            let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
            device.Activate(CLSCTX_ALL, None).ok()
        }
    }

    // ponytail: always the current default endpoint (no stable id kept across a device switch).
    pub type Device = u32;

    pub fn default_device() -> Option<Device> {
        volume().map(|_| 0)
    }

    pub fn is_muted(_device: Device) -> Option<bool> {
        unsafe { volume()?.GetMute().ok().map(|m| m.as_bool()) }
    }

    pub fn set_muted(_device: Device, muted: bool) -> bool {
        volume().is_some_and(|v| unsafe { v.SetMute(muted, std::ptr::null()).is_ok() })
    }
}

#[cfg(not(any(target_os = "macos", windows)))]
mod platform {
    pub type Device = u32;
    pub fn default_device() -> Option<Device> {
        None
    }
    pub fn is_muted(_device: Device) -> Option<bool> {
        None
    }
    pub fn set_muted(_device: Device, _muted: bool) -> bool {
        false
    }
}
