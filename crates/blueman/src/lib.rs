//! GTK3 module for Blueman's manager and applet. This library is loaded by GTK in those processes.

use std::{
    ffi::{CStr, c_char, c_int, c_uint, c_ulong, c_void},
    ptr,
    sync::Once,
};

// Keep the C GValue layout for indexing signal parameters. GLib reads the values.
#[repr(C)]
struct GValue {
    g_type: usize,
    data: [GValueData; 2],
}

#[repr(C)]
union GValueData {
    integer: i64,
    double: f64,
    pointer: *mut c_void,
}

type EmissionHook = unsafe extern "C" fn(*mut c_void, c_uint, *const GValue, *mut c_void) -> c_int;
type KeyPressed = unsafe extern "C" fn(*mut c_void, *const c_void, *mut c_void) -> c_int;
type SourceFunc = unsafe extern "C" fn(*mut c_void) -> c_int;

const NOTIFICATION_TIMEOUT_MS: c_uint = 5000;
const FIRST_ACTION_RESPONSE: c_int = 100;
const RESPONSE_DELETE_EVENT: c_int = -4;
const NOTIFICATION_TIMER: &CStr = c"hype-blueman-notification-timer";
const NOTIFICATION_ESCAPE: &CStr = c"hype-blueman-notification-escape";

#[link(name = "gtk-3")]
unsafe extern "C" {
    fn gtk_application_get_type() -> usize;
    fn gtk_window_get_application(window: *mut c_void) -> *mut c_void;
    fn gtk_accelerator_get_default_mod_mask() -> c_uint;
    fn gtk_widget_get_type() -> usize;
    fn gtk_message_dialog_get_type() -> usize;
    fn gtk_widget_get_name(widget: *mut c_void) -> *const c_char;
    fn gtk_widget_get_visible(widget: *mut c_void) -> c_int;
    fn gtk_window_get_title(window: *mut c_void) -> *const c_char;
    fn gtk_dialog_get_widget_for_response(dialog: *mut c_void, response: c_int) -> *mut c_void;
    fn gtk_dialog_response(dialog: *mut c_void, response: c_int);
}

#[link(name = "gdk-3")]
unsafe extern "C" {
    fn gdk_event_get_keyval(event: *const c_void, keyval: *mut c_uint) -> c_int;
    fn gdk_event_get_state(event: *const c_void, state: *mut c_uint) -> c_int;
    fn gdk_keyval_from_name(name: *const c_char) -> c_uint;
}

#[link(name = "gobject-2.0")]
unsafe extern "C" {
    fn g_type_class_ref(g_type: usize) -> *mut c_void;
    fn g_type_class_unref(class: *mut c_void);
    fn g_signal_lookup(name: *const c_char, g_type: usize) -> c_uint;
    fn g_signal_add_emission_hook(
        id: c_uint,
        detail: c_uint,
        hook: EmissionHook,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_ulong;
    fn g_signal_connect_data(
        instance: *mut c_void,
        detailed_signal: *const c_char,
        handler: KeyPressed,
        data: *mut c_void,
        destroy: Option<unsafe extern "C" fn(*mut c_void, *mut c_void)>,
        flags: c_uint,
    ) -> c_ulong;
    fn g_value_get_object(value: *const GValue) -> *mut c_void;
    fn g_object_get_data(object: *mut c_void, key: *const c_char) -> *mut c_void;
    fn g_object_set_data(object: *mut c_void, key: *const c_char, data: *mut c_void);
    fn g_object_ref(object: *mut c_void) -> *mut c_void;
    fn g_object_unref(object: *mut c_void);
    fn g_type_check_instance_is_a(instance: *mut c_void, g_type: usize) -> c_int;
}

#[link(name = "glib-2.0")]
unsafe extern "C" {
    fn g_timeout_add_full(
        priority: c_int,
        interval: c_uint,
        function: SourceFunc,
        data: *mut c_void,
        notify: Option<unsafe extern "C" fn(*mut c_void)>,
    ) -> c_uint;
    fn g_source_remove(id: c_uint) -> c_int;
}

#[link(name = "gio-2.0")]
unsafe extern "C" {
    fn g_application_get_application_id(application: *mut c_void) -> *const c_char;
    fn g_action_group_activate_action(
        application: *mut c_void,
        action: *const c_char,
        parameter: *mut c_void,
    );
}

/// Entry point documented in GTK3's gtkmodules.h.
#[unsafe(no_mangle)]
pub extern "C" fn gtk_module_init(_argc: *mut c_int, _argv: *mut *mut *mut c_char) {
    static INITIALIZE: Once = Once::new();
    INITIALIZE.call_once(|| {
        // Initialize the class so its window-added signal is registered before lookup.
        unsafe {
            let application_type = gtk_application_get_type();
            let class = g_type_class_ref(application_type);
            let signal = g_signal_lookup(c"window-added".as_ptr(), application_type);
            if signal != 0 {
                g_signal_add_emission_hook(signal, 0, window_added, ptr::null_mut(), None);
            }
            g_type_class_unref(class);
            let widget_type = gtk_widget_get_type();
            let class = g_type_class_ref(widget_type);
            let signal = g_signal_lookup(c"map".as_ptr(), widget_type);
            if signal != 0 {
                g_signal_add_emission_hook(signal, 0, widget_mapped, ptr::null_mut(), None);
            }
            g_type_class_unref(class);
        }
    });
}

unsafe extern "C" fn window_added(
    _hint: *mut c_void,
    count: c_uint,
    values: *const GValue,
    _data: *mut c_void,
) -> c_int {
    if count == 2 && !values.is_null() {
        // GTK emits the GtkApplication instance and its added window.
        unsafe {
            let application = g_value_get_object(values);
            let id = g_application_get_application_id(application);
            if !id.is_null() && CStr::from_ptr(id) == c"org.blueman.Manager" {
                let window = g_value_get_object(values.add(1));
                let marker = c"hype-blueman-escape".as_ptr();
                if g_object_get_data(window, marker).is_null() {
                    g_signal_connect_data(
                        window,
                        c"key-press-event".as_ptr(),
                        key_pressed,
                        ptr::null_mut(),
                        None,
                        0,
                    );
                    g_object_set_data(window, marker, window);
                }
            }
        }
    }
    1
}

unsafe extern "C" fn widget_mapped(
    _hint: *mut c_void,
    count: c_uint,
    values: *const GValue,
    _data: *mut c_void,
) -> c_int {
    if count >= 1 && !values.is_null() {
        unsafe {
            let dialog = g_value_get_object(values);
            if dialog.is_null()
                || g_type_check_instance_is_a(dialog, gtk_message_dialog_get_type()) == 0
                || CStr::from_ptr(gtk_widget_get_name(dialog)) != c"NotificationDialog"
            {
                return 1;
            }
            if g_object_get_data(dialog, NOTIFICATION_ESCAPE.as_ptr()).is_null() {
                g_signal_connect_data(
                    dialog,
                    c"key-press-event".as_ptr(),
                    notification_key_pressed,
                    ptr::null_mut(),
                    None,
                    0,
                );
                g_object_set_data(dialog, NOTIFICATION_ESCAPE.as_ptr(), dialog);
            }
            if gtk_dialog_get_widget_for_response(dialog, FIRST_ACTION_RESPONSE).is_null()
                && !is_pairing_code(dialog)
            {
                let pending = g_object_get_data(dialog, NOTIFICATION_TIMER.as_ptr()) as usize;
                if pending != 0 {
                    g_source_remove(pending as c_uint);
                }
                let timer = g_timeout_add_full(
                    0,
                    NOTIFICATION_TIMEOUT_MS,
                    close_notification,
                    g_object_ref(dialog),
                    Some(g_object_unref),
                );
                g_object_set_data(
                    dialog,
                    NOTIFICATION_TIMER.as_ptr(),
                    timer as usize as *mut c_void,
                );
            }
        }
    }
    1
}

unsafe fn is_pairing_code(dialog: *mut c_void) -> bool {
    unsafe {
        let title = gtk_window_get_title(dialog);
        !title.is_null() && CStr::from_ptr(title) == c"Bluetooth"
    }
}

unsafe extern "C" fn close_notification(dialog: *mut c_void) -> c_int {
    unsafe {
        g_object_set_data(dialog, NOTIFICATION_TIMER.as_ptr(), ptr::null_mut());
        if gtk_widget_get_visible(dialog) != 0 {
            gtk_dialog_response(dialog, RESPONSE_DELETE_EVENT);
        }
    }
    0
}

unsafe extern "C" fn notification_key_pressed(
    dialog: *mut c_void,
    event: *const c_void,
    _data: *mut c_void,
) -> c_int {
    unsafe {
        if !is_plain_escape(event) {
            return 0;
        }
        gtk_dialog_response(dialog, RESPONSE_DELETE_EVENT);
    }
    1
}

unsafe fn is_plain_escape(event: *const c_void) -> bool {
    let mut key = 0;
    let mut modifiers = 0;
    unsafe {
        gdk_event_get_keyval(event, &mut key) != 0
            && key == gdk_keyval_from_name(c"Escape".as_ptr())
            && gdk_event_get_state(event, &mut modifiers) != 0
            && modifiers & gtk_accelerator_get_default_mod_mask() == 0
    }
}

unsafe extern "C" fn key_pressed(
    window: *mut c_void,
    event: *const c_void,
    _data: *mut c_void,
) -> c_int {
    unsafe {
        if !is_plain_escape(event) {
            return 0;
        }
        let application = gtk_window_get_application(window);
        if application.is_null() {
            return 0;
        }
        g_action_group_activate_action(application, c"Quit".as_ptr(), ptr::null_mut());
    }
    1
}
