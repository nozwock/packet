mod application;
#[rustfmt::skip]
mod config;
mod constants;
mod monitors;
mod objects;
mod plugins;
mod utils;
mod widgets;
mod window;

use gettextrs::{LocaleCategory, gettext};
use gtk::{gio, glib};
use tracing::level_filters::LevelFilter;
use tracing_subscriber::EnvFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::constants::packet_log_path;

use self::application::PacketApplication;
use self::config::{GETTEXT_PACKAGE, LOCALEDIR, RESOURCES_FILE};

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
    let (file_writer, _file_guard) = tracing_appender::non_blocking(
        fs_err::File::create(packet_log_path()).expect("Couldn't create the log file"),
    );
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

    let res = gio::Resource::load(RESOURCES_FILE).expect("Could not load gresource file");
    gio::resources_register(&res);

    let app = PacketApplication::default();
    app.run()
}

pub fn tokio_runtime() -> &'static tokio::runtime::Runtime {
    use std::sync::OnceLock;
    static RUNTIME: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RUNTIME.get_or_init(|| tokio::runtime::Runtime::new().expect("Couldn't get tokio runtime"))
}
