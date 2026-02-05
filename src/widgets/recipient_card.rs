use crate::{
    ext::MessageExt,
    objects::{self, TransferState, send_transfer::SendRequestState},
    tokio_runtime,
    window::PacketApplicationWindow,
};

use adw::prelude::*;
use adw::subclass::prelude::*;
use formatx::formatx;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib, glib::clone};
use rqs_lib::channel::{ChannelMessage, MessageClient};

fn get_model_item_from_listbox_row<T>(
    model: &gio::ListStore,
    list_box: &gtk::ListBox,
    row: &gtk::ListBoxRow,
) -> Option<T>
where
    T: IsA<glib::Object>,
{
    let mut pos = 0;
    while let Some(x) = list_box.row_at_index(pos) {
        if x == *row {
            break;
        }
        pos = pos + 1;
    }

    model
        .item(pos as u32)
        .and_then(|it| it.downcast::<T>().ok())
}

/// Don't try to reuse a ListBoxRow...\
/// ListBoxRow can be attached to a different model's widget
fn get_listbox_row_from_model_item<T>(
    model: &gio::ListStore,
    list_box: &gtk::ListBox,
    model_item: &T,
) -> Option<gtk::ListBoxRow>
where
    T: IsA<glib::Object>,
{
    let mut pos = 0;
    while let Some(x) = model.item(pos) {
        if x.downcast_ref::<T>()? == model_item {
            break;
        }
        pos = pos + 1;
    }

    list_box.row_at_index(pos as i32)
}

pub fn handle_recipient_card_clicked(
    win: &PacketApplicationWindow,
    list_box: &gtk::ListBox,
    row: &gtk::ListBoxRow,
) {
    let imp = win.imp();

    let model_item =
        get_model_item_from_listbox_row::<SendRequestState>(&imp.recipient_model, list_box, row)
            .expect("Index should be valid since model and ListBox are related");

    emit_send_files(win, &model_item);

    // Only reset this on Cancelled
    row.set_activatable(false);
}

fn emit_send_files(win: &PacketApplicationWindow, model_item: &SendRequestState) {
    let imp = win.imp();

    let endpoint_info = model_item.endpoint_info();
    let files_to_send = model_item.imp().files.borrow().clone();
    let pending_text = model_item.imp().pending_text.borrow().clone();

    // Determine payload type
    let outbound_payload = if let Some((text, text_type)) = pending_text {
        rqs_lib::OutboundPayload::Text { text, text_type }
    } else {
        rqs_lib::OutboundPayload::Files(files_to_send)
    };

    // Only one transfer at a time is supported by the protocol
    // Whether it be receiving or sending
    let will_be_queued = imp
        .recipient_model
        .iter::<SendRequestState>()
        .filter_map(|it| it.ok())
        .find(|it| match it.transfer_state() {
            TransferState::RequestedForConsent | TransferState::OngoingTransfer => true,
            _ => false,
        })
        .is_some();
    if will_be_queued {
        model_item.set_transfer_state(TransferState::Queued);
    }

    tokio_runtime().spawn(clone!(
        #[weak(rename_to = file_sender)]
        imp.file_sender,
        // #[weak]
        // model_item,
        async move {
            // FIXME: Set Failed state on Err and update UI on Failed state change
            // model_item.set_transfer_state(TransferState::Failed);
            file_sender
                .lock()
                .await
                .as_mut()
                .expect("RQS .file_sender must be set")
                .send(rqs_lib::SendInfo {
                    id: endpoint_info.id.clone(),
                    name: endpoint_info
                        .name
                        .clone()
                        .unwrap_or(gettext("Unknown device")),
                    addr: format!(
                        "{}:{}",
                        endpoint_info.ip.clone().unwrap_or_default(),
                        endpoint_info.port.clone().unwrap_or_default()
                    ),
                    ob: outbound_payload,
                })
                .await
                .unwrap();
        }
    ));
}

pub fn create_recipient_card(
    win: &PacketApplicationWindow,
    _model: &gio::ListStore,
    model_item: &SendRequestState,
    init_model_state: Option<()>,
) -> adw::Bin {
    let imp = win.imp();

    if init_model_state.is_some() {
        model_item.set_device_name(model_item.endpoint_info().name.clone().unwrap_or_default());

        // Check if we're sending text or files
        let pending_text = imp.pending_text.borrow().clone();
        if let Some((text, text_type)) = pending_text {
            // Sending text
            *model_item.imp().pending_text.borrow_mut() = Some((text.clone(), text_type));

            let eta_estimator = &model_item.imp().eta;
            eta_estimator
                .borrow_mut()
                .prepare_for_new_transfer(Some(text.len()));
        } else {
            // Sending files
            let files_to_send = imp
                .manage_files_model
                .iter::<gio::File>()
                .filter_map(|it| it.ok())
                .filter_map(|it| it.path())
                .map(|it| it.to_string_lossy().to_string())
                .collect::<Vec<_>>();
            *model_item.imp().files.borrow_mut() = files_to_send;

            let eta_estimator = &model_item.imp().eta;
            if eta_estimator.borrow().total_len == 0 {
                let total_size = imp
                    .manage_files_model
                    .iter::<gio::File>()
                    .filter_map(|it| it.ok())
                    .filter_map(|it| {
                        it.query_info(
                            gio::FILE_ATTRIBUTE_STANDARD_SIZE,
                            gio::FileQueryInfoFlags::NONE,
                            None::<&gio::Cancellable>,
                        )
                        .ok()
                    })
                    .map(|it| it.size() as usize)
                    .fold(0, |acc, x| acc + x);

                eta_estimator
                    .borrow_mut()
                    .prepare_for_new_transfer(Some(total_size));
            }
        }

        if model_item.endpoint_info().present.is_some() {
            let title = model_item
                .endpoint_info()
                .name
                .clone()
                .unwrap_or(gettext("Unknown device").into());
            model_item.set_device_name(title.clone());
        }
    }

    // `card` style will be applied with `boxed-list*` on ListBox
    // v/h-align would prevent the card from expanding when space is available
    let root_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Horizontal)
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .spacing(12)
        .build();
    let root_bin = adw::Bin::builder().child(&root_box).build();

    let device_avatar = adw::Avatar::builder().show_initials(true).size(48).build();
    model_item
        .bind_property("device-name", &device_avatar, "text")
        .sync_create()
        .build();
    root_box.append(&device_avatar);

    let right_box = gtk::Box::builder().build();
    root_box.append(&right_box);

    let main_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .spacing(4)
        .build();
    right_box.append(&main_box);

    let title_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .css_classes(["title-4"])
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .build();
    model_item
        .bind_property("device-name", &title_label, "label")
        .sync_create()
        .build();
    let result_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .visible(false)
        .build();
    let unavailibility_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .label(&gettext("Unavailable"))
        .visible(false)
        .css_classes(["dimmed"])
        .build();
    let pincode_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .visible(false)
        .css_classes(["dimmed", "monospace"])
        .build();
    main_box.append(&title_label);
    main_box.append(&result_label);
    main_box.append(&unavailibility_label);
    main_box.append(&pincode_label);

    model_item.connect_transfer_state_notify(clone!(
        #[weak]
        imp,
        #[weak]
        result_label,
        move |model_item| {
            if model_item.transfer_state() == TransferState::Queued {
                result_label.set_visible(true);
                result_label.set_label(&gettext("Queued"));
                result_label.set_css_classes(&[]);
            };

            // Prevent exiting the recipients view until all transfers
            // are settled
            let is_transfer_active = imp
                .recipient_model
                .iter::<SendRequestState>()
                .filter_map(|it| it.ok())
                .find(|it| match it.transfer_state() {
                    TransferState::Queued
                    | TransferState::RequestedForConsent
                    | TransferState::OngoingTransfer => true,
                    TransferState::AwaitingConsentOrIdle
                    | TransferState::Failed
                    | TransferState::Done => false,
                })
                .is_some();
            if is_transfer_active {
                imp.select_recipients_dialog.set_can_close(false);
            } else {
                imp.select_recipients_dialog.set_can_close(true);
            }
        }
    ));

    let progress_bar = gtk::ProgressBar::builder().visible(false).build();
    main_box.append(&progress_bar);

    let eta_label = gtk::Label::builder()
        .halign(gtk::Align::Start)
        .wrap(true)
        .visible(false)
        .css_classes(["caption", "dim-label"])
        .build();
    main_box.append(&eta_label);

    let id = model_item.endpoint_info().id.clone();

    root_box.append(&adw::Bin::builder().hexpand(true).build());

    let retry_button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .icon_name("view-refresh-symbolic")
        .css_classes(["circular", "flat"])
        .tooltip_text(&gettext("Retry"))
        .visible(false)
        .build();
    root_box.append(&retry_button);
    retry_button.connect_clicked(clone!(
        #[weak]
        imp,
        #[weak]
        model_item,
        move |_button| {
            emit_send_files(&imp.obj(), &model_item);
        }
    ));

    let cancel_transfer_button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .icon_name("cross-large-symbolic")
        .css_classes(["circular", "flat"])
        .tooltip_text(&gettext("Cancel"))
        .visible(false)
        .build();
    root_box.append(&cancel_transfer_button);

    cancel_transfer_button.connect_clicked(clone!(
        #[weak(rename_to = rqs)]
        imp.rqs,
        #[strong]
        id,
        move |_button| {
            let mut guard = rqs.blocking_lock();
            if let Some(rqs) = guard.as_mut() {
                _ = rqs
                    .message_sender
                    .send(ChannelMessage {
                        id: id.clone(),
                        msg: rqs_lib::channel::Message::Lib {
                            action: rqs_lib::channel::TransferAction::TransferCancel,
                        },
                    })
                    .inspect_err(|err| tracing::error!(%err));
            }
        }
    ));

    fn set_progress_bar_fraction(progress_bar: &gtk::ProgressBar, client_msg: &MessageClient) {
        if let Some(metadata) = &client_msg.metadata {
            if metadata.total_bytes > 0 {
                progress_bar.set_fraction(metadata.ack_bytes as f64 / metadata.total_bytes as f64);
            }
        }
    }

    fn set_row_activatable(
        model_item: &SendRequestState,
        row: Option<&gtk::ListBoxRow>,
        activatable: bool,
    ) {
        if let Some(row) = row {
            if model_item.endpoint_info().present.is_none() {
                row.set_activatable(false);
            } else {
                row.set_activatable(activatable);
            }
        }
    }

    model_item.connect_endpoint_info_notify(clone!(
        #[weak]
        win,
        #[weak]
        retry_button,
        #[weak]
        unavailibility_label,
        move |model_item| {
            let imp = win.imp();
            let is_idle_card = model_item.transfer_state() == TransferState::AwaitingConsentOrIdle;
            if let Some(row) = get_listbox_row_from_model_item::<SendRequestState>(
                &imp.recipient_model,
                &imp.recipient_listbox,
                model_item,
            ) {
                set_row_activatable(model_item, Some(&row), is_idle_card);
            };

            let endpoint_info = model_item.endpoint_info();
            if endpoint_info.present.is_none() {
                retry_button.set_sensitive(false);
                unavailibility_label.set_visible(is_idle_card);
            } else {
                retry_button.set_sensitive(true);
                unavailibility_label.set_visible(false);

                // Update device name on re-connection
                let title = endpoint_info
                    .name
                    .as_ref()
                    .map(|s| s.as_str())
                    .unwrap_or("Unknown Device");
                model_item.set_device_name(title);
            }
        }
    ));
    model_item.connect_event_notify(clone!(
        #[weak]
        imp,
        move |model_item| {
            use rqs_lib::TransferState as RqsState;

            let eta_estimator = model_item.imp().eta.as_ref();

            if let Some(event_msg) = model_item.event() {
                let client_msg = event_msg.msg.as_client_unchecked();
                let state = client_msg.state.as_ref().unwrap_or(&RqsState::Initial);

                match state {
                    RqsState::Initial => {}
                    RqsState::ReceivedConnectionRequest => {}
                    RqsState::SentUkeyServerInit => {}
                    RqsState::SentPairedKeyEncryption => {}
                    RqsState::ReceivedUkeyClientFinish => {}
                    RqsState::SentConnectionResponse => {}
                    RqsState::SentPairedKeyResult => {}
                    RqsState::ReceivedPairedKeyResult => {}
                    RqsState::WaitingForUserConsent => {}
                    RqsState::ReceivingFiles => {}
                    RqsState::SentUkeyClientInit
                    | RqsState::SentUkeyClientFinish
                    | RqsState::SentIntroduction => {
                        model_item.set_transfer_state(TransferState::RequestedForConsent);

                        let listbox_row = get_listbox_row_from_model_item::<SendRequestState>(
                            &imp.recipient_model,
                            &imp.recipient_listbox,
                            model_item,
                        );
                        set_row_activatable(model_item, listbox_row.as_ref(), false);

                        unavailibility_label.set_visible(false);
                        retry_button.set_visible(false);

                        cancel_transfer_button.set_sensitive(true);
                        cancel_transfer_button.set_visible(true);

                        result_label.set_visible(true);
                        result_label.set_label(&gettext("Requested"));
                        result_label.set_css_classes(&["accent"]);

                        pincode_label.set_visible(true);
                        pincode_label.set_label(
                            &formatx!(
                                gettext("Code: {}"),
                                client_msg
                                    .metadata
                                    .as_ref()
                                    .map(|it| it.pin_code.as_ref().map(|it| it.as_str()))
                                    .flatten()
                                    .unwrap_or("???")
                            )
                            .unwrap_or_else(|_| "badly formatted locale string".into()),
                        );

                        eta_estimator.borrow_mut().prepare_for_new_transfer(None);
                    }
                    RqsState::SendingFiles => {
                        model_item.set_transfer_state(TransferState::OngoingTransfer);

                        cancel_transfer_button.set_visible(true);
                        result_label.set_visible(false);
                        unavailibility_label.set_visible(false);
                        pincode_label.set_visible(false);
                        retry_button.set_visible(false);

                        let eta_text = {
                            if let Some(metadata) = &client_msg.metadata {
                                eta_estimator
                                    .borrow_mut()
                                    .step_with(metadata.ack_bytes as usize);
                            }

                            formatx!(
                                gettext("About {} left"),
                                eta_estimator.borrow().get_estimate_string().trim()
                            )
                            .unwrap_or_else(|_| "badly formatted locale string".into())
                        };
                        eta_label.set_visible(true);
                        eta_label.set_label(&eta_text);

                        progress_bar.set_visible(true);
                        set_progress_bar_fraction(&progress_bar, &client_msg);
                    }
                    RqsState::Disconnected => {
                        model_item.set_transfer_state(TransferState::Failed);
                        // FIXME: Wait for 5~10 seconds after a send and timeout
                        // if did not receive SendingFiles within that timeframe
                        // This is how google does it in their client

                        progress_bar.set_visible(false);
                        cancel_transfer_button.set_visible(false);
                        eta_label.set_visible(false);
                        unavailibility_label.set_visible(false);
                        pincode_label.set_visible(false);

                        retry_button.set_visible(true);

                        result_label.set_visible(true);
                        result_label.set_label(&gettext("Failed"));
                        result_label.set_css_classes(&["error"]);
                    }
                    RqsState::Rejected => {
                        model_item.set_transfer_state(TransferState::Failed);
                        // Outbound(Reject) is not handled on lib side
                        // rqs_lib::hdl::outbound: Cannot process: consent denied: Reject
                    }
                    RqsState::Cancelled => {
                        model_item.set_transfer_state(TransferState::AwaitingConsentOrIdle);

                        let listbox_row = get_listbox_row_from_model_item::<SendRequestState>(
                            &imp.recipient_model,
                            &imp.recipient_listbox,
                            model_item,
                        );
                        set_row_activatable(model_item, listbox_row.as_ref(), true);

                        progress_bar.set_visible(false);
                        cancel_transfer_button.set_visible(false);
                        eta_label.set_visible(false);
                        result_label.set_visible(false);
                        retry_button.set_visible(false);
                        pincode_label.set_visible(false);

                        unavailibility_label
                            .set_visible(model_item.endpoint_info().present.is_none());

                        model_item.set_event(None::<objects::ChannelMessage>);
                    }
                    RqsState::Finished => {
                        model_item.set_transfer_state(TransferState::Done);

                        cancel_transfer_button.set_visible(false);
                        progress_bar.set_visible(false);
                        eta_label.set_visible(false);
                        retry_button.set_visible(false);
                        unavailibility_label.set_visible(false);
                        pincode_label.set_visible(false);

                        let finished_text = {
                            if model_item.imp().pending_text.borrow().is_some() {
                                gettext("Text sent")
                            } else {
                                let file_count = model_item.imp().files.borrow().len();
                                formatx!(
                                    ngettext("Sent {} file", "Sent {} files", file_count as u32),
                                    file_count
                                )
                                .unwrap_or_else(|_| "badly formatted locale string".into())
                            }
                        };

                        result_label.set_visible(true);
                        result_label.set_label(&finished_text);
                        result_label.set_css_classes(&["accent"]);
                    }
                };
            }
        }
    ));

    // Set initial widget state based on model's state
    model_item.notify_endpoint_info();
    model_item.notify_event();

    root_bin
}
