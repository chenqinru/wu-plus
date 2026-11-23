use cocoa::base::{BOOL, NO, YES, id, nil};
use cocoa::foundation::NSString;
use gpui::{App, PromptButton, PromptLevel};
use i18n::t;
use objc::{class, msg_send, runtime::Class, sel, sel_impl};
use settings::{Settings, SettingsStore};
use std::cell::Cell;
use workspace::Workspace;

thread_local! {
    // Sparkle's controller is confined to the main thread and lives for the app session.
    static CONTROLLER: Cell<id> = const { Cell::new(nil) };
}

#[derive(Clone, Copy, Debug, settings::RegisterSetting)]
struct AutomaticUpdates(bool);

impl Settings for AutomaticUpdates {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self(content.auto_update.unwrap_or(true))
    }
}

pub fn data_directory() -> &'static str {
    unsafe {
        let bundle: id = msg_send![class!(NSBundle), mainBundle];
        let key = NSString::alloc(nil).init_str("WuPlusUpdateTest");
        let value: id = msg_send![bundle, objectForInfoDictionaryKey: key];
        let _: () = msg_send![key, release];
        let test: BOOL = msg_send![value, boolValue];
        if test == YES {
            "Library/Application Support/Wu Plus Update Test"
        } else {
            "Library/Application Support/Wu Plus"
        }
    }
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|_, _: &auto_update::Check, window, cx| {
            let controller = CONTROLLER.get();
            if controller == nil {
                drop(window.prompt(
                    PromptLevel::Info,
                    &t!("Could not check for updates"),
                    Some(&t!("Auto-updates disabled for non-bundled app.")),
                    &[PromptButton::ok(t!("OK"))],
                    cx,
                ));
            } else {
                unsafe {
                    let _: () = msg_send![controller, checkForUpdates: nil];
                }
            }
        });
    })
    .detach();

    unsafe {
        let Some(controller) = load_controller() else {
            return;
        };
        CONTROLLER.set(controller);
        let _: () = msg_send![controller, startUpdater];
    }
    sync_setting(cx);
    cx.observe_global::<SettingsStore>(sync_setting).detach();
}

fn sync_setting(cx: &mut App) {
    let enabled = AutomaticUpdates::get_global(cx).0;
    unsafe {
        let updater: id = msg_send![CONTROLLER.get(), updater];
        let value = if enabled { YES } else { NO };
        let _: () = msg_send![updater, setAutomaticallyChecksForUpdates: value];
    }
}

unsafe fn load_controller() -> Option<id> {
    unsafe {
        let main_bundle: id = msg_send![class!(NSBundle), mainBundle];
        let feed_key = NSString::alloc(nil).init_str("SUFeedURL");
        let feed: id = msg_send![main_bundle, objectForInfoDictionaryKey: feed_key];
        let _: () = msg_send![feed_key, release];
        if feed == nil {
            return None;
        }

        let frameworks: id = msg_send![main_bundle, privateFrameworksPath];
        let filename = NSString::alloc(nil).init_str("Sparkle.framework");
        let path: id = msg_send![frameworks, stringByAppendingPathComponent: filename];
        let _: () = msg_send![filename, release];
        let bundle: id = msg_send![class!(NSBundle), bundleWithPath: path];
        let loaded: BOOL = msg_send![bundle, load];
        if loaded == NO {
            log::error!("Wu Plus could not load its Sparkle framework");
            return None;
        }

        let class = Class::get("SPUStandardUpdaterController")?;
        let controller: id = msg_send![class, alloc];
        let controller: id = msg_send![controller,
            initWithStartingUpdater:NO updaterDelegate:nil userDriverDelegate:nil
        ];
        (controller != nil).then_some(controller)
    }
}
