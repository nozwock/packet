mod application;
#[rustfmt::skip]
mod config;
mod constants;
mod dbus;
mod ext;
mod monitors;
mod objects;
mod plugins;
#[cfg(target_os = "linux")]
mod tray;
mod utils;
mod widgets;
mod window;

use std::env;

use gettextrs::{LocaleCategory, gettext};
use gtk::{gio, glib};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::config::APP_ID;
use crate::constants::packet_log_path;

use self::application::Application;
use self::config::{GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE, UI_RESOURCES_FILE};

fn main() -> glib::ExitCode {
    let env_filter = if std::env::var_os("RUST_LOG").is_none() {
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .parse("packet=debug,rqs_lib=debug")
            .expect("Log level directive isn't valid")
    } else {
        EnvFilter::builder()
            .with_default_directive(LevelFilter::INFO.into())
            .from_env_lossy()
    };

    let stdout_layer = tracing_subscriber::fmt::layer().with_line_number(true);
    let log_path = packet_log_path();
    if let Some(parent) = log_path.parent() {
        _ = fs_err::create_dir_all(parent);
    }
    let log_writer = rolling_file::BasicRollingFileAppender::new(
        log_path,
        rolling_file::RollingConditionBasic::new().max_size(constants::MAX_LOG_FILE_SIZE),
        1,
    )
    .expect("Couldn't create the log file");
    let (file_writer, _file_guard) = tracing_appender::non_blocking(log_writer);
    let file_layer = tracing_subscriber::fmt::layer()
        .with_writer(file_writer)
        .with_line_number(true)
        .with_ansi(false);

    // Initialize logger
    tracing_subscriber::registry()
        .with(stdout_layer)
        .with(file_layer)
        .with(env_filter)
        .init();

    // Prepare i18n
    gettextrs::setlocale(LocaleCategory::LcAll, "");
    gettextrs::bindtextdomain(GETTEXT_PACKAGE, LOCALEDIR).expect("Unable to bind the text domain");
    gettextrs::textdomain(GETTEXT_PACKAGE).expect("Unable to switch to the text domain");

    glib::set_application_name(&gettext("Packet"));

    for (res_env, res_path) in [
        ("RESOURCE_FILE", RESOURCES_FILE),
        ("UI_RESOURCE_FILE", UI_RESOURCES_FILE),
    ] {
        let res = env::var("MESON_DEVENV")
            .ok()
            .and_then(|_| env::var(res_env).ok())
            .map(|path| gio::Resource::load(&path).expect("Could not load gresource file"))
            .unwrap_or_else(|| {
                gio::Resource::load(res_path).expect("Could not load gresource file")
            });
        gio::resources_register(&res);
    }

    // `packet --send-files <paths..>` hands the files over to the primary
    // instance (D-Bus activating it if needed) and exits. Used by the Dolphin
    // service menu.
    let args = env::args().collect::<Vec<_>>();
    if args.get(1).map(String::as_str) == Some("--send-files") {
        return send_files_to_primary_instance(&args[2..]);
    }

    tokio_runtime().block_on(async move {
        if let Err(err) = ashpd::register_host_app(APP_ID.try_into().unwrap()).await {
            tracing::warn!("Failed to register host app: {err}");
        }
    });

    let app = Application::default();
    app.run()
}

/// Calls the `send-files` action exported under `/<APP_ID Path>/Share`.
/// See `dbus_register` in application.rs.
fn send_files_to_primary_instance(paths: &[String]) -> glib::ExitCode {
    use gtk::glib::prelude::*;

    let files = paths
        .iter()
        .map(|it| std::path::absolute(it).unwrap_or_else(|_| it.into()))
        .map(|it| it.to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    if files.is_empty() {
        tracing::error!("No files were passed to --send-files");
        return glib::ExitCode::FAILURE;
    }

    let params = glib::Variant::tuple_from_iter([
        "send-files".to_variant(),
        vec![files.to_variant()].to_variant(),
        std::collections::HashMap::<String, glib::Variant>::new().to_variant(),
    ]);

    let result = gio::bus_get_sync(gio::BusType::Session, gio::Cancellable::NONE).and_then(
        |conn| {
            conn.call_sync(
                Some(APP_ID),
                &format!("/{}/Share", APP_ID.replace('.', "/")),
                "org.gtk.Actions",
                "Activate",
                Some(&params),
                None,
                gio::DBusCallFlags::NONE,
                -1,
                gio::Cancellable::NONE,
            )
        },
    );

    match result {
        Ok(_) => glib::ExitCode::SUCCESS,
        Err(err) => {
            tracing::error!(%err, "Failed to send files to Packet over D-Bus");
            glib::ExitCode::FAILURE
        }
    }
}

pub fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("Couldn't get tokio runtime"))
}
