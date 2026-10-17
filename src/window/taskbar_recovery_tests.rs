use super::*;

#[test]
fn unexpected_destruction_waits_for_a_taskbar_and_survives_handle_reuse() {
    let lifecycle = TaskbarLifecycle::new();
    assert!(!lifecycle.record_widget_destroyed(true));
    assert_eq!(lifecycle.relaunch_reason(false, true, false, false), None);
    assert_eq!(
        lifecycle.relaunch_reason(false, true, false, true),
        Some("widget_destroyed")
    );
    // Both numeric handles may have been reused, but the destruction latch remains.
    assert_eq!(
        lifecycle.relaunch_reason(true, true, true, true),
        Some("widget_destroyed")
    );
}

#[test]
fn live_widget_recovers_only_when_its_taskbar_is_missing() {
    let lifecycle = TaskbarLifecycle::new();
    assert_eq!(lifecycle.relaunch_reason(true, true, true, true), None);
    assert_eq!(lifecycle.relaunch_reason(true, true, false, false), None);
    assert_eq!(
        lifecycle.relaunch_reason(true, true, false, true),
        Some("taskbar_handle_changed")
    );
    assert_eq!(lifecycle.relaunch_reason(true, false, false, true), None);
    assert_eq!(
        lifecycle.relaunch_reason(false, false, false, true),
        Some("widget_destroyed")
    );
}

#[test]
fn intentional_exit_and_confirmed_session_end_never_relaunch() {
    for session_end in [false, true] {
        let lifecycle = TaskbarLifecycle::new();
        if session_end {
            lifecycle.end_session(true);
        } else {
            lifecycle.request_exit();
        }
        assert!(lifecycle.record_widget_destroyed(true));
        lifecycle.end_session(false);
        assert_eq!(lifecycle.relaunch_reason(false, true, false, true), None);
    }
}

#[test]
fn canceled_session_end_resumes_recovery_without_premature_quit() {
    let lifecycle = TaskbarLifecycle::new();
    lifecycle.query_end_session();
    assert!(!lifecycle.record_widget_destroyed(true));
    assert_eq!(lifecycle.relaunch_reason(false, true, false, true), None);
    lifecycle.end_session(false);
    assert_eq!(
        lifecycle.relaunch_reason(false, true, false, true),
        Some("widget_destroyed")
    );
}

#[test]
fn destruction_before_initialization_still_quits() {
    assert!(TaskbarLifecycle::new().record_widget_destroyed(false));
}

struct TestWindow(HWND);

impl Drop for TestWindow {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            unsafe {
                let _ = DestroyWindow(self.0);
            }
        }
    }
}

unsafe fn synthetic_taskbar() -> TestWindow {
    TestWindow(
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            windows::core::w!("STATIC"),
            windows::core::w!("taskbar-recovery-test"),
            WS_POPUP,
            0,
            0,
            800,
            40,
            None,
            None,
            None,
            None,
        )
        .unwrap(),
    )
}

unsafe extern "system" fn recovery_test_widget_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if msg == WM_NCCREATE {
        let create = &*(lparam.0 as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
    }
    if msg == WM_DESTROY {
        let lifecycle = &*(GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const TaskbarLifecycle);
        if lifecycle.record_widget_destroyed(true) {
            PostQuitMessage(0);
        }
        return LRESULT(0);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

#[test]
fn native_parent_destruction_preserves_observer_and_explicit_close_still_quits() {
    let lifecycle = TaskbarLifecycle::new();
    unsafe {
        let observer = TestWindow(create_taskbar_observer(&lifecycle).unwrap());
        let mut parent = synthetic_taskbar();
        let class_name = windows::core::w!("CodexUsageRecoveryTestWidget");
        let instance = GetModuleHandleW(PCWSTR::null()).unwrap();
        let class = WNDCLASSW {
            lpfnWndProc: Some(recovery_test_widget_proc),
            hInstance: HINSTANCE(instance.0),
            lpszClassName: class_name,
            ..Default::default()
        };
        assert_ne!(RegisterClassW(&class), 0);
        let mut child = TestWindow(
            CreateWindowExW(
                WS_EX_LAYERED,
                class_name,
                PCWSTR::null(),
                WS_POPUP,
                0,
                0,
                40,
                20,
                None,
                None,
                instance,
                Some(&lifecycle as *const TaskbarLifecycle as *const std::ffi::c_void),
            )
            .unwrap(),
        );
        native_interop::embed_in_taskbar(child.0, parent.0);
        assert_eq!(GetParent(child.0).unwrap(), parent.0);
        assert!(!IsWindowVisible(observer.0).as_bool());
        assert!(GetParent(observer.0).is_err());
        DestroyWindow(parent.0).unwrap();
        parent.0 = HWND::default();
        assert!(!IsWindow(child.0).as_bool());
        child.0 = HWND::default();
        assert!(IsWindow(observer.0).as_bool());
        let mut message = MSG::default();
        assert!(!PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
        assert_eq!(lifecycle.relaunch_reason(false, true, false, false), None);

        let replacement = synthetic_taskbar();
        SendMessageW(
            observer.0,
            lifecycle.taskbar_created_message.load(Ordering::SeqCst),
            WPARAM(0),
            LPARAM(0),
        );
        assert_eq!(
            lifecycle.relaunch_reason(false, true, false, IsWindow(replacement.0).as_bool()),
            Some("widget_destroyed")
        );
        // Updates/menu exit must work even though the widget HWND no longer exists.
        SendMessageW(observer.0, WM_CLOSE, WPARAM(0), LPARAM(0));
        assert_eq!(lifecycle.relaunch_reason(false, true, false, true), None);
        assert!(PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
        assert_eq!(message.wParam.0, 0);
    }
}

#[test]
fn native_observer_handles_session_cancel_and_confirmed_end() {
    let lifecycle = TaskbarLifecycle::new();
    unsafe {
        let observer = TestWindow(create_taskbar_observer(&lifecycle).unwrap());
        assert_eq!(
            SendMessageW(observer.0, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0)),
            LRESULT(1)
        );
        assert!(lifecycle.recovery_suppressed());
        SendMessageW(observer.0, WM_ENDSESSION, WPARAM(0), LPARAM(0));
        assert!(!lifecycle.recovery_suppressed());
        let mut message = MSG::default();
        assert!(!PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
        SendMessageW(observer.0, WM_QUERYENDSESSION, WPARAM(0), LPARAM(0));
        SendMessageW(observer.0, WM_ENDSESSION, WPARAM(1), LPARAM(0));
        assert!(lifecycle.recovery_suppressed());
        assert!(PeekMessageW(&mut message, None, WM_QUIT, WM_QUIT, PM_REMOVE).as_bool());
    }
}

#[test]
fn saved_monitor_is_restored_after_reordering_disconnect_and_delayed_recreation() {
    let taskbar = |id, primary| native_interop::TaskbarWindow {
        hwnd: HWND(id as *mut _),
        rect: RECT::default(),
        is_primary: primary,
    };
    let primary = taskbar(1isize, true);
    let secondary = taskbar(2isize, false);
    let other = taskbar(3isize, false);
    let device_for = |taskbar: &native_interop::TaskbarWindow| {
        Some(format!(r"\\.\DISPLAY{}", taskbar.hwnd.0 as isize))
    };
    let selected = Some(r"\\.\DISPLAY2");
    assert_eq!(
        preferred_taskbar_index_with(&[], selected, 1, device_for),
        0
    );
    assert_eq!(
        preferred_taskbar_index_with(&[other, primary], selected, 0, device_for),
        1
    );
    assert_eq!(
        preferred_taskbar_index_with(&[secondary, primary, other], selected, 1, device_for),
        0
    );
    assert_eq!(
        preferred_taskbar_index_with(&[other, primary, secondary], selected, 0, device_for),
        2
    );
    assert_eq!(
        preferred_taskbar_index_with(&[primary, secondary], None, 99, device_for),
        1
    );
}

#[test]
fn recovery_settings_preserve_monitor_offset_anchor_and_visibility() {
    let settings = SettingsFile {
        monitor_device: Some(r"\\.\DISPLAY2".into()),
        tray_offset: 321,
        anchor_left: true,
        widget_visible: false,
        ..SettingsFile::default()
    };
    let restored: SettingsFile =
        serde_json::from_str(&serde_json::to_string(&settings).unwrap()).unwrap();
    assert_eq!(restored.monitor_device, settings.monitor_device);
    assert_eq!(restored.tray_offset, settings.tray_offset);
    assert_eq!(restored.anchor_left, settings.anchor_left);
    assert_eq!(restored.widget_visible, settings.widget_visible);
}

#[test]
fn taskbar_refresh_message_does_not_replace_existing_notifications() {
    assert!(![
        WM_APP_USAGE_UPDATED,
        WM_APP_TRAY,
        WM_APP_UPDATE_CHECK_COMPLETE,
        recovery_events::MESSAGE,
    ]
    .contains(&WM_APP_TASKBAR_REFRESH));
}

#[test]
fn live_widget_detects_and_repairs_hidden_empty_detached_and_off_parent_states() {
    // SAFETY: all HWNDs are private test windows, created and destroyed on this thread.
    unsafe {
        let parent = synthetic_taskbar();
        let child = synthetic_taskbar();
        let taskbar = native_interop::TaskbarWindow {
            hwnd: parent.0,
            rect: native_interop::get_window_rect_safe(parent.0).unwrap(),
            is_primary: true,
        };
        let restore = |phase| {
            native_interop::embed_in_taskbar(child.0, parent.0);
            let attached_ex = GetWindowLongW(child.0, GWL_EXSTYLE);
            native_interop::move_window(child.0, 20, 0, 80, 40);
            let moved_ex = GetWindowLongW(child.0, GWL_EXSTYLE);
            native_interop::set_widget_visibility(child.0, true).unwrap();
            assert!(
                !native_interop::taskbar_widget_needs_restore(child.0, &taskbar, true),
                "restore phase={phase} attached_ex={attached_ex:#x} moved_ex={moved_ex:#x} parent={:?} style={:#x} ex={:#x} rect={:?}",
                GetParent(child.0),
                GetWindowLongW(child.0, GWL_STYLE),
                GetWindowLongW(child.0, GWL_EXSTYLE),
                native_interop::get_window_rect_safe(child.0),
            );
        };
        restore(-1);
        for failure in 0..6 {
            match failure {
                0 => {
                    let _ = ShowWindow(child.0, SW_HIDE);
                }
                1 => native_interop::move_window(child.0, 20, 0, 0, 0),
                2 => native_interop::move_window(child.0, 900, 0, 80, 40),
                3 => {
                    SetParent(child.0, None).unwrap();
                }
                4 => {
                    let style = GetWindowLongW(child.0, GWL_EXSTYLE);
                    SetWindowLongW(child.0, GWL_EXSTYLE, style & !(WS_EX_LAYERED.0 as i32));
                }
                _ => {
                    let style = GetWindowLongW(child.0, GWL_STYLE) as u32;
                    SetWindowLongW(
                        child.0,
                        GWL_STYLE,
                        ((style & !WS_CHILD.0) | WS_POPUP.0) as i32,
                    );
                }
            }
            assert!(
                native_interop::taskbar_widget_needs_restore(child.0, &taskbar, true),
                "missed live-widget failure {failure}"
            );
            assert!(IsWindow(child.0).as_bool());
            restore(failure);
        }
    }
}

#[test]
fn automatic_restoration_preserves_hidden_preference_and_auto_hidden_parent() {
    // SAFETY: these hidden synthetic windows never interact with the real taskbar.
    unsafe {
        let parent = synthetic_taskbar();
        let child = synthetic_taskbar();
        let taskbar = native_interop::TaskbarWindow {
            hwnd: parent.0,
            rect: native_interop::get_window_rect_safe(parent.0).unwrap(),
            is_primary: true,
        };
        native_interop::embed_in_taskbar(child.0, parent.0);
        native_interop::move_window(child.0, 0, 0, 80, 40);
        native_interop::set_widget_visibility(child.0, false).unwrap();
        assert!(!native_interop::taskbar_widget_needs_restore(
            child.0, &taskbar, false
        ));
        assert!(native_interop::taskbar_widget_needs_restore(
            child.0, &taskbar, true
        ));
        native_interop::set_widget_visibility(child.0, true).unwrap();
        assert!(!IsWindowVisible(child.0).as_bool());
        assert_eq!(GetWindowLongW(parent.0, GWL_STYLE) as u32 & WS_VISIBLE.0, 0);
        assert!(!native_interop::taskbar_widget_needs_restore(
            child.0, &taskbar, true
        ));
        // An auto-hide taskbar moves outside its logical appbar rectangle.
        native_interop::move_window(parent.0, 0, -80, 800, 40);
        assert!(!native_interop::taskbar_widget_needs_restore(
            child.0, &taskbar, true
        ));
        native_interop::set_widget_visibility(child.0, false).unwrap();
        assert!(!native_interop::taskbar_widget_needs_restore(
            child.0, &taskbar, false
        ));
    }
}

#[test]
fn wake_display_changes_and_position_reset_queue_the_same_restoration_path() {
    // No AppState is initialized in unit tests: no provider requests or settings writes.
    assert!(lock_state().is_none());
    // SAFETY: WndProc is exercised with scalar messages and a private test HWND;
    // queued restoration messages are inspected, not dispatched against Explorer.
    unsafe {
        let target = synthetic_taskbar();
        for (message, command) in [
            (WM_POWERBROADCAST, PBT_APMRESUMEAUTOMATIC as usize),
            (WM_POWERBROADCAST, PBT_APMRESUMESUSPEND as usize),
            (WM_DISPLAYCHANGE, 0),
            (WM_COMMAND, IDM_RESET_POSITION as usize),
        ] {
            wnd_proc(target.0, message, WPARAM(command), LPARAM(0));
            let mut queued = MSG::default();
            assert!(PeekMessageW(
                &mut queued,
                target.0,
                WM_APP_TASKBAR_REFRESH,
                WM_APP_TASKBAR_REFRESH,
                PM_REMOVE,
            )
            .as_bool());
            assert_eq!(queued.wParam.0, 0);
            assert_eq!(queued.lParam.0, 0);
        }
        #[repr(C)]
        struct DisplayNotification {
            setting: windows::core::GUID,
            length: u32,
            value: u32,
        }
        let mut notification = DisplayNotification {
            setting: windows::core::GUID::from_u128(0x2b84c20e_ad23_4ddf_93db_05ffbd7efca5),
            length: 4,
            value: 0,
        };
        for value in [0, 1, 2] {
            notification.value = value;
            wnd_proc(
                target.0,
                WM_POWERBROADCAST,
                WPARAM(PBT_POWERSETTINGCHANGE as usize),
                LPARAM(&notification as *const DisplayNotification as isize),
            );
            let mut queued = MSG::default();
            let restored = PeekMessageW(
                &mut queued,
                target.0,
                WM_APP_TASKBAR_REFRESH,
                WM_APP_TASKBAR_REFRESH,
                PM_REMOVE,
            )
            .as_bool();
            assert_eq!(restored, value == 1);
            if restored {
                assert_eq!(queued.lParam.0, 0, "never queue the borrowed power pointer");
            }
        }
    }
}

#[test]
fn display_retries_are_bounded_but_startup_waits_for_delayed_taskbar() {
    let mut remaining = 3;
    assert!(taskbar_restore_retry_needed(&mut remaining, true));
    assert!(taskbar_restore_retry_needed(&mut remaining, true));
    assert!(!taskbar_restore_retry_needed(&mut remaining, true));
    assert!(!taskbar_restore_retry_needed(&mut remaining, true));
    let mut startup = 0;
    for _ in 0..10 {
        assert!(taskbar_restore_retry_needed(&mut startup, false));
    }
    assert!(!taskbar_restore_retry_needed(&mut startup, true));
}
