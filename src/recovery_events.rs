//! Windows wake and connectivity notifications, without a polling thread.
use crate::diagnose;
use std::ffi::c_void;
use std::sync::Mutex;
use std::time::{Duration, Instant};
use windows::core::{s, w, GUID};
use windows::Win32::Foundation::{BOOLEAN, HANDLE, HWND, LPARAM, WIN32_ERROR, WPARAM};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, PNETWORK_CONNECTIVITY_HINT_CHANGE_CALLBACK,
};
use windows::Win32::Networking::WinSock::{
    NetworkConnectivityLevelHintInternetAccess, NL_NETWORK_CONNECTIVITY_HINT,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Power::{
    RegisterPowerSettingNotification, RegisterSuspendResumeNotification,
    UnregisterPowerSettingNotification, UnregisterSuspendResumeNotification, HPOWERNOTIFY,
    POWERBROADCAST_SETTING,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, DEVICE_NOTIFY_WINDOW_HANDLE};

pub const MESSAGE: u32 = 0x8000 + 4;
// WinNT.h: interactive applications monitor their session, not the console display.
const GUID_SESSION_DISPLAY_STATUS: GUID = GUID::from_u128(0x2b84c20e_ad23_4ddf_93db_05ffbd7efca5);

/// Recognize a WM_POWERBROADCAST/PBT_POWERSETTINGCHANGE display-on notification.
///
/// # Safety
/// A non-null lparam must point to a live Windows notification header. When its
/// DataLength is four, the following four data bytes must also be readable.
/// The pointer must not be retained or forwarded asynchronously.
pub unsafe fn display_turned_on(lparam: LPARAM) -> bool {
    let setting = lparam.0 as *const POWERBROADCAST_SETTING;
    if setting.is_null() {
        return false;
    }
    // SAFETY: the caller supplies a live notification. Raw, unaligned reads avoid
    // assuming the alignment or Rust reference layout of its variable-length Data.
    unsafe {
        if std::ptr::addr_of!((*setting).PowerSetting).read_unaligned()
            != GUID_SESSION_DISPLAY_STATUS
            || std::ptr::addr_of!((*setting).DataLength).read_unaligned() != 4
        {
            return false;
        }
        std::ptr::addr_of!((*setting).Data)
            .cast::<u32>()
            .read_unaligned()
            == 1
    }
}
static CONNECTIVITY: Mutex<Connectivity> = Mutex::new(Connectivity { online: None });
#[derive(Default)]
struct Connectivity {
    online: Option<bool>,
}
impl Connectivity {
    fn observe(&mut self, online: bool) -> bool {
        let restored = self.online == Some(false) && online;
        self.online = Some(online);
        restored
    }
}
#[derive(Default)]
pub struct Debounce {
    last: [Option<Instant>; 2],
}
impl Debounce {
    pub fn accept(&mut self, network: bool, now: Instant) -> bool {
        let last = &mut self.last[network as usize];
        if last.is_some_and(|time| now.saturating_duration_since(time) < Duration::from_secs(5)) {
            return false;
        }
        *last = Some(now);
        true
    }
}
type NotifyHintChange = unsafe extern "system" fn(
    PNETWORK_CONNECTIVITY_HINT_CHANGE_CALLBACK,
    *const c_void,
    BOOLEAN,
    *mut HANDLE,
) -> WIN32_ERROR;

/// Windows 10 2004+ only. Resolved at runtime so older builds still start, without network recovery.
unsafe fn notify_network_connectivity_hint_change() -> Option<NotifyHintChange> {
    let module = LoadLibraryW(w!("iphlpapi.dll")).ok()?;
    let function = GetProcAddress(module, s!("NotifyNetworkConnectivityHintChange"))?;
    Some(std::mem::transmute::<
        unsafe extern "system" fn() -> isize,
        NotifyHintChange,
    >(function))
}
pub struct Watch {
    power: Option<HPOWERNOTIFY>,
    display: Option<HPOWERNOTIFY>,
    network: isize,
}
unsafe extern "system" fn network_changed(
    context: *const c_void,
    hint: NL_NETWORK_CONNECTIVITY_HINT,
) {
    let restored = CONNECTIVITY
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .observe(hint.ConnectivityLevel == NetworkConnectivityLevelHintInternetAccess);
    if restored {
        let _ = PostMessageW(HWND(context as *mut _), MESSAGE, WPARAM(1), LPARAM(0));
    }
}
impl Watch {
    pub fn register(hwnd: HWND) -> Self {
        unsafe {
            let power = RegisterSuspendResumeNotification(hwnd, DEVICE_NOTIFY_WINDOW_HANDLE)
                .map_err(|e| diagnose::log_error("wake notification registration failed", e))
                .ok();
            let display = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_SESSION_DISPLAY_STATUS,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            )
            .map_err(|e| diagnose::log_error("display notification registration failed", e))
            .ok();
            let mut network = HANDLE::default();
            let Some(notify) = notify_network_connectivity_hint_change() else {
                diagnose::log("network notification unavailable on this Windows version");
                return Self {
                    power,
                    display,
                    network: 0,
                };
            };
            let status = notify(
                Some(network_changed),
                hwnd.0 as *const c_void,
                BOOLEAN(1),
                &mut network,
            );
            if status.0 != 0 {
                diagnose::log(format!(
                    "network notification registration failed code={}",
                    status.0
                ));
            }
            diagnose::log(format!(
                "recovery notifications registered wake={} display={} network={}",
                power.is_some(),
                display.is_some(),
                status.0 == 0
            ));
            Self {
                power,
                display,
                network: if status.0 == 0 { network.0 as isize } else { 0 },
            }
        }
    }
}
impl Drop for Watch {
    fn drop(&mut self) {
        unsafe {
            if self.network != 0 {
                let _ = CancelMibChangeNotify2(HANDLE(self.network as *mut _));
            }
            if let Some(power) = self.power {
                let _ = UnregisterSuspendResumeNotification(power);
            }
            if let Some(display) = self.display {
                let _ = UnregisterPowerSettingNotification(display);
            }
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn display_notification_requires_the_session_guid_a_dword_and_on_state() {
        #[repr(C)]
        struct Notification {
            setting: GUID,
            length: u32,
            value: u32,
        }
        let mut notification = Notification {
            setting: GUID_SESSION_DISPLAY_STATUS,
            length: 4,
            value: 1,
        };
        let check = |notification: &Notification| {
            // SAFETY: the repr(C) fixture has the notification header and four data bytes.
            unsafe { display_turned_on(LPARAM(notification as *const Notification as isize)) }
        };
        assert!(check(&notification));
        for value in [0, 2, 256] {
            notification.value = value;
            assert!(!check(&notification));
        }
        notification.value = 1;
        for length in [0, 1, 3, 8] {
            notification.length = length;
            assert!(!check(&notification));
        }
        notification.length = 4;
        notification.setting = GUID::from_u128(0);
        assert!(!check(&notification));
        // SAFETY: null is explicitly accepted and never dereferenced.
        assert!(!unsafe { display_turned_on(LPARAM(0)) });
    }
    #[test]
    fn initial_or_cost_only_changes_do_not_refresh_but_restored_internet_does() {
        let mut state = Connectivity::default();
        assert!(!state.observe(true));
        assert!(!state.observe(true));
        assert!(!state.observe(false));
        assert!(!state.observe(false));
        assert!(state.observe(true));
        assert!(!state.observe(true));
    }
    #[test]
    fn duplicate_wake_events_coalesce_without_suppressing_a_later_network_recovery() {
        let now = Instant::now();
        let mut events = Debounce::default();
        assert!(events.accept(false, now));
        assert!(!events.accept(false, now + Duration::from_secs(1)));
        assert!(events.accept(true, now + Duration::from_secs(1)));
        assert!(!events.accept(true, now + Duration::from_secs(2)));
        assert!(events.accept(true, now + Duration::from_secs(6)));
    }
}
