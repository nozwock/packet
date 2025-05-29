use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;

use adw::prelude::*;
use adw::subclass::prelude::*;
use anyhow::{anyhow, Context};
use formatx::formatx;
use gettextrs::{gettext, ngettext};
use gtk::gio::FILE_ATTRIBUTE_STANDARD_SIZE;
use gtk::glib::clone;
use gtk::{gdk, gio, glib};
use tokio::sync::watch;

use crate::application::PacketApplication;
use crate::config::{APP_ID, PROFILE};
use crate::objects::TransferState;
use crate::objects::{self, SendRequestState};
use crate::utils::{strip_user_home_prefix, xdg_download_with_fallback};
use crate::{monitors, tokio_runtime, widgets};

#[derive(Debug)]
pub enum LoopingTaskHandle {
    Tokio(tokio::task::JoinHandle<()>),
    Glib(glib::JoinHandle<()>),
}

mod imp {
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        rc::Rc,
        sync::Arc,
    };

    use tokio::sync::Mutex;

    use super::*;

    #[derive(Debug, gtk::CompositeTemplate, better_default::Default)]
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
        pub rqs_error: Rc<RefCell<Option<anyhow::Error>>>,

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
        pub main_box: TemplateChild<gtk::Box>,
        #[template_child]
        pub main_nav_content: TemplateChild<adw::StatusPage>,
        #[template_child]
        pub main_add_files_button: TemplateChild<gtk::Button>,

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

        pub send_transfers_id_cache: Arc<Mutex<HashMap<String, SendRequestState>>>,

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
            obj.setup_ui();
            obj.setup_connection_monitors();
            obj.setup_rqs_service();
        }
    }

    impl WidgetImpl for PacketApplicationWindow {}
    impl WindowImpl for PacketApplicationWindow {
        // Save window state on delete event
        fn close_request(&self) -> glib::Propagation {
            if let Err(err) = self.obj().save_window_size() {
                tracing::warn!("Failed to save window state, {}", &err);
            }
            if let Err(err) = self.obj().save_app_state() {
                tracing::warn!("Failed to save app state, {}", &err);
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
        @implements gio::ActionMap, gio::ActionGroup, gtk::Root;
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

    fn setup_ui(&self) {
        self.setup_bottom_bar();

        self.setup_status_pages();
        self.setup_main_page();
        self.setup_manage_files_page();
        self.setup_recipient_page();
    }

    fn setup_status_pages(&self) {
        let imp = self.imp();

        let clipboard = self.clipboard();
        imp.rqs_error_copy_button.connect_clicked(clone!(
            #[weak]
            imp,
            move |_| {
                // TODO: Replace the copy button with an info button that
                // opens up a dialog with the option to copy or save the
                // app log, and a link to the issues page.
                clipboard.set_text(
                    &imp.rqs_error
                        .borrow()
                        .as_ref()
                        .map(|e| format!("{e:#}"))
                        .unwrap_or_default(),
                );
                imp.toast_overlay.add_toast(adw::Toast::new(&gettext(
                    "Copied error report to clipboard",
                )));
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
                    Self::handle_added_files_to_send(
                        &imp,
                        Self::filter_added_files(&imp.manage_files_model, file_list.files()),
                    );
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
                // Clear previous recipients
                imp.send_transfers_id_cache.blocking_lock().clear();
                imp.recipient_model.remove_all();

                imp.obj().start_mdns_discovery(None);

                imp.select_recipients_dialog
                    .present(imp.obj().root().as_ref());
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
                    Self::handle_added_files_to_send(
                        &imp,
                        Self::filter_added_files(&imp.manage_files_model, file_list.files()),
                    );
                }

                false
            }
        ));

        imp.manage_files_listbox.bind_model(
            Some(&imp.manage_files_model),
            clone!(
                #[weak]
                imp,
                #[upgrade_or]
                adw::Bin::new().into(),
                move |model| {
                    let model_item = model.downcast_ref::<gio::File>().unwrap();
                    widgets::create_file_card(&imp.obj(), &imp.manage_files_model, model_item)
                        .into()
                }
            ),
        );

        imp.select_recipients_dialog.connect_closed(clone!(
            #[weak]
            imp,
            move |_| {
                imp.obj().stop_mdns_discovery();
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
                            last_state = ?(obj.transfer_state(), &obj.event().state),
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

    fn bottom_bar_status_indicator_ui_update(&self, is_visible: bool) {
        let imp = self.imp();

        let network_state = imp.network_state.get();
        let bluetooth_state = imp.bluetooth_state.get();

        if network_state && bluetooth_state {
            if is_visible {
                imp.bottom_bar_title.set_label(&gettext("Ready"));
                imp.bottom_bar_title.add_css_class("accent");
                imp.bottom_bar_image.set_icon_name(Some("visible-symbolic"));
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
                .set_icon_name(Some("cross-small-circle-outline-symbolic"));
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

    fn handle_added_files_to_send(imp: &imp::PacketApplicationWindow, files: Vec<gio::File>) {
        if files.len() == 0 {
            imp.toast_overlay.add_toast(
                adw::Toast::builder()
                    .title(&gettext("Couldn't open files"))
                    .build(),
            );
        } else {
            tracing::debug!(files_added = ?files.iter().map(|it| it.path()).collect::<Vec<_>>());

            let file_count = files.len() + imp.manage_files_model.n_items() as usize;
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

            if let Some(tag) = imp.main_nav_view.visible_page_tag() {
                if &tag != "manage_files_nav_page" {
                    imp.main_nav_view.push_by_tag("manage_files_nav_page");
                }
            }

            for file in &files {
                imp.manage_files_model.append(file);
            }
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

                        Self::handle_added_files_to_send(
                            &imp,
                            Self::filter_added_files(&imp.manage_files_model, files_vec),
                        );
                    };
                }
            ),
        );
    }

    fn filter_added_files(model: &gio::ListStore, files: Vec<gio::File>) -> Vec<gio::File> {
        files
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
                        return false;
                    }
                }

                true
            })
            .collect::<Vec<_>>()
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
            use rqs_lib::State;
            match model_item
                .event()
                .state
                .as_ref()
                .unwrap_or(&rqs_lib::State::Initial)
            {
                State::Initial
                | State::Disconnected
                | State::Rejected
                | State::Cancelled
                | State::Finished => {}
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

    fn setup_rqs_service(&self) -> glib::JoinHandle<()> {
        let imp = self.imp();

        let (tx, rx) = async_channel::bounded(1);

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
        tokio_runtime().spawn(async move {
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

            let rqs_run_result = rqs.run().await;
            tx.send((rqs, rqs_run_result)).await.unwrap();
        });
        let rqs_init_handle = glib::spawn_future_local(clone!(
            #[weak]
            imp,
            async move {
                let (rqs, rqs_run_result) = rx.recv().await.unwrap();

                tracing::debug!("Fetched RQS instance after run()");
                *imp.rqs.lock().await = Some(rqs);
                let (mdns_discovery_broadcast_tx, _) =
                    tokio::sync::broadcast::channel::<rqs_lib::EndpointInfo>(10);
                *imp.mdns_discovery_broadcast_tx.lock().await = Some(mdns_discovery_broadcast_tx);

                match rqs_run_result {
                    Ok((file_sender, ble_receiver)) => {
                        *imp.file_sender.lock().await = Some(file_sender);
                        *imp.ble_receiver.lock().await = Some(ble_receiver);

                        imp.root_stack.get().set_visible_child_name("main_page");

                        spawn_rqs_receiver_tasks(&imp);
                    }
                    Err(err) => {
                        let err = err.context("Failed to setup Packet");
                        tracing::error!("{err:#}");
                        imp.rqs_error.borrow_mut().replace(err);

                        imp.root_stack
                            .get()
                            .set_visible_child_name("rqs_error_status_page");
                    }
                };
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

                                // FIXME: Send desktop notification aswell
                                // send_request_notification(name, channel_msg.id.clone());
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
                    let mut share_request_state: Option<objects::ReceiveTransferState> = None;
                    loop {
                        let channel_message = rx.recv().await.unwrap();

                        tracing::debug!(event = ?channel_message, "Received event on UI thread");

                        let id = &channel_message.id;

                        use rqs_lib::State;
                        match channel_message
                            .state
                            .clone()
                            .unwrap_or(rqs_lib::State::Initial)
                        {
                            State::Initial => {}
                            State::ReceivedConnectionRequest => {}
                            State::SentUkeyServerInit => {}
                            State::SentPairedKeyEncryption => {}
                            State::ReceivedUkeyClientFinish => {}
                            State::SentConnectionResponse => {}
                            State::SentPairedKeyResult => {}
                            State::ReceivedPairedKeyResult => {}
                            State::WaitingForUserConsent => {
                                // Receive data transfer requests
                                {
                                    let state = objects::ReceiveTransferState::new(
                                        &objects::ChannelMessage(channel_message),
                                    );
                                    widgets::present_receive_transfer_ui(&imp.obj(), &state);
                                    share_request_state = Some(state);
                                }
                            }
                            State::SentUkeyClientInit
                            | State::SentUkeyClientFinish
                            | State::SentIntroduction
                            | State::Disconnected
                            | State::Rejected
                            | State::Cancelled
                            | State::Finished
                            | State::SendingFiles
                            | State::ReceivingFiles => {
                                match channel_message.rtype {
                                    Some(rqs_lib::channel::TransferType::Inbound) => {
                                        // Receive
                                        if let Some(state) = share_request_state.as_mut() {
                                            state.set_event(objects::ChannelMessage(
                                                channel_message,
                                            ));
                                        }
                                    }
                                    Some(rqs_lib::channel::TransferType::Outbound) => {
                                        // Send
                                        let send_transfers_id_cache =
                                            imp.send_transfers_id_cache.lock().await;

                                        if let Some(model_item) = send_transfers_id_cache.get(id) {
                                            model_item.set_event(objects::ChannelMessage(
                                                channel_message,
                                            ));
                                        }
                                    }
                                    _ => {
                                        // FIXME: the Disconnect message you'll get can have no rtype
                                        // and so it's not received in the widget
                                        // leaving the card in Sending Files state
                                        // Take a look at what the hell is happening with rqs_lib
                                        // rqs_lib::manager: TcpServer: error while handling client:
                                        // packet::window: Received on UI thread, Disconnected message
                                        // with None rtype (to differentiate Outbound/Inbound)

                                        // As a bandit fix, assume this message is both
                                        // The issue occurs for both inbound/outbound.
                                        if channel_message.state == Some(State::Disconnected) {
                                            {
                                                let send_transfers_id_cache =
                                                    imp.send_transfers_id_cache.lock().await;

                                                if let Some(model_item) =
                                                    send_transfers_id_cache.get(id)
                                                {
                                                    model_item.set_event(objects::ChannelMessage(
                                                        channel_message.clone(),
                                                    ));
                                                }
                                            }

                                            // Received Disconnected for incoming transfer
                                            if let Some(state) = share_request_state.as_mut() {
                                                if channel_message.id == state.event().id {
                                                    state.set_event(objects::ChannelMessage(
                                                        channel_message,
                                                    ));
                                                }
                                            }
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
                                // FIXME: Update visibility in UI?
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

            // FIXME: Since renaming device name will restart the service,
            // we need to reset the ble_receiver here in the loop as well.
            // Ideal solution seem to be to keep a handle on this async task
            // and close it when we set device name and respawn it.
            // tokio_runtime().spawn(clone!(
            //     #[weak(rename_to = ble_receiver)]
            //     imp.ble_receiver,
            //     async move {
            //         let mut ble_receiver =
            //             ble_receiver.lock().await.as_ref().unwrap().resubscribe();
            //         // let mut last_sent = std::time::Instant::now() - std::time::Duration::from_secs(120);
            //         loop {
            //             match ble_receiver.recv().await {
            //                 Ok(_) => {
            //                     // let is_visible = device_visibility_switch.is_active();
            //                     // FIXME: Get visibility via a channel
            //                     // and temporarily make device visible?

            //                     tracing::debug!("Received BLE")
            //                 }
            //                 Err(err) => {
            //                     tracing::error!(%err,"Error receiving BLE");
            //                 }
            //             }
            //         }
            //     }
            // ));
        }

        rqs_init_handle
    }
}
