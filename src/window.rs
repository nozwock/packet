use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use anyhow::{Context, anyhow};
use ashpd::desktop::background::Background;
use ashpd::desktop::notification::NotificationProxy;
use formatx::formatx;
use futures_lite::StreamExt;
use gettextrs::{gettext, ngettext};
use gtk::gio::FILE_ATTRIBUTE_STANDARD_SIZE;
use gtk::glib::clone;
use gtk::{gdk, gio, glib};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::application::PacketApplication;
use crate::config::{APP_ID, PROFILE};
use crate::constants::packet_log_path;
use crate::ext::MessageExt;
use crate::objects::{self, SendRequestState};
use crate::objects::{TransferState, UserAction};
use crate::plugins::{FileBasedPlugin, NautilusPlugin, Plugin};
use crate::utils::{strip_user_home_prefix, with_signals_blocked, xdg_download_with_fallback};
use crate::{monitors, tokio_runtime, widgets};

#[derive(Debug)]
pub enum LoopingTaskHandle {
    Tokio(tokio::task::JoinHandle<()>),
    Glib(glib::JoinHandle<()>),
}

#[derive(Debug, Clone)]
pub struct ReceiveTransferCache {
    pub transfer_id: String,
    pub notification_id: String,
    pub state: objects::ReceiveTransferState,
    pub auto_decline_ctk: CancellationToken,
}

/// Simple URL detection heuristic
fn is_likely_url(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed.starts_with("http://")
        || trimmed.starts_with("https://")
        || trimmed.starts_with("www.")
}

mod imp {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
        sync::Arc,
    };

    use tokio::sync::Mutex;

    use crate::{ext::MessageExt, utils::remove_notification};

    use super::*;

    #[derive(gtk::CompositeTemplate, better_default::Default)]
    #[template(resource = "/io/github/nozwock/Packet/ui/window.ui")]
    pub struct PacketApplicationWindow {
        #[default(gio::Settings::new(APP_ID))]
        pub settings: gio::Settings,

        #[template_child]
        pub preferences_dialog: TemplateChild<adw::PreferencesDialog>,

        #[template_child]
        pub help_dialog: TemplateChild<adw::Dialog>,

        #[template_child]
        pub root_stack: TemplateChild<gtk::Stack>,

        #[template_child]
        pub rqs_error_copy_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub rqs_error_retry_button: TemplateChild<gtk::Button>,

        #[template_child]
        pub toast_overlay: TemplateChild<adw::ToastOverlay>,

        #[template_child]
        pub main_nav_view: TemplateChild<adw::NavigationView>,

        #[template_child]
        pub bottom_bar_image: TemplateChild<gtk::Image>,
        #[template_child]
        pub bottom_bar_title: TemplateChild<gtk::Label>,
        #[template_child]
        pub bottom_bar_caption: TemplateChild<gtk::Label>,
        #[template_child]
        pub bottom_bar_spacer: TemplateChild<adw::Bin>,
        #[template_child]
        pub bottom_bar_status: TemplateChild<gtk::Box>,
        #[template_child]
        pub bottom_bar_status_top: TemplateChild<gtk::Box>,

        #[template_child]
        pub device_name_entry: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub device_visibility_switch: TemplateChild<adw::SwitchRow>,
        #[template_child]
        pub static_port_expander: TemplateChild<adw::ExpanderRow>,
        #[template_child]
        pub static_port_entry: TemplateChild<adw::EntryRow>,
        #[template_child]
        pub download_folder_row: TemplateChild<adw::ActionRow>,
        #[template_child]
        pub download_folder_pick_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub run_in_background_switch: TemplateChild<adw::SwitchRow>,
        pub run_in_background_switch_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        #[template_child]
        pub auto_start_switch: TemplateChild<adw::SwitchRow>,
        pub auto_start_switch_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        #[template_child]
        pub nautilus_plugin_switch: TemplateChild<adw::SwitchRow>,
        pub nautilus_plugin_switch_handler_id: RefCell<Option<glib::SignalHandlerId>>,
        #[template_child]
        pub tray_icon_group: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub tray_icon_switch: TemplateChild<adw::SwitchRow>,

        #[template_child]
        pub main_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub main_nav_content: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub main_add_files_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub main_share_text_button: TemplateChild<gtk::Button>,

        #[template_child]
        pub share_text_dialog: TemplateChild<adw::Dialog>,
        #[template_child]
        pub paste_text_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub send_text_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub share_text_view: TemplateChild<gtk::TextView>,
        #[template_child]
        pub share_text_counter: TemplateChild<gtk::Label>,

        #[template_child]
        pub manage_files_nav_content: TemplateChild<gtk::Box>,
        #[template_child]
        pub manage_files_header: TemplateChild<adw::PreferencesGroup>,
        #[template_child]
        pub manage_files_add_files_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub manage_files_send_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub manage_files_listbox: TemplateChild<gtk::ListBox>,
        #[default(gio::ListStore::new::<gio::File>())]
        pub manage_files_model: gio::ListStore,

        #[template_child]
        pub select_recipients_dialog: TemplateChild<adw::Dialog>,
        #[template_child]
        pub select_recipient_refresh_button: TemplateChild<gtk::Button>,
        #[template_child]
        pub recipient_listbox: TemplateChild<gtk::ListBox>,
        #[template_child]
        pub loading_recipients_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub recipients_help_button: TemplateChild<gtk::LinkButton>,
        #[default(gio::ListStore::new::<SendRequestState>())]
        pub recipient_model: gio::ListStore,

        pub send_transfers_id_cache: Arc<Mutex<HashMap<String, SendRequestState>>>, // id, state
        pub receive_transfer_cache: Arc<Mutex<Option<ReceiveTransferCache>>>,

        #[default(gio::NetworkMonitor::default())]
        pub network_monitor: gio::NetworkMonitor,
        pub dbus_system_conn: Rc<RefCell<Option<zbus::Connection>>>,
        // Would do unwrap_or_default anyways, so keeping it as just bool
        pub network_state: Rc<Cell<bool>>,
        pub bluetooth_state: Rc<Cell<bool>>,

        // FIXME: use this to receive network state on send/receive transfers, to cancel them
        // on connection loss
        pub network_state_sender: Arc<Mutex<Option<tokio::sync::broadcast::Sender<bool>>>>,

        // RQS State
        pub rqs: Arc<Mutex<Option<rqs_lib::RQS>>>,
        pub file_sender: Arc<Mutex<Option<tokio::sync::mpsc::Sender<rqs_lib::SendInfo>>>>,
        pub ble_receiver: Arc<Mutex<Option<tokio::sync::broadcast::Receiver<()>>>>,
        pub mdns_discovery_broadcast_tx:
            Arc<Mutex<Option<tokio::sync::broadcast::Sender<rqs_lib::EndpointInfo>>>>,
        pub is_mdns_discovery_on: Rc<Cell<bool>>,

        pub looping_async_tasks: RefCell<Vec<LoopingTaskHandle>>,

        pub is_background_allowed: Cell<bool>,
        pub should_quit: Cell<bool>,

        pub is_recipients_dialog_opened: Cell<bool>,

        pub pending_text: RefCell<Option<(String, rqs_lib::TextPayloadType)>>,

        pub nautilus_plugin: NautilusPlugin,

        #[cfg(target_os = "linux")]
        pub tray_icon_handle: RefCell<Option<ksni::Handle<crate::tray::Tray>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PacketApplicationWindow {
        const NAME: &'static str = "PacketApplicationWindow";
        type Type = super::PacketApplicationWindow;
        type ParentType = adw::ApplicationWindow;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        // You must call `Widget`'s `init_template()` within `instance_init()`.
        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PacketApplicationWindow {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();

            // Devel Profile
            if PROFILE == "Devel" {
                obj.add_css_class("devel");
            }

            // Load latest window state
            obj.load_window_size();
            obj.load_app_state();
            obj.setup_gactions();
            obj.setup_preferences();
            #[cfg(target_os = "linux")]
            obj.setup_tray_icon();
            obj.setup_ui();
            obj.setup_connection_monitors();
            obj.setup_notification_actions_monitor();
            obj.setup_rqs_service();
            obj.request_background_at_start();
        }
    }

    impl WidgetImpl for PacketApplicationWindow {}
    impl WindowImpl for PacketApplicationWindow {
        // Save window state on delete event
        fn close_request(&self) -> glib::Propagation {
            if self.is_background_allowed.get()
                && self.settings.boolean("run-in-background")
                && !self.should_quit.get()
            {
                tracing::info!("Running Packet in background");
                self.obj().set_visible(false);
                return glib::Propagation::Stop;
            }

            tracing::debug!("GtkApplicationWindow<PacketApplicationWindow>::close");

            if let Err(err) = self.obj().save_window_size() {
                tracing::warn!("Failed to save window state, {}", &err);
            }
            if let Err(err) = self.obj().save_app_state() {
                tracing::warn!("Failed to save app state, {}", &err);
            }

            if let Some(cached_transfer) = self.receive_transfer_cache.blocking_lock().as_ref() {
                use rqs_lib::TransferState;
                match cached_transfer
                    .state
                    .event()
                    .unwrap()
                    .msg
                    .as_client_unchecked()
                    .state
                    .as_ref()
                    .unwrap_or(&TransferState::Initial)
                {
                    TransferState::Disconnected
                    | TransferState::Rejected
                    | TransferState::Cancelled
                    | TransferState::Finished => {}
                    _ => {
                        remove_notification(cached_transfer.notification_id.clone());
                    }
                }
            }

            // Abort all looping tasks before closing
            tracing::info!(
                count = self.looping_async_tasks.borrow().len(),
                "Cancelling looping tasks"
            );
            while let Some(join_handle) = self.looping_async_tasks.borrow_mut().pop() {
                match join_handle {
                    LoopingTaskHandle::Tokio(join_handle) => join_handle.abort(),
                    LoopingTaskHandle::Glib(join_handle) => join_handle.abort(),
                }
            }

            let (tx, rx) = async_channel::bounded(1);
            tokio_runtime().spawn(clone!(
                #[weak(rename_to = rqs)]
                self.rqs,
                async move {
                    {
                        tracing::info!("Stopping RQS service");
                        let mut rqs_guard = rqs.lock().await;
                        if let Some(rqs) = rqs_guard.as_mut() {
                            rqs.stop().await;
                        }
                    }

                    tx.send(()).await.unwrap();
                }
            ));

            rx.recv_blocking().unwrap();

            // Pass close request on to the parent
            self.parent_close_request()
        }
    }

    impl ApplicationWindowImpl for PacketApplicationWindow {}
    impl AdwApplicationWindowImpl for PacketApplicationWindow {}
}

glib::wrapper! {
    pub struct PacketApplicationWindow(ObjectSubclass<imp::PacketApplicationWindow>)
        @extends gtk::Widget, gtk::Window, gtk::ApplicationWindow, adw::ApplicationWindow,
        @implements gio::ActionGroup, gio::ActionMap, gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget,
        gtk::Native, gtk::Root, gtk::ShortcutManager;
}

impl PacketApplicationWindow {
    pub fn new(app: &PacketApplication) -> Self {
        glib::Object::builder().property("application", app).build()
    }

    fn save_window_size(&self) -> Result<(), glib::BoolError> {
        let imp = self.imp();

        let (width, height) = self.default_size();

        imp.settings.set_int("window-width", width)?;
        imp.settings.set_int("window-height", height)?;

        imp.settings
            .set_boolean("is-maximized", self.is_maximized())?;

        Ok(())
    }

    fn load_window_size(&self) {
        let imp = self.imp();

        let width = imp.settings.int("window-width");
        let height = imp.settings.int("window-height");
        let is_maximized = imp.settings.boolean("is-maximized");

        self.set_default_size(width, height);

        if is_maximized {
            self.maximize();
        }
    }

    fn save_app_state(&self) -> Result<(), glib::BoolError> {
        let imp = self.imp();

        imp.settings
            .set_string("device-name", imp.device_name_entry.text().as_str())?;

        Ok(())
    }

    fn load_app_state(&self) {
        let imp = self.imp();
        if imp.settings.string("download-folder").is_empty() {
            imp.settings
                .set_string(
                    "download-folder",
                    xdg_download_with_fallback().to_str().unwrap(),
                )
                .unwrap();
        }

        imp.settings
            .bind(
                "enable-static-port",
                &imp.static_port_expander.get(),
                "enable-expansion",
            )
            .build();
        imp.static_port_entry
            .set_text(&imp.settings.int("static-port-number").to_string());
    }

    fn setup_gactions(&self) {
        let preferences_dialog = gio::ActionEntry::builder("preferences")
            .activate(move |win: &Self, _, _| {
                win.imp()
                    .preferences_dialog
                    .present(win.root().and_downcast_ref::<adw::ApplicationWindow>());
            })
            .build();

        let received_files = gio::ActionEntry::builder("received-files")
            .activate(move |win: &Self, _, _| {
                // Open current download folder
                gtk::FileLauncher::new(Some(&gio::File::for_path(
                    win.imp().settings.string("download-folder"),
                )))
                .launch(
                    win.root().and_downcast::<adw::ApplicationWindow>().as_ref(),
                    None::<&gio::Cancellable>,
                    move |_| {},
                )
            })
            .build();

        let help_dialog = gio::ActionEntry::builder("help")
            .activate(move |win: &Self, _, _| {
                win.imp()
                    .help_dialog
                    .present(win.root().and_downcast_ref::<adw::ApplicationWindow>());
            })
            .build();

        let pick_download_folder = gio::ActionEntry::builder("pick-download-folder")
            .activate(move |win: &Self, _, _| {
                win.pick_download_folder();
            })
            .build();

        self.add_action_entries([
            preferences_dialog,
            received_files,
            help_dialog,
            pick_download_folder,
        ]);
    }

    fn add_toast(&self, msg: &str) {
        self.imp().toast_overlay.add_toast(adw::Toast::new(msg));
    }

    fn get_device_name_state(&self) -> glib::GString {
        self.imp().settings.string("device-name")
    }

    fn set_device_name_state(&self, s: &str) -> Result<(), glib::BoolError> {
        self.imp().settings.set_string("device-name", s)
    }

    fn setup_preferences(&self) {
        let imp = self.imp();

        imp.device_visibility_switch
            .set_active(imp.settings.boolean("device-visibility"));
        imp.settings
            .bind(
                "device-visibility",
                &imp.device_visibility_switch.get(),
                "active",
            )
            .build();
        imp.settings
            .bind(
                "run-in-background",
                &imp.run_in_background_switch.get(),
                "active",
            )
            .build();
        imp.settings
            .bind("auto-start", &imp.auto_start_switch.get(), "active")
            .build();
        imp.settings
            .bind(
                "enable-nautilus-plugin",
                &imp.nautilus_plugin_switch.get(),
                "active",
            )
            .build();
        imp.settings
            .bind("enable-tray-icon", &imp.tray_icon_switch.get(), "active")
            .build();

        // TODO: The value of many preference options are only validated in the
        // UI, not outside of it.
        //
        // Incase the users modifies the setting value outside of the app to
        // something invalid, might want to take such a scenario into
        // consideration.
        //
        // For example the port value with non-numeric value, display name with
        // non-UTF8 bytes, etc.

        let device_name = &self.get_device_name_state();
        let device_name_entry = imp.device_name_entry.get();
        {
            if device_name.is_empty() {
                let device_name = whoami::devicename();
                device_name_entry.set_text(&device_name);
                // Can't use bind, since that's not the behaviour we want
                // We need to keep a state of entry widget before apply so
                // that we can restore the name to what's actually being used
                self.set_device_name_state(&device_name).unwrap();
            } else {
                device_name_entry.set_text(device_name);
            }
        }

        if imp.settings.boolean("enable-nautilus-plugin") {
            // Update plugin
            // This takes care of cases of applying updates to the python extension
            // script as well as reinstalling it if it got removed for some reason.
            let plugin = imp.nautilus_plugin.clone();
            glib::spawn_future_local(clone!(
                #[weak]
                imp,
                async move {
                    let success = tokio_runtime()
                        .spawn_blocking(move || plugin.install_plugin())
                        .await
                        .map_err(|err| anyhow::anyhow!(err))
                        .and_then(|it| it)
                        .inspect_err(|err| tracing::error!("{err:#}"))
                        .is_ok();

                    if !success {
                        imp.obj()
                            .add_toast(&gettext("Couldn't update the Nautilus plugin"));
                    }
                }
            ));
        }

        let _signal_handle = imp.nautilus_plugin_switch.connect_active_notify(clone!(
            #[weak]
            imp,
            move |switch| {
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    #[weak]
                    switch,
                    async move {
                        switch.set_sensitive(false);

                        let enable_plugin = switch.is_active();

                        tracing::info!(enable_plugin, "Setting Nautilus plugin state");

                        let plugin = imp.nautilus_plugin.clone();
                        let success = tokio_runtime()
                            .spawn_blocking(move || {
                                if enable_plugin {
                                    plugin.install_plugin()
                                } else {
                                    plugin.uninstall_plugin()
                                }
                            })
                            .await
                            .map_err(|err| anyhow::anyhow!(err))
                            .and_then(|it| it)
                            .inspect_err(|err| tracing::error!("{err:#}"))
                            .is_ok();

                        if enable_plugin {
                            if success {
                                imp.obj().present_plugin_success_dialog();
                            } else {
                                imp.obj().present_plugin_error_dialog(
                                        NautilusPlugin::help_install_dir(),
                                    );
                                with_signals_blocked(
                                    &[(
                                        &switch,
                                        imp.nautilus_plugin_switch_handler_id.borrow().as_ref(),
                                    )],
                                    || {
                                        switch.set_active(false);
                                    },
                                );
                            }
                        }

                        switch.set_sensitive(true);
                    }
                ));
            }
        ));
        imp.nautilus_plugin_switch_handler_id
            .replace(Some(_signal_handle));

        #[cfg(target_os = "linux")]
        imp.tray_icon_switch.connect_active_notify(clone!(
            #[weak]
            imp,
            move |switch| {
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    #[weak]
                    switch,
                    async move {
                        switch.set_sensitive(false);

                        if switch.is_active() {
                            _ = imp.obj().enable_tray_icon().await;
                        } else {
                            imp.obj().disable_tray_icon().await;
                        }

                        switch.set_sensitive(true);
                    }
                ));
            }
        ));

        let _signal_handle = imp.run_in_background_switch.connect_active_notify(clone!(
            #[weak]
            imp,
            move |switch| {
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    #[weak]
                    switch,
                    async move {
                        switch.set_sensitive(false);

                        {
                            let is_run_in_background = switch.is_active();
                            tracing::info!(
                                is_active = is_run_in_background,
                                "Setting run in background"
                            );

                            let is_run_in_background_allowed = imp
                                .obj()
                                .portal_request_background()
                                .await
                                .map(|it| it.run_in_background())
                                .unwrap_or_default();

                            if is_run_in_background && !is_run_in_background_allowed {
                                imp.obj()
                                    .add_toast(&gettext("Packet cannot run in the background"));
                            }
                        }

                        switch.set_sensitive(true);
                    }
                ));
            }
        ));
        imp.run_in_background_switch_handler_id
            .replace(Some(_signal_handle));

        let _signal_handle = imp.auto_start_switch.connect_active_notify(clone!(
            #[weak]
            imp,
            move |switch| {
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    #[weak]
                    switch,
                    async move {
                        switch.set_sensitive(false);

                        {
                            let is_auto_start = switch.is_active();
                            tracing::info!(is_active = is_auto_start, "Setting auto-start");

                            let is_auto_start_allowed = imp
                                .obj()
                                .portal_request_background()
                                .await
                                .map(|it| it.auto_start())
                                .unwrap_or_default();

                            if is_auto_start && !is_auto_start_allowed {
                                imp.obj().add_toast(&gettext("Packet cannot run at login"));
                            }
                        }

                        switch.set_sensitive(true);
                    }
                ));
            }
        ));
        imp.auto_start_switch_handler_id
            .replace(Some(_signal_handle));

        let prev_validation_state = Rc::new(Cell::new(None));
        let changed_signal_handle = Rc::new(RefCell::new(None));
        imp.device_name_entry.connect_apply(clone!(
            #[weak(rename_to = this)]
            self,
            #[weak]
            prev_validation_state,
            move |entry| {
                entry.remove_css_class("success");
                prev_validation_state.set(None);

                let device_name = entry.text();
                let is_name_already_set = this.get_device_name_state() == device_name;
                if !is_name_already_set {
                    tracing::info!(?device_name, "Setting device name");

                    {
                        let imp = this.imp();

                        // Since transfers from this device to other devices will be affected,
                        // we won't proceed if they exist
                        if this.is_no_file_being_send() {
                            imp.preferences_dialog.close();

                            this.set_device_name_state(&device_name).unwrap();

                            glib::spawn_future_local(clone!(
                                #[weak]
                                this,
                                #[weak]
                                imp,
                                async move {
                                    _ = this.restart_rqs_service().await;

                                    // Restart mDNS discovery if it was on before the RQS service restart
                                    this.start_mdns_discovery(Some(imp.is_mdns_discovery_on.get()));
                                }
                            ));
                        } else {
                            // Although this should be unreacable with the current design, since
                            // the dialog locks out the user during an ongoing transfer and
                            // the user can't open preferences whatsoever in that state

                            imp.device_name_entry.set_show_apply_button(false);
                            imp.device_name_entry
                                .set_text(&this.get_device_name_state());
                            imp.device_name_entry.set_show_apply_button(true);

                            tracing::debug!("Active transfers found, can't rename device name");

                            imp.toast_overlay.add_toast(
                                adw::Toast::builder()
                                    .title(&gettext(
                                        "Can't rename device during an active transfer",
                                    ))
                                    .build(),
                            );
                        }
                    }

                    this.bottom_bar_status_indicator_ui_update(
                        this.imp().device_visibility_switch.is_active(),
                    );
                }
            }
        ));
        let _changed_signal_handle = imp.device_name_entry.connect_changed(clone!(
            #[strong]
            changed_signal_handle,
            #[strong]
            prev_validation_state,
            move |obj| {
                set_entry_validation_state(
                    &obj,
                    // Empty device names are not discoverable from other devices, they'll be
                    // filtered out as malformed.
                    !obj.text().trim().is_empty(),
                    &prev_validation_state,
                    changed_signal_handle.borrow().as_ref().unwrap(),
                );
            }
        ));
        *changed_signal_handle.as_ref().borrow_mut() = Some(_changed_signal_handle);

        /// `signal_handle` is the handle for the `changed` signal handler
        /// where this function should be called.
        ///
        /// Reset `prev_validation_state` to `None` in the `apply` signal.
        fn set_entry_validation_state(
            entry: &adw::EntryRow,
            is_valid: bool,
            prev_validation_state: &Rc<Cell<Option<bool>>>,
            signal_handle: &glib::signal::SignalHandlerId,
        ) {
            if is_valid {
                if prev_validation_state.get().is_none()
                    || !prev_validation_state.get().unwrap_or(true)
                {
                    // To emit `changed` only on valid/invalid state change,
                    // and not when the entry is valid and was valid previously
                    prev_validation_state.set(Some(true));

                    entry.add_css_class("success");
                    entry.remove_css_class("error");

                    entry.set_show_apply_button(true);
                    entry.block_signal(&signal_handle);
                    // `show-apply-button` becomes visible on `::changed` signal on
                    // the GtkText child of the AdwEntryRow, not the root widget itself.
                    // Hence, the GtkEditable delegate.
                    entry.delegate().unwrap().emit_by_name::<()>("changed", &[]);
                    entry.unblock_signal(&signal_handle);
                }
            } else {
                prev_validation_state.set(Some(false));

                entry.remove_css_class("success");
                entry.add_css_class("error");

                entry.set_show_apply_button(false);
            }
        }

        imp.static_port_expander
            .connect_enable_expansion_notify(clone!(
                #[weak]
                imp,
                move |obj| {
                    glib::spawn_future_local(clone!(
                        #[weak]
                        obj,
                        async move {
                            let port_number = imp.settings.int("static-port-number");
                            if obj.enables_expansion()
                                && Some(port_number as u32)
                                    != imp.rqs.lock().await.as_ref().unwrap().port_number
                            {
                                tracing::info!(port_number, "Setting custom static port");

                                // FIXME: maybe just make the widget insensitive
                                // for the duration of the service restart instead
                                imp.preferences_dialog.close();

                                _ = imp.obj().restart_rqs_service().await;
                            }
                        }
                    ));
                }
            ));

        let prev_validation_state = Rc::new(Cell::new(None));
        let changed_signal_handle = Rc::new(RefCell::new(None));
        imp.static_port_entry.connect_apply(clone!(
            #[weak]
            imp,
            #[weak]
            prev_validation_state,
            #[weak]
            changed_signal_handle,
            move |obj| {
                obj.remove_css_class("success");
                prev_validation_state.set(None);

                let port_number = {
                    let port_number = obj.text().as_str().parse::<u16>();
                    tracing::info!(?port_number, "Setting custom static port");

                    port_number.unwrap()
                };

                if port_scanner::local_port_available(port_number) {
                    imp.settings
                        .set_int("static-port-number", port_number.into())
                        .unwrap();

                    imp.preferences_dialog.close();

                    imp.obj().restart_rqs_service();
                }
                else if Some(port_number as u32) == imp.rqs.blocking_lock().as_ref().unwrap().port_number {
                    // Don't do anything if port is already set
                }
                else {
                    tracing::info!(port_number, "Port number isn't available");

                    // To prevent the apply button from showing after setting the text
                    obj.block_signal(&changed_signal_handle.borrow().as_ref().unwrap());
                    imp.static_port_entry.set_show_apply_button(false);
                    imp.static_port_entry
                        .set_text(&imp.settings.int("static-port-number").to_string());
                    imp.static_port_entry.set_show_apply_button(true);
                    obj.unblock_signal(&changed_signal_handle.borrow().as_ref().unwrap());

                    let info_dialog = adw::AlertDialog::builder()
                        .heading(&gettext("Invalid Port"))
                        .body(
                            &formatx!(
                                gettext(
                                    "The chosen static port \"{}\" is not available. Try a different port above 1024."
                                ),
                                port_number
                            )
                            .unwrap_or_default(),
                        )
                        .default_response("ok")
                        .build();
                    info_dialog.add_response("ok", &gettext("_Ok"));
                    info_dialog.set_response_appearance("ok", adw::ResponseAppearance::Suggested);
                    info_dialog.present(
                        imp.obj()
                            .root()
                            .and_downcast_ref::<PacketApplicationWindow>(),
                    );
                };
            }
        ));
        let _changed_signal_handle = imp.static_port_entry.connect_changed(clone!(
            #[strong]
            changed_signal_handle,
            #[strong]
            prev_validation_state,
            move |obj| {
                let parsed_port_number = obj.text().as_str().parse::<u16>();
                set_entry_validation_state(
                    &obj,
                    parsed_port_number.is_ok() && parsed_port_number.unwrap() > 1024,
                    &prev_validation_state,
                    changed_signal_handle.borrow().as_ref().unwrap(),
                );
            }
        ));
        *changed_signal_handle.as_ref().borrow_mut() = Some(_changed_signal_handle);

        // Check if we still have access to the set "Downloads Folder"
        {
            let download_folder = imp.settings.string("download-folder");
            let download_folder_exists = std::fs::exists(&download_folder).unwrap_or_default();

            if !download_folder_exists {
                let fallback = xdg_download_with_fallback();

                tracing::warn!(
                    ?download_folder,
                    ?fallback,
                    "Couldn't access Downloads folder. Resetting to fallback"
                );

                // Fallback for when user doesn't select a download folder when prompted
                imp.settings
                    .set_string("download-folder", fallback.to_str().unwrap())
                    .unwrap();

                imp.toast_overlay.add_toast(
                    adw::Toast::builder()
                        .title(&gettext("Can't access Downloads folder"))
                        .button_label(&gettext("Pick Folder"))
                        .action_name("win.pick-download-folder")
                        .build(),
                );
            }
        }

        imp.download_folder_row.set_subtitle(
            &strip_user_home_prefix(&imp.settings.string("download-folder")).to_string_lossy(),
        );
        imp.download_folder_pick_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                imp.obj().pick_download_folder();
            }
        ));
    }

    async fn portal_request_background(&self) -> Option<Background> {
        let imp = self.imp();

        let response = Background::request()
            .identifier(ashpd::WindowIdentifier::from_native(&self.native().unwrap()).await)
            .auto_start(self.imp().settings.boolean("auto-start"))
            .command(["packet", "--background"])
            .dbus_activatable(false)
            .reason(gettext("Packet wants to run in the background").as_str())
            .send()
            .await
            .and_then(|it| it.response());

        match response {
            Ok(response) => {
                self.imp().is_background_allowed.replace(true);

                Some(response)
            }
            Err(err) => {
                tracing::warn!("Background request denied: {:#}", err);

                imp.is_background_allowed.replace(false);

                with_signals_blocked(
                    &[
                        (
                            &imp.run_in_background_switch.get(),
                            imp.run_in_background_switch_handler_id.borrow().as_ref(),
                        ),
                        (
                            &imp.auto_start_switch.get(),
                            imp.auto_start_switch_handler_id.borrow().as_ref(),
                        ),
                    ],
                    || {
                        // Reset preferences to false in case request fails
                        _ = imp.settings.set_boolean("auto-start", false);
                        _ = imp.settings.set_boolean("run-in-background", false);
                    },
                );

                None
            }
        }
    }

    fn request_background_at_start(&self) {
        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                let is_run_in_background = this.imp().settings.boolean("run-in-background");
                if !is_run_in_background {
                    return;
                }
                if let Some(response) = this.portal_request_background().await {
                    tracing::debug!(?response, "Background request successful");

                    if !response.auto_start() {
                        if let Some(app) =
                            this.application().and_downcast_ref::<PacketApplication>()
                        {
                            app.imp().start_in_background.replace(false);
                        }
                    }
                } else {
                    this.add_toast(&gettext("Packet cannot run in the background"));
                }
            }
        ));
    }

    fn pick_download_folder(&self) {
        let imp = self.imp();

        glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                if let Ok(file) = gtk::FileDialog::new()
                    .select_folder_future(
                        imp.obj()
                            .root()
                            .and_downcast_ref::<PacketApplicationWindow>(),
                    )
                    .await
                {
                    // TODO: Maybe format the display path in the preferences?
                    // `Sandbox: Music` or `Music` instead of `/run/user/1000/_/Music` (for mounted paths)
                    // This would require storing the display string in gschema however
                    //
                    // Check whether it's a sandbox path or not by matching the path
                    // against the xattr host path, if it doesn't match, it's sandbox
                    //
                    // Flatpak metadata is available from `/.flatpak-info`, which contains info
                    // about host filesystem paths being available to the app, and much more.

                    // Path provided is host path if the app has been granted host access to it via
                    // --filesystem. Otherwise, it's a mounted path.
                    //
                    // Now, there's an issue with the vscode-flatpak extension where while running
                    // the app through it, the path given by FileChooser is always a mounted path.
                    // Leaving this note here so as to not base our logic on this wrong behaviour.
                    let folder_path = file.path().unwrap();

                    let display_path = strip_user_home_prefix(&folder_path);

                    tracing::debug!(
                        ?folder_path,
                        ?display_path,
                        "Selected custom downloads folder"
                    );

                    imp.download_folder_row
                        .set_subtitle(&display_path.to_string_lossy());

                    imp.settings
                        .set_string("download-folder", folder_path.to_str().unwrap())
                        .unwrap();
                    imp.rqs
                        .lock()
                        .await
                        .as_mut()
                        .unwrap()
                        .set_download_path(Some(folder_path));
                };
            }
        ));
    }

    #[cfg(target_os = "linux")]
    /// There's `tray-icon` for cross-platform systray support but on linux it still relies on gtk3 which doesn't
    /// work with gtk4 environment.
    ///
    /// https://github.com/tauri-apps/tray-icon/pull/201
    fn setup_tray_icon(&self) {
        let imp = self.imp();

        imp.tray_icon_group.set_visible(true);

        let is_enable_tray_icon = imp.settings.boolean("enable-tray-icon");
        tracing::debug!(?is_enable_tray_icon);
        if is_enable_tray_icon {
            self.enable_tray_icon();
        }
    }

    #[cfg(target_os = "linux")]
    async fn disable_tray_icon(&self) {
        let imp = self.imp();

        if let Some(ref mut handle) = *imp.tray_icon_handle.borrow_mut() {
            tracing::debug!("Disabling tray icon");
            handle.shutdown().await;
        }
        imp.tray_icon_handle.take();
    }

    #[cfg(target_os = "linux")]
    fn enable_tray_icon(&self) -> glib::JoinHandle<()> {
        use crate::tray;
        use ksni::*;

        let imp = self.imp();

        tracing::debug!("Enabling tray icon");
        let (tx, mut rx) = tokio::sync::mpsc::channel::<tray::TrayMessage>(1);
        let handle = glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                let tray = crate::tray::Tray { tx: tx };
                let handle = if ashpd::is_sandboxed().await {
                    tray.spawn_without_dbus_name().await
                } else {
                    tray.spawn().await
                }
                .inspect_err(
                    |err| tracing::warn!(%err, "Failed to setup KStatusNotifierItem tray icon"),
                )
                .ok();
                *imp.tray_icon_handle.borrow_mut() = handle;
            }
        ));

        glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                while let Some(msg) = rx.recv().await {
                    match msg {
                        tray::TrayMessage::OpenWindow => {
                            imp.obj().present();
                        }
                        tray::TrayMessage::Quit => {
                            imp.should_quit.replace(true);
                            // FIXME: If preference window is opened, that window gets closed instead of
                            // PacketApplicationWindow for some reason
                            imp.obj().close();
                        }
                    }
                }
            }
        ));

        handle
    }

    fn setup_ui(&self) {
        self.setup_bottom_bar();

        self.setup_status_pages();
        self.setup_main_page();
        self.setup_manage_files_page();
        self.setup_recipient_page();
        self.setup_share_text_page();
    }

    fn present_plugin_success_dialog(&self) {
        let dialog = adw::AlertDialog::builder()
            .heading(&gettext("Plugin Installed"))
            .default_response("done")
            .build();

        dialog.add_response("done", &gettext("Done"));
        dialog.set_response_appearance("done", adw::ResponseAppearance::Suggested);

        let info_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .spacing(8)
            .build();
        dialog.set_extra_child(Some(&info_box));

        let pkg_info_label = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .label(
                &formatx!(
                    gettext(
                        "The plugin was installed successfully, but requires the \
                        following packages to function: {}"
                    ),
                    "<tt>nautilus-python, python-dbus</tt>",
                )
                .unwrap_or_default(),
            )
            .build();
        info_box.append(&pkg_info_label);

        let pkg_info_link_label = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .label(
                &formatx!(
                    gettext(
                        "Package names may vary by distribution, visit <a {}>this \
                        link</a> for details."
                    ),
                    // Keeping it out of the msgid so that translators are less
                    // likely to mess something in here
                    "href=\"https://github.com/nozwock/packet?tab=readme-ov-file#plugin-requirements\""
                )
                .unwrap_or_default(),
            )
            .build();
        info_box.append(&pkg_info_link_label);

        let restart_info_label = gtk::Label::builder()
            .wrap(true)
            .label(&gettext(
                "Once that's done, restart Nautilus (e.g., by logging out and back in) to load the plugin.",
            ))
            .build();
        info_box.append(&restart_info_label);

        dialog.present(self.root().as_ref());
    }

    fn present_plugin_error_dialog(&self, extensions_display_dir: &str) {
        let dialog = adw::AlertDialog::builder()
            .heading(&gettext("Installation Failed"))
            .default_response("close")
            .build();

        dialog.add_response("close", &gettext("Close"));
        dialog.set_response_appearance("close", adw::ResponseAppearance::Suggested);

        let info_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .halign(gtk::Align::Center)
            .spacing(8)
            .build();
        dialog.set_extra_child(Some(&info_box));

        let info_label = gtk::Label::builder()
            .use_markup(true)
            .wrap(true)
            .label(&gettext(
                "Plugin installation failed. Make sure the following directory \
                exists and is accessible by Packet:",
            ))
            .build();
        info_box.append(&info_label);

        let extensions_dir_label = gtk::Label::builder()
            .selectable(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .label(extensions_display_dir)
            .css_classes(["command_snippet"])
            .build();
        info_box.append(&extensions_dir_label);

        dialog.present(self.root().as_ref());
    }

    pub async fn spawn_send_files_receiver(&self, rx: async_channel::Receiver<Vec<String>>) {
        let imp = self.imp();

        while let Ok(files) = rx.recv().await {
            // Bring the app window to focus
            self.present();

            let success = self.handle_added_files_to_send(
                &imp.manage_files_model,
                files
                    .into_iter()
                    .map(|it| gio::File::for_path(it))
                    .collect::<Vec<_>>(),
            );
            if success {
                self.present_recipients_dialog();
            } else {
                self.close_recipients_dialog();
            }
        }
    }

    fn present_recipients_dialog(&self) {
        let imp = self.imp();

        if imp.is_recipients_dialog_opened.get() {
            return;
        }

        // Clear previous recipients
        imp.send_transfers_id_cache.blocking_lock().clear();
        imp.recipient_model.remove_all();

        imp.obj().start_mdns_discovery(None);

        imp.select_recipients_dialog.present(self.root().as_ref());
        imp.is_recipients_dialog_opened.set(true);
    }

    fn close_recipients_dialog(&self) {
        let imp = self.imp();

        if !imp.is_recipients_dialog_opened.get() {
            return;
        }

        // is_recipients_dialog_opened is set in `closed` signal handler
        imp.select_recipients_dialog.close();
    }

    fn setup_status_pages(&self) {
        let imp = self.imp();

        let clipboard = self.clipboard();
        imp.rqs_error_copy_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |button| {
                button.set_sensitive(false);
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    #[weak]
                    button,
                    #[weak]
                    clipboard,
                    async move {
                        let logs = tokio_runtime()
                            .spawn_blocking(move || -> anyhow::Result<_> {
                                Ok(fs_err::read_to_string(packet_log_path())?)
                            })
                            .await
                            .map_err(|err| anyhow::anyhow!(err))
                            .and_then(|it| it)
                            .map_err(|err| err.context(gettext("Failed to retrieve the logs")))
                            .inspect_err(|err| tracing::warn!("{err:#}"));

                        match logs {
                            Ok(logs) => {
                                clipboard.set_text(&logs);
                                imp.toast_overlay.add_toast(adw::Toast::new(&gettext(
                                    "Copied log to clipboard",
                                )));
                            }
                            Err(err) => {
                                imp.toast_overlay
                                    .add_toast(adw::Toast::new(&err.to_string()));
                            }
                        };

                        button.set_sensitive(true);
                    }
                ));
            }
        ));
        imp.rqs_error_retry_button.connect_clicked(clone!(
            #[weak(rename_to = this)]
            self,
            move |_| {
                this.restart_rqs_service();
            }
        ));
    }

    fn setup_main_page(&self) {
        let imp = self.imp();

        imp.main_add_files_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                imp.manage_files_model.remove_all();
                imp.obj().add_files_via_dialog();
            }
        ));

        let files_drop_target = gtk::DropTarget::builder()
            .name("add-files-drop-target")
            .actions(gdk::DragAction::COPY)
            .formats(&gdk::ContentFormats::for_type(gdk::FileList::static_type()))
            .build();
        imp.main_nav_content
            .get()
            .add_controller(files_drop_target.clone());
        files_drop_target.connect_drop(clone!(
            #[weak]
            imp,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                imp.manage_files_model.remove_all();
                if let Ok(file_list) = value.get::<gdk::FileList>() {
                    imp.obj()
                        .handle_added_files_to_send(&imp.manage_files_model, file_list.files());
                }

                false
            }
        ));
    }

    fn setup_manage_files_page(&self) {
        let imp = self.imp();

        imp.manage_files_add_files_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                imp.obj().add_files_via_dialog();
            }
        ));
        imp.manage_files_send_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                imp.obj().present_recipients_dialog();
            }
        ));

        let manage_files_add_drop_target = gtk::DropTarget::builder()
            .name("manage-files-add-drop-target")
            .actions(gdk::DragAction::COPY)
            .formats(&gdk::ContentFormats::for_type(gdk::FileList::static_type()))
            .build();
        imp.manage_files_nav_content
            .get()
            .add_controller(manage_files_add_drop_target.clone());
        manage_files_add_drop_target.connect_drop(clone!(
            #[weak]
            imp,
            #[upgrade_or]
            false,
            move |_, value, _, _| {
                if let Ok(file_list) = value.get::<gdk::FileList>() {
                    imp.obj()
                        .handle_added_files_to_send(&imp.manage_files_model, file_list.files());
                }

                false
            }
        ));

        // TODO: Improve keyboard accessibility, make elements that can't be
        // activated non-focusable

        imp.manage_files_listbox.bind_model(
            Some(&imp.manage_files_model),
            clone!(
                #[weak]
                imp,
                #[upgrade_or]
                adw::Bin::new().into(),
                move |model| {
                    let model_item = model.downcast_ref::<gio::File>().unwrap();
                    let widget =
                        widgets::create_file_card(&imp.obj(), &imp.manage_files_model, model_item);

                    // TODO: Should focusable be false too since it adds unnecessary steps in keyboard navigation?
                    let row = gtk::ListBoxRow::new();
                    row.set_activatable(false);
                    row.set_child(Some(&widget));

                    row.into()
                }
            ),
        );

        imp.select_recipients_dialog.connect_closed(clone!(
            #[weak]
            imp,
            move |_| {
                imp.is_recipients_dialog_opened.set(false);
                imp.obj().stop_mdns_discovery();
                // Clear pending text when dialog closes
                *imp.pending_text.borrow_mut() = None;
            }
        ));
    }

    fn setup_recipient_page(&self) {
        let imp = self.imp();

        imp.recipient_listbox.bind_model(
            Some(&imp.recipient_model),
            clone!(
                #[weak]
                imp,
                #[upgrade_or]
                adw::Bin::new().into(),
                move |obj| {
                    let model_item = obj.downcast_ref::<SendRequestState>().unwrap();
                    widgets::create_recipient_card(
                        &imp.obj(),
                        &imp.recipient_model,
                        model_item,
                        Some(()),
                    )
                    .into()
                }
            ),
        );
        imp.recipient_listbox.connect_row_activated(clone!(
            #[weak]
            imp,
            move |obj, row| {
                widgets::handle_recipient_card_clicked(&imp.obj(), &obj, &row);
            }
        ));
        imp.recipient_model.connect_items_changed(clone!(
            #[weak]
            imp,
            move |model, _, _, _| {
                if model.n_items() == 0 {
                    imp.loading_recipients_box.set_visible(true);
                    imp.recipients_help_button.set_visible(true);
                    imp.recipient_listbox.set_visible(false);
                } else {
                    imp.loading_recipients_box.set_visible(false);
                    imp.recipients_help_button.set_visible(false);
                    imp.recipient_listbox.set_visible(true);
                }
            }
        ));

        imp.recipients_help_button
            .action_set_enabled("menu.popup", false);
        imp.recipients_help_button
            .action_set_enabled("clipboard.copy", false);
        imp.recipients_help_button.connect_activate_link(clone!(
            #[weak]
            imp,
            #[upgrade_or]
            true.into(),
            move |_| {
                imp.help_dialog.present(
                    imp.obj()
                        .root()
                        .and_downcast_ref::<PacketApplicationWindow>(),
                );

                true.into()
            }
        ));

        imp.select_recipient_refresh_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                tracing::info!("Refreshing recipients");

                {
                    let mut recipients_to_remove = imp
                        .recipient_model
                        .iter::<SendRequestState>()
                        .enumerate()
                        .filter_map(|(pos, it)| it.ok().and_then(|it| Some((pos, it))))
                        .filter(|(_, it)| match it.transfer_state() {
                            TransferState::Queued
                            | TransferState::RequestedForConsent
                            | TransferState::OngoingTransfer => false,
                            TransferState::AwaitingConsentOrIdle
                            | TransferState::Failed
                            | TransferState::Done => true,
                        })
                        .collect::<Vec<_>>();
                    recipients_to_remove.sort_by_key(|(pos, _)| *pos);

                    let mut items_removed = 0;
                    let mut guard = imp.send_transfers_id_cache.blocking_lock();
                    for (pos, obj) in recipients_to_remove {
                        let actual_pos = pos - items_removed;

                        imp.recipient_model.remove(actual_pos as u32);
                        let removed_model_item = guard.remove(&obj.endpoint_info().id);
                        items_removed += 1;

                        tracing::debug!(
                            endpoint_info = %obj.endpoint_info(),
                            last_state = ?(
                                obj.transfer_state(),
                                &obj.event()
                                    .as_ref()
                                    .and_then(|it| it.msg.as_client())
                                    .as_ref()
                                    .map(|msg| &msg.state),
                            ),
                            model_item_pos = actual_pos,
                            was_model_item_cached = removed_model_item.is_some(),
                            "Removed recipient card"
                        );
                    }
                }

                imp.obj().stop_mdns_discovery();
                imp.obj().start_mdns_discovery(None);
            }
        ));
    }

    fn setup_share_text_page(&self) {
        let imp = self.imp();

        // Maximum characters allowed for text sharing
        const MAX_TEXT_CHARS: i32 = 100_000;

        // Open share text dialog
        imp.main_share_text_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                imp.share_text_view.buffer().set_text("");
                imp.send_text_button.set_sensitive(false);
                imp.share_text_counter.set_label(&format!("0 / {}", MAX_TEXT_CHARS));
                imp.share_text_counter.remove_css_class("error");
                imp.share_text_dialog.present(Some(imp.obj().as_ref()));
            }
        ));

        // Paste from clipboard
        imp.paste_text_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                let clipboard = imp.obj().clipboard();
                glib::spawn_future_local(clone!(
                    #[weak]
                    imp,
                    async move {
                        if let Ok(text) = clipboard.read_text_future().await {
                            if let Some(text) = text {
                                imp.share_text_view.buffer().set_text(&text);
                            }
                        }
                    }
                ));
            }
        ));

        // Enable/disable send button based on text content and length
        imp.share_text_view.buffer().connect_changed(clone!(
            #[weak]
            imp,
            move |buffer| {
                let char_count = buffer.char_count();
                let is_valid = char_count > 0 && char_count <= MAX_TEXT_CHARS;

                imp.send_text_button.set_sensitive(is_valid);
                imp.share_text_counter.set_label(&format!("{} / {}", char_count, MAX_TEXT_CHARS));

                if char_count > MAX_TEXT_CHARS {
                    imp.share_text_counter.add_css_class("error");
                } else {
                    imp.share_text_counter.remove_css_class("error");
                }
            }
        ));

        // Send text
        imp.send_text_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                let buffer = imp.share_text_view.buffer();
                let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                let text_str = text.to_string();

                // Detect if it's a URL
                let text_type = if is_likely_url(&text_str) {
                    rqs_lib::TextPayloadType::Url
                } else {
                    rqs_lib::TextPayloadType::Text
                };

                // Store pending text
                *imp.pending_text.borrow_mut() = Some((text_str, text_type));

                imp.share_text_dialog.close();
                imp.obj().present_recipients_dialog();
            }
        ));
    }

    fn bottom_bar_status_indicator_ui_update(&self, is_visible: bool) {
        let imp = self.imp();

        let network_state = imp.network_state.get();
        let bluetooth_state = imp.bluetooth_state.get();

        if network_state && bluetooth_state {
            if is_visible {
                imp.bottom_bar_title.set_label(&gettext("Ready"));
                imp.bottom_bar_title.add_css_class("accent");
                imp.bottom_bar_image
                    .set_icon_name(Some("network-available-symbolic"));
                imp.bottom_bar_image.add_css_class("accent");
                imp.bottom_bar_caption.set_label(
                    &formatx!(
                        gettext("Visible as {:?}"),
                        imp.obj().get_device_name_state().as_str()
                    )
                    .unwrap_or_else(|_| "badly formatted locale string".into()),
                );
            } else {
                imp.bottom_bar_title.set_label(&gettext("Invisible"));
                imp.bottom_bar_title.remove_css_class("accent");
                imp.bottom_bar_image
                    .set_icon_name(Some("eye-not-looking-symbolic"));
                imp.bottom_bar_image.remove_css_class("accent");
                imp.bottom_bar_caption
                    .set_label(&gettext("No new devices can share with you"));
            };
        } else {
            imp.bottom_bar_image
                .set_icon_name(Some("horizontal-arrows-long-x-symbolic"));
            imp.bottom_bar_title.set_label(&gettext("Disconnected"));
            imp.bottom_bar_image.remove_css_class("accent");
            imp.bottom_bar_title.remove_css_class("accent");

            if !network_state && !bluetooth_state {
                imp.bottom_bar_caption
                    .set_label(&gettext("Connect to Wi-Fi and turn on Bluetooth"));
            } else if !network_state && bluetooth_state {
                imp.bottom_bar_caption
                    .set_label(&gettext("Connect to Wi-Fi"));
            } else if network_state && !bluetooth_state {
                imp.bottom_bar_caption
                    .set_label(&gettext("Turn on Bluetooth"));
            }
        }
    }

    fn setup_bottom_bar(&self) {
        let imp = self.imp();

        // Switch bottom bar layout b/w "Selected Files" page and other pages
        imp.main_nav_view.connect_visible_page_notify(clone!(
            #[weak]
            imp,
            move |obj| {
                if let Some(tag) = obj.visible_page_tag() {
                    match tag.as_str() {
                        "manage_files_nav_page" => {
                            imp.bottom_bar_status.set_halign(gtk::Align::Start);
                            imp.bottom_bar_status_top.set_halign(gtk::Align::Start);
                            imp.bottom_bar_caption.set_xalign(0.);
                            imp.bottom_bar_spacer.set_visible(true);
                            imp.manage_files_send_button.set_visible(true);
                        }
                        _ => {
                            imp.bottom_bar_status.set_halign(gtk::Align::Center);
                            imp.bottom_bar_status_top.set_halign(gtk::Align::Center);
                            imp.bottom_bar_caption.set_xalign(0.5);
                            imp.bottom_bar_spacer.set_visible(false);
                            imp.manage_files_send_button.set_visible(false);
                        }
                    }
                }
            }
        ));

        self.bottom_bar_status_indicator_ui_update(imp.device_visibility_switch.is_active());
        imp.device_visibility_switch.connect_active_notify(clone!(
            #[weak]
            imp,
            move |obj| {
                imp.obj()
                    .bottom_bar_status_indicator_ui_update(obj.is_active());

                let visibility = if obj.is_active() {
                    rqs_lib::Visibility::Visible
                } else {
                    rqs_lib::Visibility::Invisible
                };

                glib::spawn_future_local(async move {
                    imp.rqs
                        .lock()
                        .await
                        .as_mut()
                        .unwrap()
                        .change_visibility(visibility);
                });
            }
        ));
    }

    fn handle_added_files_to_send(&self, model: &gio::ListStore, files: Vec<gio::File>) -> bool {
        let imp = self.imp();

        tracing::debug!(selected_files = ?files.iter().map(|it| it.path()).collect::<Vec<_>>());

        let (files, is_already_in_model) = Self::filter_added_files(model, files);
        if is_already_in_model {
            return true;
        }

        // TODO: Maybe don't show this if the only filtered out files
        // are the 0 byte sized
        if files.len() == 0 {
            self.add_toast(&gettext("Couldn't open files"));

            false
        } else {
            let file_count = files.len() + model.n_items() as usize;
            imp.manage_files_header.set_title(
                &formatx!(
                    ngettext(
                        // Translators: An e.g. "4 Files"
                        "{} File",
                        "{} Files",
                        file_count as u32
                    ),
                    file_count
                )
                .unwrap_or_else(|_| "badly formatted locale string".into()),
            );

            for file in &files {
                model.append(file);
            }

            let Some(tag) = imp.main_nav_view.visible_page_tag() else {
                return false;
            };

            if &tag != "manage_files_nav_page" {
                imp.main_nav_view.push_by_tag("manage_files_nav_page");
            }

            true
        }
    }

    fn add_files_via_dialog(&self) {
        let imp = self.imp();
        gtk::FileDialog::new().open_multiple(
            imp.obj()
                .root()
                .and_downcast_ref::<adw::ApplicationWindow>(),
            None::<&gio::Cancellable>,
            clone!(
                #[weak]
                imp,
                move |files| {
                    if let Ok(files) = files {
                        let mut files_vec = Vec::with_capacity(files.n_items() as usize);
                        for i in 0..files.n_items() {
                            let file = files.item(i).unwrap().downcast::<gio::File>().unwrap();
                            files_vec.push(file);
                        }

                        imp.obj()
                            .handle_added_files_to_send(&imp.manage_files_model, files_vec);
                    };
                }
            ),
        );
    }

    fn filter_added_files(model: &gio::ListStore, files: Vec<gio::File>) -> (Vec<gio::File>, bool) {
        let files_len = files.len();

        let mut already_included_count = 0usize;
        let filtered_files = files
            .into_iter()
            .filter(|file| {
                file.query_file_type(
                    gio::FileQueryInfoFlags::NOFOLLOW_SYMLINKS,
                    gio::Cancellable::NONE,
                ) == gio::FileType::Regular
            })
            .filter(|it| {
                // Don't send 0 byte files
                // Because the rqs_lib expect files

                let file_size = it
                    .query_info(
                        FILE_ATTRIBUTE_STANDARD_SIZE,
                        gio::FileQueryInfoFlags::NONE,
                        gio::Cancellable::NONE,
                    )
                    .map(|it| it.size())
                    .unwrap_or_default();

                file_size != 0
            })
            .filter(|file| {
                for existing_file in model.iter::<gio::File>().filter_map(|it| it.ok()) {
                    if existing_file.parse_name() == file.parse_name() {
                        already_included_count += 1;
                        return false;
                    }
                }

                true
            })
            .collect::<Vec<_>>();

        let is_already_in_model = already_included_count == files_len;
        (filtered_files, is_already_in_model)
    }

    fn start_mdns_discovery(&self, force: Option<bool>) {
        let imp = self.imp();

        if (force.is_some() && force.unwrap_or_default())
            || (force.is_none() && !imp.is_mdns_discovery_on.get())
        {
            tracing::info!(?force, "Starting mDNS discovery task");

            tokio_runtime().spawn(clone!(
                #[weak(rename_to = mdns_discovery_broadcast_tx)]
                imp.mdns_discovery_broadcast_tx,
                #[weak(rename_to = rqs)]
                imp.rqs,
                async move {
                    _ = rqs
                        .lock()
                        .await
                        .as_mut()
                        .unwrap()
                        .discovery(
                            mdns_discovery_broadcast_tx
                                .lock()
                                .await
                                .as_ref()
                                .unwrap()
                                .clone(),
                        )
                        .inspect_err(|err| {
                            tracing::error!(
                                err = format!("{err:#}"),
                                "Failed to start mDNS discovery task"
                            )
                        });
                }
            ));

            imp.is_mdns_discovery_on.replace(true);
        }
    }

    fn stop_mdns_discovery(&self) {
        let imp = self.imp();

        if imp.is_mdns_discovery_on.get() {
            tokio_runtime().spawn(clone!(
                #[weak(rename_to = rqs)]
                imp.rqs,
                async move {
                    rqs.lock().await.as_mut().unwrap().stop_discovery();
                }
            ));

            imp.is_mdns_discovery_on.replace(false);
        }
    }

    fn is_no_file_being_send(&self) -> bool {
        let imp = self.imp();

        for model_item in imp
            .recipient_model
            .iter::<SendRequestState>()
            .filter_map(|it| it.ok())
        {
            use rqs_lib::TransferState;
            match model_item
                .event()
                .unwrap()
                .msg
                .as_client_unchecked()
                .state
                .as_ref()
                .unwrap_or(&rqs_lib::TransferState::Initial)
            {
                TransferState::Initial
                | TransferState::Disconnected
                | TransferState::Rejected
                | TransferState::Cancelled
                | TransferState::Finished => {}
                _ => {
                    return false;
                }
            }
        }

        true
    }

    fn restart_rqs_service(&self) -> glib::JoinHandle<()> {
        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            async move {
                this.imp()
                    .root_stack
                    .set_visible_child_name("loading_service_page");
                _ = this.stop_rqs_service().await;
                _ = this.setup_rqs_service().await;
            }
        ))
    }

    fn stop_rqs_service(&self) -> tokio::task::JoinHandle<()> {
        let imp = self.imp();

        // Abort all looping tasks before closing
        tracing::info!(
            count = imp.looping_async_tasks.borrow().len(),
            "Cancelling looping tasks"
        );
        while let Some(join_handle) = imp.looping_async_tasks.borrow_mut().pop() {
            match join_handle {
                LoopingTaskHandle::Tokio(join_handle) => join_handle.abort(),
                LoopingTaskHandle::Glib(join_handle) => join_handle.abort(),
            }
        }

        let handle = tokio_runtime().spawn(clone!(
            #[weak(rename_to = rqs)]
            imp.rqs,
            async move {
                {
                    let mut rqs_guard = rqs.lock().await;
                    if let Some(rqs) = rqs_guard.as_mut() {
                        rqs.stop().await;
                        tracing::info!("Stopped RQS service");
                    }
                }
            }
        ));

        handle
    }

    fn setup_connection_monitors(&self) {
        let imp = self.imp();

        let (tx, mut network_rx) = watch::channel(false);
        // Set initial state
        _ = tx.send(imp.network_monitor.is_network_available());
        imp.network_monitor
            .connect_network_changed(move |monitor, _| {
                _ = tx.send(monitor.is_network_available());
            });

        glib::spawn_future_local(clone!(
            #[weak(rename_to = this)]
            self,
            #[weak(rename_to = dbus_system_conn)]
            imp.dbus_system_conn,
            async move {
                let conn = {
                    let conn = zbus::Connection::system().await;
                    *dbus_system_conn.borrow_mut() = conn.clone().ok();
                    conn.unwrap()
                };

                let bluetooth_initial_state = monitors::is_bluetooth_powered(&conn)
                    .await
                    .map_err(|err| {
                        anyhow!(err).context("Failed to get initial Bluetooth powered state")
                    })
                    .inspect_err(|err| {
                        tracing::warn!(fallback = false, "{err:#}",);
                    })
                    .unwrap_or_default();
                let (tx, mut bluetooth_rx) = watch::channel(bluetooth_initial_state);
                glib::spawn_future(async move {
                    if let Err(err) = monitors::spawn_bluetooth_power_monitor_task(conn, tx)
                        .await
                        .map_err(|err| anyhow!(err))
                    {
                        tracing::error!(
                            "{:#}",
                            err.context("Failed to spawn the Bluetooth powered state monitor task")
                        );
                    };
                });

                glib::spawn_future_local(clone!(
                    #[weak]
                    this,
                    async move {
                        enum ChangedState {
                            Network,
                            Bluetooth,
                        }

                        let imp = this.imp();

                        imp.bluetooth_state.set(bluetooth_initial_state);

                        #[allow(unused)]
                        let mut is_state_changed = None;

                        loop {
                            tokio::select! {
                                _ = network_rx.changed() => {

                                    let v = *network_rx.borrow();

                                    // Since we get spammed with network change events
                                    // even though the state hasn't changed from before
                                    //
                                    // This also helps keep the logs to a minimum
                                    is_state_changed = (imp.network_state.get() != v).then_some(ChangedState::Network);

                                    imp.network_state.set(v) ;
                                }
                                _ = bluetooth_rx.changed() => {
                                    is_state_changed = Some(ChangedState::Bluetooth);

                                    imp.bluetooth_state.set(*bluetooth_rx.borrow());
                                    tracing::info!(bluetooth_state = imp.bluetooth_state.get(), "Bluetooth powered state changed");
                                }
                            };

                            if is_state_changed.is_some() {
                                if let Some(ChangedState::Network) = is_state_changed {
                                    tracing::info!(
                                        network_state = imp.network_state.get(),
                                        "Network state changed"
                                    );
                                }

                                this.bottom_bar_status_indicator_ui_update(
                                    imp.device_visibility_switch.is_active(),
                                );
                            }
                        }
                    }
                ));
            }
        ));
    }

    fn setup_notification_actions_monitor(&self) {
        let imp = self.imp();

        glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                _ = async move || -> anyhow::Result<()> {
                    let proxy = NotificationProxy::new().await?;

                    let mut action_stream = proxy.receive_action_invoked().await?;
                    loop {
                        let action = action_stream.next().await.context("Stream exhausted")?;
                        tracing::info!(action_name = ?action.name(), id = action.id(), params = ?action.parameter(), "Notification action received");

                        if let Some(cached_transfer) = imp.receive_transfer_cache.lock().await.as_mut() {
                            match action.name() {
                                "consent-accept" => {
                                    // TODO: Maybe Enum should contain transfer id
                                    // since notifications can outlast the app, might as well
                                    // put some safe guards in place in case we fail to cleanup
                                    // some notification on app close.
                                    //
                                    // But, it doesn't seems like the action that doesn't start with `app.`
                                    // really do anything while the app is closed, so maybe not.
                                    cached_transfer.state.set_user_action(Some(UserAction::ConsentAccept));
                                },
                                "consent-decline" => {
                                    cached_transfer.state.set_user_action(Some(UserAction::ConsentDecline));
                                },
                                "transfer-cancel" => {
                                    cached_transfer.state.set_user_action(Some(UserAction::TransferCancel));
                                },
                                "open-folder" => {
                                    if let Some(param) = action.parameter().get(0).and_then(|it| {
                                        it.downcast_ref::<String>()
                                            .inspect_err(|err| tracing::warn!("{err:#}"))
                                            .ok()
                                    }) {
                                        gtk::FileLauncher::new(Some(&gio::File::for_path(param))).launch(
                                            Some(imp.obj().as_ref()),
                                            None::<&gio::Cancellable>,
                                            move |_| {},
                                        );
                                    }
                                },
                                "copy-text" => {
                                    if let Some(param) = action.parameter().get(0).and_then(|it| {
                                        it.downcast_ref::<String>()
                                            .inspect_err(|err| tracing::warn!("{err:#}"))
                                            .ok()
                                    }) {
                                        let clipboard = imp.obj().clipboard();
                                        clipboard.set_text(&param);
                                    }
                                },
                                // Default actions, etc
                                _ => {},
                            };
                        }
                    }
                }()
                .await
                .inspect_err(|err| tracing::error!("{err:#}"));
            }
        ));
    }

    fn setup_rqs_service(&self) -> glib::JoinHandle<()> {
        let imp = self.imp();

        let is_device_visible = imp.settings.boolean("device-visibility");
        let device_name = self.get_device_name_state();
        let download_path = imp
            .settings
            .string("download-folder")
            .parse::<PathBuf>()
            .unwrap();
        let static_port = imp
            .settings
            .boolean("enable-static-port")
            .then(|| imp.settings.int("static-port-number") as u32);
        let rqs_init_handle = glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                let _imp = imp.clone();
                if let Err(err) = async move || -> anyhow::Result<()> {
                    let (rqs, run_result) = tokio_runtime()
                        .spawn(async move {
                            tracing::info!(
                                ?device_name,
                                visibility = ?is_device_visible,
                                ?download_path,
                                ?static_port,
                                "Starting RQS service"
                            );

                            let mut rqs = rqs_lib::RQS::new(
                                if is_device_visible {
                                    rqs_lib::Visibility::Visible
                                } else {
                                    rqs_lib::Visibility::Invisible
                                },
                                static_port,
                                Some(download_path),
                                Some(device_name.to_string()),
                            );

                            let run_result = rqs.run().await;
                            (rqs, run_result)
                        })
                        .await?;

                    *imp.rqs.lock().await = Some(rqs);
                    let (mdns_discovery_broadcast_tx, _) =
                        tokio::sync::broadcast::channel::<rqs_lib::EndpointInfo>(10);
                    *imp.mdns_discovery_broadcast_tx.lock().await =
                        Some(mdns_discovery_broadcast_tx);

                    let (file_sender, ble_receiver) = run_result?;
                    *imp.file_sender.lock().await = Some(file_sender);
                    *imp.ble_receiver.lock().await = Some(ble_receiver);

                    imp.root_stack.get().set_visible_child_name("main_page");

                    spawn_rqs_receiver_tasks(&imp);

                    Ok(())
                }()
                .await
                {
                    let err = err.context("Failed to setup Packet");
                    tracing::error!("{err:#}");

                    _imp.root_stack
                        .get()
                        .set_visible_child_name("rqs_error_status_page");
                }
            }
        ));

        fn spawn_rqs_receiver_tasks(imp: &imp::PacketApplicationWindow) {
            let (tx, rx) = async_channel::bounded(1);
            let handle = tokio_runtime().spawn(clone!(
                #[weak(rename_to = rqs)]
                imp.rqs,
                async move {
                    let mut rx = rqs
                        .lock()
                        .await
                        .as_ref()
                        .expect("State must be set")
                        .message_sender
                        .subscribe();

                    loop {
                        match rx.recv().await {
                            Ok(channel_message) => {
                                tx.send(channel_message).await.unwrap();
                            }
                            Err(err) => {
                                tracing::error!("{err:#}")
                            }
                        };
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Tokio(handle));

            let handle = glib::spawn_future_local(clone!(
                #[weak]
                imp,
                async move {
                    loop {
                        let channel_message = rx.recv().await.unwrap();

                        if channel_message.msg.as_client().is_none() {
                            // Ignore library messages
                            continue;
                        }

                        tracing::debug!(event = ?channel_message, "Received event on UI thread");

                        let id = &channel_message.id;
                        let client_msg = channel_message.msg.as_client_unchecked();

                        use rqs_lib::TransferState;
                        match client_msg
                            .state
                            .clone()
                            .unwrap_or(rqs_lib::TransferState::Initial)
                        {
                            TransferState::Initial => {}
                            TransferState::ReceivedConnectionRequest => {}
                            TransferState::SentUkeyServerInit => {}
                            TransferState::SentPairedKeyEncryption => {}
                            TransferState::ReceivedUkeyClientFinish => {}
                            TransferState::SentConnectionResponse => {}
                            TransferState::SentPairedKeyResult => {}
                            TransferState::ReceivedPairedKeyResult => {}
                            TransferState::WaitingForUserConsent => {
                                // Receive data transfer requests
                                {
                                    let channel_message = objects::ChannelMessage(channel_message);

                                    let notification_id = glib::uuid_string_random().to_string();
                                    let state =
                                        objects::ReceiveTransferState::new(&channel_message);
                                    let ctk = CancellationToken::new();

                                    widgets::present_receive_transfer_ui(
                                        &imp.obj(),
                                        &state,
                                        notification_id.clone(),
                                        ctk.clone(),
                                    );
                                    *imp.receive_transfer_cache.lock().await =
                                        Some(ReceiveTransferCache {
                                            transfer_id: channel_message.id.to_string(),
                                            notification_id,
                                            state: state,
                                            auto_decline_ctk: ctk,
                                        });
                                }
                            }
                            TransferState::SentUkeyClientInit
                            | TransferState::SentUkeyClientFinish
                            | TransferState::SentIntroduction
                            | TransferState::Disconnected
                            | TransferState::Rejected
                            | TransferState::Cancelled
                            | TransferState::Finished
                            | TransferState::SendingFiles
                            | TransferState::ReceivingFiles => {
                                match client_msg.kind {
                                    rqs_lib::channel::TransferKind::Inbound => {
                                        // Receive
                                        if let Some(cached_transfer) =
                                            imp.receive_transfer_cache.lock().await.as_mut()
                                        {
                                            if !cached_transfer.auto_decline_ctk.is_cancelled() {
                                                // Cancel auto-decline
                                                cached_transfer.auto_decline_ctk.cancel();
                                            }

                                            cached_transfer.state.set_event(
                                                objects::ChannelMessage(channel_message),
                                            );
                                        }
                                    }
                                    rqs_lib::channel::TransferKind::Outbound => {
                                        // Send
                                        let send_transfers_id_cache =
                                            imp.send_transfers_id_cache.lock().await;

                                        if let Some(model_item) = send_transfers_id_cache.get(id) {
                                            model_item.set_event(Some(objects::ChannelMessage(
                                                channel_message,
                                            )));
                                        }
                                    }
                                };
                            }
                        };
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Glib(handle));

            // MDNS discovery receiver
            // Discover the devices to send file transfer requests to
            // The Sender used in RQS::discovery()
            let (tx, rx) = async_channel::bounded(1);
            let handle = tokio_runtime().spawn(clone!(
                #[weak(rename_to = mdns_discovery_broadcast_tx)]
                imp.mdns_discovery_broadcast_tx,
                async move {
                    let mdns_discovery_broadcast_tx = mdns_discovery_broadcast_tx
                        .lock()
                        .await
                        .as_ref()
                        .unwrap()
                        .clone();
                    let mut mdns_discovery_rx = mdns_discovery_broadcast_tx.subscribe();

                    loop {
                        match mdns_discovery_rx.recv().await {
                            Ok(endpoint_info) => {
                                tracing::trace!(?endpoint_info, "Processing endpoint");
                                tx.send(endpoint_info).await.unwrap();
                            }
                            Err(err) => {
                                tracing::error!(
                                    err = format!("{err:#}"),
                                    "mDNS discovery receiver"
                                );
                            }
                        }
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Tokio(handle));

            let handle = glib::spawn_future_local(clone!(
                #[weak]
                imp,
                async move {
                    loop {
                        {
                            let endpoint_info = rx.recv().await.unwrap();

                            let mut send_transfers_id_cache_guard =
                                imp.send_transfers_id_cache.lock().await;
                            if let Some(data_transfer) =
                                send_transfers_id_cache_guard.get(&endpoint_info.id)
                            {
                                // Update endpoint
                                let endpoint_info = objects::EndpointInfo(endpoint_info);
                                tracing::info!(%endpoint_info, "Updated endpoint");
                                data_transfer.set_endpoint_info(endpoint_info);
                            } else {
                                // Set new endpoint
                                let endpoint_info = objects::EndpointInfo(endpoint_info);
                                tracing::info!(%endpoint_info, "Discovered endpoint");
                                let obj = SendRequestState::new();
                                let id = endpoint_info.id.clone();
                                obj.set_endpoint_info(endpoint_info);
                                imp.recipient_model.insert(0, &obj);
                                send_transfers_id_cache_guard.insert(id, obj);
                            }
                        }
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Glib(handle));

            let handle = tokio_runtime().spawn(clone!(
                #[weak(rename_to = rqs)]
                imp.rqs,
                async move {
                    let mut visibility_receiver = rqs
                        .lock()
                        .await
                        .as_ref()
                        .expect("State must be set")
                        .visibility_sender
                        .lock()
                        .unwrap()
                        .subscribe();

                    loop {
                        match visibility_receiver.changed().await {
                            Ok(_) => {
                                // FIXME: Update visibility in UI, not used for now
                                // since visibility is not being set from outside
                                let visibility = visibility_receiver.borrow_and_update();
                                tracing::debug!(?visibility, "Visibility change");
                            }
                            Err(err) => {
                                tracing::error!(
                                    err = format!("{err:#}"),
                                    "Visibility watcher receiver"
                                );
                            }
                        }
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Tokio(handle));

            // A task that handles BLE advertisements from other nearby devices
            //
            // Close previous tasks and restart service whenever running RQS::run,
            // since that resets the ble receiver and other stuff, and here the
            // ble receiver is set to whichever one is in the Window state at the
            // time of setting up the task.
            let handle = tokio_runtime().spawn(clone!(
                #[weak(rename_to = ble_receiver)]
                imp.ble_receiver,
                async move {
                    let mut ble_receiver =
                        ble_receiver.lock().await.as_ref().unwrap().resubscribe();

                    // let mut last_sent = std::time::Instant::now() - std::time::Duration::from_secs(120);
                    loop {
                        match ble_receiver.recv().await {
                            Ok(_) => {
                                // let is_visible = device_visibility_switch.is_active();

                                // FIXME: The task is for the "A nearby device is sharing" feature
                                // where you're given an option to make yourself temporarily visible

                                // tracing::debug!("Received BLE event, show a \"A nearby device is sharing\" notification here")
                            }
                            Err(err) => {
                                tracing::error!(
                                    err = format!("{err:#}"),
                                    "Couldn't receive BLE event"
                                );
                            }
                        }
                    }
                }
            ));
            imp.looping_async_tasks
                .borrow_mut()
                .push(LoopingTaskHandle::Tokio(handle));
        }

        rqs_init_handle
    }
}
