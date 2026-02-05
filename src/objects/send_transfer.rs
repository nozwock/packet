use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use rqs_lib::hdl::{
    TextPayloadType,
    info::{TransferPayload, TransferPayloadKind},
};

use crate::{ext::MessageExt, impl_deref_for_newtype, utils};

#[derive(Debug, Clone, Default, glib::Boxed)]
#[boxed_type(name = "StateBoxed")]
pub struct State(pub rqs_lib::TransferState);
impl_deref_for_newtype!(State, rqs_lib::TransferState);

#[derive(Debug, Clone, Default, glib::Boxed)]
#[boxed_type(name = "EndpointInfoBoxed")]
pub struct EndpointInfo(pub rqs_lib::EndpointInfo);
impl_deref_for_newtype!(EndpointInfo, rqs_lib::EndpointInfo);

impl std::fmt::Display for EndpointInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "{{ id={:?} present={:?} name={:?} }}",
            self.id,
            self.present.unwrap_or_default(),
            self.name.as_ref().map(|it| it.as_str()).unwrap_or_default(),
        )
    }
}

#[derive(Debug, Clone, glib::Boxed)]
#[boxed_type(name = "ChannelMessageBoxed", nullable)]
pub struct ChannelMessage(pub rqs_lib::channel::ChannelMessage);
impl_deref_for_newtype!(ChannelMessage, rqs_lib::channel::ChannelMessage);

impl ChannelMessage {
    pub fn device_name(&self) -> String {
        self.msg
            .as_client_unchecked()
            .metadata
            .as_ref()
            .and_then(|meta| meta.source.as_ref())
            .map(|source| source.name.clone())
            .unwrap_or(gettext("Unknown device"))
    }

    pub fn files(&self) -> Option<&Vec<String>> {
        self.msg
            .as_client_unchecked()
            .metadata
            .as_ref()
            .and_then(|it| match &it.payload {
                Some(TransferPayload::Files(files)) => Some(files),
                _ => None,
            })
    }

    pub fn text_preview(&self) -> Option<String> {
        self.msg
            .as_client_unchecked()
            .metadata
            .as_ref()
            .and_then(|meta| meta.payload_preview.clone())
    }

    pub fn is_text_type(&self) -> bool {
        self.msg
            .as_client_unchecked()
            .metadata
            .as_ref()
            .map(|meta| match meta.payload_kind {
                TransferPayloadKind::Text
                | TransferPayloadKind::Url
                | TransferPayloadKind::WiFi => true,
                TransferPayloadKind::Files => false,
            })
            .unwrap_or_default()
    }

    pub fn transferred_text_data(&self) -> Option<(String, TextPayloadType)> {
        self.msg
            .as_client_unchecked()
            .metadata
            .as_ref()
            .and_then(|meta| match &meta.payload {
                Some(TransferPayload::Text(text)) => Some((text.clone(), TextPayloadType::Text)),
                Some(TransferPayload::Url(text)) => Some((text.clone(), TextPayloadType::Url)),
                Some(TransferPayload::Wifi {
                    ssid,
                    password,
                    security_type: _,
                }) => Some((format!("{ssid}: {password}"), TextPayloadType::Wifi)),
                _ => None,
            })
    }
}

#[derive(Debug, Clone, Default, PartialEq, glib::Boxed)]
#[boxed_type(name = "TransferStateBoxed")]
pub enum TransferState {
    Queued,
    #[default]
    AwaitingConsentOrIdle,
    RequestedForConsent,
    OngoingTransfer,
    Failed,
    Done,
}

pub mod imp {
    use std::{cell::RefCell, rc::Rc};

    use gtk::glib::Properties;

    use super::*;

    #[derive(Debug, Default, Properties)]
    #[properties(wrapper_type = super::SendRequestState)]
    pub struct SendTransferState {
        pub eta: Rc<RefCell<utils::DataTransferEta>>,
        pub files: Rc<RefCell<Vec<String>>>,
        pub pending_text: Rc<RefCell<Option<(String, rqs_lib::TextPayloadType)>>>,

        #[property(get, set)]
        transfer_state: RefCell<TransferState>,
        #[property(get, set)]
        device_name: RefCell<String>,

        // For modifying widget by listening for events
        #[property(get, set)]
        endpoint_info: RefCell<EndpointInfo>,
        #[property(get, set, nullable)]
        event: RefCell<Option<ChannelMessage>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SendTransferState {
        const NAME: &'static str = "PacketSendTransferState";
        type Type = super::SendRequestState;
    }

    #[glib::derived_properties]
    impl ObjectImpl for SendTransferState {}
}

glib::wrapper! {
    pub struct SendRequestState(ObjectSubclass<imp::SendTransferState>);
}

impl SendRequestState {
    pub fn new() -> Self {
        Default::default()
    }
    pub fn copy(&self) -> Self {
        let obj = Self::new();
        obj.set_endpoint_info(self.endpoint_info());
        obj.set_event(self.event());
        obj.set_device_name(self.device_name());
        *obj.imp().eta.borrow_mut() = self.imp().eta.borrow().clone();
        *obj.imp().files.borrow_mut() = self.imp().files.borrow().clone();
        *obj.imp().pending_text.borrow_mut() = self.imp().pending_text.borrow().clone();

        obj
    }
}

impl Default for SendRequestState {
    fn default() -> Self {
        glib::Object::builder().build()
    }
}
