use gettextrs::gettext;

use crate::config::APP_ID;

#[derive(Debug)]
pub struct Tray {
    pub tx: tokio::sync::mpsc::Sender<TrayMessage>,
}

#[derive(Debug, Clone)]
pub enum TrayMessage {
    OpenWindow,
    Quit,
}

impl ksni::Tray for Tray {
    const MENU_ON_ACTIVATE: bool = true;

    fn id(&self) -> String {
        APP_ID.into()
    }
    fn icon_name(&self) -> String {
        "io.github.nozwock.Packet-symbolic".into()
    }
    // https://github.com/ubuntu/gnome-shell-extension-appindicator
    // While the extension supporting tray icons on GNOME seem to be able to find the symbolic icon for the sandboxed
    // app in `~/.var/app/{APP_ID}/data/icons/`, other desktop environments' (e.g. KDE Plasma) SNI implementation
    // doesn't seem to able to do so. Pointing to the sandbox directory seems to allow the DE to find the tray icon.
    fn icon_theme_path(&self) -> String {
        gtk::glib::user_data_dir()
            .join("icons")
            .to_string_lossy()
            .into()
    }
    fn title(&self) -> String {
        gettext("Packet")
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        use ksni::menu::*;
        vec![
            StandardItem {
                label: gettext("Open"),
                activate: Box::new(move |this: &mut Self| {
                    _ = this.tx.try_send(TrayMessage::OpenWindow);
                }),
                ..Default::default()
            }
            .into(),
            StandardItem {
                label: gettext("Exit"),
                icon_name: "application-exit-symbolic".into(),
                activate: Box::new(move |this: &mut Self| {
                    _ = this.tx.try_send(TrayMessage::Quit);
                }),
                ..Default::default()
            }
            .into(),
        ]
    }
}
