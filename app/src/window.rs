//! Native macOS shell: one NSWindow with a WKWebView pointing at the local server, plus the menu bar.

use block2::DynBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject, NSObjectProtocol, ProtocolObject, Sel};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSAppearance, NSAppearanceCustomization, NSAppearanceNameDarkAqua, NSApplication, NSApplicationActivationPolicy,
    NSApplicationDelegate, NSApplicationTerminateReply, NSBackingStoreType, NSColor, NSEventModifierFlags,
    NSMenu, NSMenuItem, NSWindow, NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::{
    MainThreadMarker, NSError, NSNotification, NSNumber, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLRequest, ns_string,
};
use objc2_web_kit::{WKScriptMessage, WKScriptMessageHandler, WKUserContentController, WKWebView, WKWebViewConfiguration};
use sushload_core::Engine;
use std::cell::OnceCell;
use std::path::PathBuf;
use std::sync::OnceLock;

static ENGINE: OnceLock<Engine> = OnceLock::new();
static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

#[derive(Default)]
struct Ivars {
    window: OnceCell<Retained<NSWindow>>,
    web: OnceCell<Retained<WKWebView>>,
    url: OnceCell<String>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements and Delegate does not implement Drop.
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Delegate;

    unsafe impl NSObjectProtocol for Delegate {}

    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _n: &NSNotification) {
            let mtm = self.mtm();
            let app = NSApplication::sharedApplication(mtm);
            app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
            build_menu(mtm, &app, self);
            self.make_window(mtm);
            #[allow(deprecated)]
            app.activateIgnoringOtherApps(true);
        }

        #[unsafe(method(applicationShouldHandleReopen:hasVisibleWindows:))]
        fn should_reopen(&self, _app: &NSApplication, _visible: bool) -> bool {
            self.show_window();
            false
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn terminate_after_last(&self, _app: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _app: &NSApplication) -> NSApplicationTerminateReply {
            // Stop transfers and flush the state file before the process goes away.
            if let Some(e) = ENGINE.get() {
                e.shutdown();
            }
            if let Some(d) = DATA_DIR.get() {
                let _ = std::fs::remove_file(d.join("instance.json"));
            }
            NSApplicationTerminateReply::TerminateNow
        }
    }

    unsafe impl WKScriptMessageHandler for Delegate {
        #[unsafe(method(userContentController:didReceiveScriptMessage:))]
        fn did_receive(&self, _c: &WKUserContentController, message: &WKScriptMessage) {
            let body = unsafe { message.body() };
            let Ok(s) = body.downcast::<NSString>() else { return };
            let Some(w) = self.ivars().window.get() else { return };
            match s.to_string().as_str() {
                "drag" => {
                    let app = NSApplication::sharedApplication(self.mtm());
                    if let Some(ev) = app.currentEvent() {
                        w.performWindowDragWithEvent(&ev);
                    }
                }
                "zoom" => w.performZoom(None),
                _ => {}
            }
        }
    }

    impl Delegate {
        #[unsafe(method(openSettings:))]
        fn open_settings(&self, _sender: Option<&AnyObject>) {
            self.show_window();
            self.eval("window.sushloadGo && window.sushloadGo('settings')");
        }

        #[unsafe(method(selectAllRows:))]
        fn select_all_rows(&self, _sender: Option<&AnyObject>) {
            self.eval("window.sushloadSelectAll && window.sushloadSelectAll()");
        }

        #[unsafe(method(showMainWindow:))]
        fn show_main_window(&self, _sender: Option<&AnyObject>) {
            self.show_window();
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, url: String) -> Retained<Self> {
        let ivars = Ivars::default();
        let _ = ivars.url.set(url);
        let this = Self::alloc(mtm).set_ivars(ivars);
        // SAFETY: NSObject's init has the expected signature.
        unsafe { msg_send![super(this), init] }
    }

    fn eval(&self, js: &str) {
        if let Some(web) = self.ivars().web.get() {
            let none: Option<&DynBlock<dyn Fn(*mut AnyObject, *mut NSError)>> = None;
            // SAFETY: plain JS evaluation with no completion handler.
            unsafe { web.evaluateJavaScript_completionHandler(&NSString::from_str(js), none) };
        }
    }

    fn show_window(&self) {
        if let Some(w) = self.ivars().window.get() {
            if w.isMiniaturized() {
                w.deminiaturize(None);
            }
            w.makeKeyAndOrderFront(None);
            #[allow(deprecated)]
            NSApplication::sharedApplication(self.mtm()).activateIgnoringOtherApps(true);
        }
    }

    fn make_window(&self, mtm: MainThreadMarker) {
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable
            | NSWindowStyleMask::FullSizeContentView;
        // SAFETY: released-when-closed is disabled right below, as required outside a controller.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1100.0, 700.0)),
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(ns_string!("sushLoad"));
        window.setTitlebarAppearsTransparent(true);
        window.setTitleVisibility(NSWindowTitleVisibility::Hidden);
        window.setContentMinSize(NSSize::new(820.0, 520.0));
        let bg = NSColor::colorWithSRGBRed_green_blue_alpha(30.0 / 255.0, 30.0 / 255.0, 32.0 / 255.0, 1.0);
        window.setBackgroundColor(Some(&bg));
        if let Some(dark) = NSAppearance::appearanceNamed(unsafe { NSAppearanceNameDarkAqua }) {
            window.setAppearance(Some(&dark));
        }

        let config = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe {
            config
                .userContentController()
                .addScriptMessageHandler_name(ProtocolObject::from_ref(self), ns_string!("sushload"));
        }
        let web = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(1100.0, 700.0)),
                &config,
            )
        };
        unsafe {
            // No white flash while the page loads.
            let no: Retained<AnyObject> = Retained::cast_unchecked(NSNumber::numberWithBool(false));
            let _: () = msg_send![&*web, setValue: &*no, forKey: ns_string!("drawsBackground")];
            web.setUnderPageBackgroundColor(Some(&bg));
        }
        if let Some(url) = self.ivars().url.get()
            && let Some(u) = NSURL::URLWithString(&NSString::from_str(url))
        {
            let req = NSURLRequest::requestWithURL(&u);
            unsafe { web.loadRequest(&req) };
        }
        window.setContentView(Some(&web));
        window.center();
        window.makeKeyAndOrderFront(None);
        let _ = self.ivars().web.set(web);
        let _ = self.ivars().window.set(window);
    }
}

fn menu_item(
    mtm: MainThreadMarker,
    title: &str,
    action: Option<Sel>,
    key: &str,
    mods: NSEventModifierFlags,
    target: Option<&AnyObject>,
) -> Retained<NSMenuItem> {
    // SAFETY: valid title/selector/key strings.
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(
            NSMenuItem::alloc(mtm),
            &NSString::from_str(title),
            action,
            &NSString::from_str(key),
        )
    };
    item.setKeyEquivalentModifierMask(mods);
    if let Some(t) = target {
        unsafe { item.setTarget(Some(t)) };
    }
    item
}

fn submenu(mtm: MainThreadMarker, title: &str, items: &[Retained<NSMenuItem>]) -> Retained<NSMenuItem> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), &NSString::from_str(title));
    for i in items {
        menu.addItem(i);
    }
    let holder = NSMenuItem::new(mtm);
    holder.setSubmenu(Some(&menu));
    holder
}

fn build_menu(mtm: MainThreadMarker, app: &NSApplication, delegate: &Delegate) {
    let cmd = NSEventModifierFlags::Command;
    let none = NSEventModifierFlags::empty();
    let target: &AnyObject = delegate;
    let sep = || NSMenuItem::separatorItem(mtm);

    let app_menu = submenu(
        mtm,
        "sushLoad",
        &[
            menu_item(mtm, "About sushLoad", Some(sel!(orderFrontStandardAboutPanel:)), "", none, None),
            sep(),
            menu_item(mtm, "Settings…", Some(sel!(openSettings:)), ",", cmd, Some(target)),
            sep(),
            menu_item(mtm, "Hide sushLoad", Some(sel!(hide:)), "h", cmd, None),
            menu_item(mtm, "Hide Others", Some(sel!(hideOtherApplications:)), "h", cmd | NSEventModifierFlags::Option, None),
            menu_item(mtm, "Show All", Some(sel!(unhideAllApplications:)), "", none, None),
            sep(),
            menu_item(mtm, "Quit sushLoad", Some(sel!(terminate:)), "q", cmd, None),
        ],
    );
    let edit_menu = submenu(
        mtm,
        "Edit",
        &[
            menu_item(mtm, "Undo", Some(sel!(undo:)), "z", cmd, None),
            menu_item(mtm, "Redo", Some(sel!(redo:)), "z", cmd | NSEventModifierFlags::Shift, None),
            sep(),
            menu_item(mtm, "Cut", Some(sel!(cut:)), "x", cmd, None),
            menu_item(mtm, "Copy", Some(sel!(copy:)), "c", cmd, None),
            menu_item(mtm, "Paste", Some(sel!(paste:)), "v", cmd, None),
            menu_item(mtm, "Select All", Some(sel!(selectAllRows:)), "a", cmd, Some(target)),
        ],
    );
    let window_menu = submenu(
        mtm,
        "Window",
        &[
            menu_item(mtm, "Minimize", Some(sel!(performMiniaturize:)), "m", cmd, None),
            menu_item(mtm, "Zoom", Some(sel!(performZoom:)), "", none, None),
            sep(),
            menu_item(mtm, "sushLoad", Some(sel!(showMainWindow:)), "0", cmd, Some(target)),
        ],
    );
    let main = NSMenu::new(mtm);
    main.addItem(&app_menu);
    main.addItem(&edit_menu);
    main.addItem(&window_menu);
    if let Some(m) = window_menu.submenu() {
        app.setWindowsMenu(Some(&m));
    }
    app.setMainMenu(Some(&main));
}

/// Runs the Cocoa event loop; never returns (Quit ends the process from the delegate).
pub fn run(engine: Engine, url: String) {
    let _ = ENGINE.set(engine);
    if let Some(home) = std::env::var_os("HOME") {
        let _ = DATA_DIR.set(PathBuf::from(home).join("Library/Application Support/sushLoad"));
    }
    let mtm = MainThreadMarker::new().expect("must run on the main thread");
    let app = NSApplication::sharedApplication(mtm);
    let delegate = Delegate::new(mtm, url);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    app.run();
}
