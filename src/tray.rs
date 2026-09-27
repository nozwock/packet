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
        format!("{APP_ID}-symbolic")
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
