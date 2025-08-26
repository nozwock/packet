use gettextrs::gettext;
use gtk::glib::clone;
use tracing::{debug, info};

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, gio, glib};

use crate::config::{APP_ID, PKGDATADIR, PROFILE, VERSION};
use crate::constants::packet_log_path;
use crate::tokio_runtime;
use crate::window::PacketApplicationWindow;

type AsyncChannel<T> = (async_channel::Sender<T>, async_channel::Receiver<T>);

mod imp {

    use super::*;
    use glib::WeakRef;
    use std::cell::{Cell, OnceCell};

    #[derive(Debug, better_default::Default)]
    pub struct PacketApplication {
        pub window: OnceCell<WeakRef<PacketApplicationWindow>>,

        pub start_in_background: Cell<bool>,

        #[default(async_channel::bounded(1))]
        pub send_files_channel: AsyncChannel<Vec<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PacketApplication {
        const NAME: &'static str = "PacketApplication";
        type Type = super::PacketApplication;
        type ParentType = adw::Application;
    }

    impl ObjectImpl for PacketApplication {}

    impl ApplicationImpl for PacketApplication {
        fn activate(&self) {
            debug!("GtkApplication<PacketApplication>::activate");
            self.parent_activate();
            let app = self.obj();

            if let Some(window) = self.window.get() {
                let window = window.upgrade().unwrap();
                window.present();
                return;
            }

            let window = PacketApplicationWindow::new(&app);
            self.window
                .set(window.downgrade())
                .expect("Window already set.");

            // Setup receiver
            let rx = self.send_files_channel.1.clone();
            glib::spawn_future_local(glib::clone!(
                #[weak]
                app,
                async move {
                    app.main_window().spawn_send_files_receiver(rx).await;
                }
            ));

            if !self.start_in_background.get() {
                app.main_window().present();
            }
        }

        fn startup(&self) {
            debug!("GtkApplication<PacketApplication>::startup");
            
            // Restrict GDK to only use the macOS backend to avoid X11/Wayland dependencies
            #[cfg(target_os = "macos")]
            gdk::set_allowed_backends("macos");
            
            self.parent_startup();
            let app = self.obj();

            // Set icons for shell
            gtk::Window::set_default_icon_name(APP_ID);

            app.setup_css();
            app.setup_gactions();
            app.setup_accels();
        }

        fn dbus_register(
            &self,
            connection: &gio::DBusConnection,
            object_path: &str,
        ) -> Result<(), glib::Error> {
            // Multiple approaches were considered for sending a list of paths from
            // the nautilus extension to the instance of app, or to launch the app
            // first if it wasn't running, such as:
            //
            // 1. Figuring out the exec command from the Desktop file using
            // Gio.DesktopAppInfo and running the command with some `--send-files`
            // arg that takes in a list of paths. But, the issue being in passing
            // these options from a remote instance to the primary instance of the
            // app while using the same binary instead. `is_remote()` couldn't be
            // used to set the HANDLES_COMMAND_LINE application flag dynamically.
            //
            // 2. Having our own D-Bus API using zbus, but that'd require us to use a
            // separate service name and file than the one GApplication will be
            // using.
            //
            // 3. Or, to export our Dbus objects using DBusConnection, unfortunately
            // the documentation is quite lackluster. But, thankfully it allows for
            // exporting ActionGroup which is easy enough to do even though the
            // resulting generated API is not too ergonomic.
            //
            // #3 is currently how it's implemented. The ActionGroup is exported
            // under `/<APP_ID Path>/Share` and the action can be called from the
            // `Activate` method over the interface `org.gtk.Actions` with parameters
            // such as:
            //
            // ```
            // ('send-files', [<['~/file_1', '~/file_2']>], {})
            // ```
            let group = gio::SimpleActionGroup::new();
            {
                let send_files_action =
                    gio::SimpleAction::new("send-files", Some(glib::VariantTy::STRING_ARRAY));
                send_files_action.connect_activate(clone!(
                    #[weak(rename_to = this)]
                    self,
                    move |_, variant| {
                        if let Some(variant) = variant {
                            let files = variant
                                .get::<Vec<String>>()
                                .expect("Parameter of the Action isn't array of string");
                            glib::spawn_future_local(clone!(
                                #[weak]
                                this,
                                async move {
                                    _ = this
                                        .send_files_channel
                                        .0
                                        .send(files)
                                        .await
                                        .inspect_err(|err| tracing::warn!("{err:#}"));
                                }
                            ));
                        }
                    }
                ));
                group.add_action(&send_files_action);
            }

            connection.export_action_group(&format!("{object_path}/Share"), &group)?;

            Ok(())
        }

        fn handle_local_options(&self, options: &glib::VariantDict) -> glib::ExitCode {
            self.obj().handle_command_line(options);
            self.parent_handle_local_options(options)
        }

        fn shutdown(&self) {
            debug!("GtkApplication<PacketApplication>::shutdown");
            self.parent_shutdown();
        }
    }

    impl GtkApplicationImpl for PacketApplication {}
    impl AdwApplicationImpl for PacketApplication {}
}

glib::wrapper! {
    pub struct PacketApplication(ObjectSubclass<imp::PacketApplication>)
        @extends gio::Application, gtk::Application, adw::Application,
        @implements gio::ActionMap, gio::ActionGroup;
}

impl PacketApplication {
    fn main_window(&self) -> PacketApplicationWindow {
        self.imp().window.get().unwrap().upgrade().unwrap()
    }

    fn setup_gactions(&self) {
        // Quit
        let action_quit = gio::ActionEntry::builder("quit")
            .activate(move |app: &Self, _, _| {
                tracing::debug!("Invoked action app.quit");

                // On GNOME, closing the background app from their "Background Apps" UI seems to invoke app.quit
                app.main_window().imp().should_quit.replace(true);

                app.main_window().close();
                app.quit();
            })
            .build();

        // About
        let action_about = gio::ActionEntry::builder("about")
            .activate(|app: &Self, _, _| {
                app.show_about_dialog();
            })
            .build();
        self.add_action_entries([action_quit, action_about]);
    }

    // Sets up keyboard shortcuts
    fn setup_accels(&self) {
        // This will close the app regardless of "Run in Background"
        self.set_accels_for_action("app.quit", &["<Control>q"]);

        self.set_accels_for_action("window.close", &["<Control>w"]);
        self.set_accels_for_action("win.preferences", &["<Control>comma"]);
        self.set_accels_for_action("win.help", &["F1"]);
    }

    fn setup_css(&self) {
        let provider = gtk::CssProvider::new();
        provider.load_from_resource("/io/github/nozwock/Packet/style.css");
        if let Some(display) = gdk::Display::default() {
            gtk::style_context_add_provider_for_display(
                &display,
                &provider,
                gtk::STYLE_PROVIDER_PRIORITY_APPLICATION,
            );
        }
    }

    #[allow(dead_code)]
    fn authors() -> Vec<&'static str> {
        // Authors are defined in Cargo.toml
        env!("CARGO_PKG_AUTHORS").split(":").collect()
    }

    fn show_about_dialog(&self) {
        // Reference:
        // https://gnome.pages.gitlab.gnome.org/libadwaita/doc/1.7/class.AboutDialog.html
        // https://github.com/youpie/Iconic/blob/main/src/application.rs
        let dialog = adw::AboutDialog::builder()
            .application_name(gettext(
                // Translators: The name should remain untranslated
                "Packet",
            ))
            .application_icon(APP_ID)
            .version(VERSION)
            .developer_name("nozwock")
            // Legal section
            .license_type(gtk::License::Gpl30)
            // Details section
            .website("https://github.com/nozwock/packet")
            // Credits and Acknowledgements section
            // format: "Name https://example.com" or "Name <email@example.com>"
            .developers(["nozwock https://github.com/nozwock"])
            .designers(["Dominik Baran https://gitlab.gnome.org/wallaby"])
            .translator_credits(gettext(
                // Translators: Replace "translator-credits" with your names, one name per line
                "translator-credits",
            ))
            // Troubleshooting section
            .issue_url("https://github.com/nozwock/packet/issues")
            .debug_info_filename("packet.log")
            .debug_info(&gettext("Loading logs..."))
            .build();

        dialog.add_acknowledgement_section(
            Some(&gettext("Similar Projects")),
            &[
                "NearDrop https://github.com/grishka/NearDrop/",
                "rquickshare https://github.com/Martichou/rquickshare/",
            ],
        );

        // One issue with this approach is that the logs in the dialog will not
        // be updated unless the dialog is repopened.
        glib::spawn_future_local(clone!(
            #[weak]
            dialog,
            async move {
                let logs = tokio_runtime()
                    .spawn_blocking(move || -> anyhow::Result<_> {
                        Ok(fs_err::read_to_string(packet_log_path())?)
                    })
                    .await
                    .map_err(|err| anyhow::anyhow!(err))
                    .and_then(|it| it)
                    .map_err(|err| err.context(gettext("Failed to retrieve the logs")))
                    .inspect_err(|err| tracing::warn!("{err:#}"))
                    .unwrap_or_else(|err| format!("{err:#}"));

                dialog.set_debug_info(&logs);
            }
        ));

        dialog.present(Some(&self.main_window()));
    }

    fn handle_command_line(&self, options: &glib::VariantDict) {
        let imp = self.imp();

        tracing::debug!(
            background = ?options.lookup::<bool>("background"),
            "Processing command line options"
        );

        imp.start_in_background
            .replace(options.contains("background"));
    }

    fn setup_command_line_options(&self) {
        self.add_main_option(
            "background",
            b'b'.into(),
            glib::OptionFlags::NONE,
            glib::OptionArg::None,
            "Start the application in background",
            None,
        );
    }

    pub fn run(&self) -> glib::ExitCode {
        info!("Packet ({})", APP_ID);
        info!("Version: {} ({})", VERSION, PROFILE);
        info!("Datadir: {}", PKGDATADIR);

        self.setup_command_line_options();

        ApplicationExtManual::run(self)
    }
}

impl Default for PacketApplication {
    fn default() -> Self {
        glib::Object::builder()
            .property("application-id", APP_ID)
            .property("resource-base-path", "/io/github/nozwock/Packet/")
            .build()
    }
}
