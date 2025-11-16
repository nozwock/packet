use std::{cell::Cell, rc::Rc, time::Duration};

use adw::prelude::*;
use adw::subclass::prelude::*;
use ashpd::desktop::notification::{DisplayHint, Notification, Priority};
use formatx::formatx;
use gettextrs::{gettext, ngettext};
use gtk::{
    gio,
    glib::{self, clone},
};
use rqs_lib::hdl::TextPayloadType;
use tokio_util::sync::CancellationToken;

use crate::{
    objects::{self, UserAction},
    utils::{remove_notification, spawn_notification},
    window::PacketApplicationWindow,
};

pub fn display_text_type(value: &TextPayloadType) -> String {
    match value {
        TextPayloadType::Url => gettext("Link"),
        TextPayloadType::Text => gettext("Text"),
        TextPayloadType::Wifi => gettext("Wi-Fi"),
    }
}

// So, Quick Share wraps the string in `""\n` sometimes, it seem to differ based
// on where you're copying the text from. For e.g. sharing from the Github app doesn't
// wrap the string in quote, but it does when shared from Chrome.
// Don't think we can fix this on our side...
fn clean_text_payload(s: &str) -> &str {
    if s.starts_with('"') && s.ends_with("\"\n") {
        &s[1..s.len() - 2]
    } else {
        s
    }
}

fn clean_preview_text_payload(s: &str) -> &str {
    clean_text_payload(s)
        .trim_matches(|c| c == '"' || c == '\n')
        .lines()
        .next()
        .unwrap_or_default()
        .trim_matches(|c| c == '"' || c == '\n')
}

// Rewriting receive UI for the 4rd time ;(
// Using a chain of AlertDialog this time
pub fn present_receive_transfer_ui(
    win: &PacketApplicationWindow,
    receive_state: &objects::ReceiveTransferState,
    notification_id: String,
    auto_decline_ctk: CancellationToken,
) {
    let init_id = receive_state.event().unwrap().id.clone();
    let win = win.clone();

    // Progress dialog
    let is_user_cancelled = Rc::new(Cell::new(false));
    let progress_dialog = adw::AlertDialog::builder()
        .heading(&gettext("Receiving"))
        .width_request(200)
        .build();
    progress_dialog.add_responses(&[("cancel", &gettext("Cancel"))]);
    progress_dialog.set_default_response(None);
    progress_dialog.connect_response(
        None,
        clone!(
            #[weak]
            receive_state,
            move |dialog, response_id| {
                match response_id {
                    "cancel" => {
                        dialog.set_response_enabled("cancel", false);
                        receive_state.set_user_action(Some(UserAction::TransferCancel));
                    }
                    _ => {}
                }
            }
        ),
    );
    progress_dialog.set_can_close(false);

    let progress_stack = gtk::Stack::new();

    let progress_files_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(24)
        .margin_end(24)
        .spacing(12)
        .build();
    progress_stack.add_named(&progress_files_box, Some("progress_files"));

    let device_name = receive_state.event().unwrap().device_name();
    let device_name_box = create_device_name_box(&device_name);
    device_name_box.set_margin_bottom(4);
    progress_files_box.append(&device_name_box);

    let progress_bar = gtk::ProgressBar::new();
    progress_files_box.append(&progress_bar);
    let eta_label = gtk::Label::builder()
        .halign(gtk::Align::Center)
        .wrap(true)
        .css_classes(["dimmed"])
        .build();
    progress_files_box.append(&eta_label);

    let progress_text_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(24)
        .margin_end(24)
        .spacing(12)
        .build();

    let device_name_box = create_device_name_box(&device_name);
    device_name_box.set_margin_bottom(4);
    progress_text_box.append(&device_name_box);
    progress_text_box.append(&adw::Spinner::new());
    progress_stack.add_named(&progress_text_box, Some("progress_text"));

    progress_dialog.set_extra_child(Some(&progress_stack));

    fn create_device_name_box(device_name: &str) -> gtk::Box {
        let device_name_box = gtk::Box::builder()
            .halign(gtk::Align::Center)
            .spacing(8)
            .build();
        let avatar = adw::Avatar::builder()
            .text(device_name)
            .show_initials(true)
            .size(32)
            .build();
        device_name_box.append(&avatar);
        let device_label = gtk::Label::builder()
            .label(device_name)
            .halign(gtk::Align::Center)
            .css_classes(["title-4"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        device_name_box.append(&device_label);

        device_name_box
    }

    let consent_dialog = adw::AlertDialog::builder()
        .heading(&gettext("Incoming Transfer"))
        .width_request(200)
        .build();

    receive_state.connect_user_action_notify(clone!(
        #[weak]
        win,
        #[weak]
        progress_dialog,
        #[weak]
        consent_dialog,
        #[weak]
        is_user_cancelled,
        #[strong]
        auto_decline_ctk,
        #[strong]
        notification_id,
        move |receive_state| {
            // Cancel auto-decline
            if !auto_decline_ctk.is_cancelled() {
                auto_decline_ctk.cancel();
            }

            let event = receive_state.event().unwrap();
            match receive_state.user_action() {
                Some(UserAction::ConsentAccept) => {
                    consent_dialog.close();

                    win.imp()
                        .rqs
                        .blocking_lock()
                        .as_mut()
                        .unwrap()
                        .message_sender
                        .send(rqs_lib::channel::ChannelMessage {
                            id: event.id.to_string(),
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::ConsentAccept,
                            },
                        })
                        .unwrap();

                    // Update the notification
                    spawn_notification(
                        notification_id.clone(),
                        Notification::new(&event.device_name())
                            .body(gettext("Receiving...").as_str())
                            .priority(Priority::High)
                            .display_hint([DisplayHint::Persistent])
                            .default_action(None)
                            .button(ashpd::desktop::notification::Button::new(
                                &gettext("Cancel"),
                                "transfer-cancel",
                            )),
                    );

                    // Spawn progress dialog
                    progress_dialog.present(Some(&win));
                }
                Some(UserAction::ConsentDecline) => {
                    consent_dialog.close();
                    remove_notification(notification_id.clone());

                    win.imp()
                        .rqs
                        .blocking_lock()
                        .as_mut()
                        .unwrap()
                        .message_sender
                        .send(rqs_lib::channel::ChannelMessage {
                            id: event.id.to_string(),
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::ConsentDecline,
                            },
                        })
                        .unwrap();
                }
                Some(UserAction::TransferCancel) => {
                    progress_dialog.set_can_close(true);
                    progress_dialog.close();
                    remove_notification(notification_id.clone());

                    is_user_cancelled.replace(true);

                    win.imp()
                        .rqs
                        .blocking_lock()
                        .as_mut()
                        .unwrap()
                        .message_sender
                        .send(rqs_lib::channel::ChannelMessage {
                            id: event.id.to_string(),
                            msg: rqs_lib::channel::Message::Lib {
                                action: rqs_lib::channel::TransferAction::TransferCancel,
                            },
                        })
                        .unwrap();
                }
                None => {}
            };
        }
    ));

    receive_state.connect_event_notify(clone!(
        #[weak]
        win,
        #[strong]
        notification_id,
        move |receive_state| {
            use rqs_lib::TransferState;

            let event_msg = receive_state.event().expect("Property setter isn't nullable");
            let client_msg = event_msg.msg.as_client().unwrap();

            match client_msg.state.clone().unwrap_or(TransferState::Initial) {
                TransferState::Initial => {}
                TransferState::ReceivedConnectionRequest => {}
                TransferState::SentUkeyServerInit => {}
                TransferState::SentUkeyClientInit => {}
                TransferState::SentUkeyClientFinish => {}
                TransferState::SentPairedKeyEncryption => {}
                TransferState::ReceivedUkeyClientFinish => {}
                TransferState::SentConnectionResponse => {}
                TransferState::SentPairedKeyResult => {}
                TransferState::SentIntroduction => {}
                TransferState::ReceivedPairedKeyResult => {}
                TransferState::WaitingForUserConsent => {
                    consent_dialog.add_responses(&[
                        ("decline", &gettext("Decline")),
                        ("accept", &gettext("Accept")),
                    ]);
                    consent_dialog
                        .set_response_appearance("accept", adw::ResponseAppearance::Suggested);

                    consent_dialog.set_default_response(Some("decline"));
                    consent_dialog.set_close_response("close");

                    let info_box = gtk::Box::builder()
                        .orientation(gtk::Orientation::Vertical)
                        .halign(gtk::Align::Center)
                        .spacing(8)
                        .build();
                    consent_dialog.set_extra_child(Some(&info_box));

                    let device_name = event_msg.device_name();

                    let device_name_box = create_device_name_box(&device_name);
                    info_box.append(&device_name_box);

                    let total_bytes = client_msg.metadata.as_ref().unwrap().total_bytes;
                    let transfer_size = human_bytes::human_bytes(total_bytes as f64);

                    if let Some(files) = event_msg.files() {
                        let file_count = files.len();

                        let files_label = gtk::Label::builder()
                            .label(
                                formatx!(
                                    ngettext(
                                        // Translators: An e.g. "6 Files (42.3MB)"
                                        "{} file ({})",
                                        "{} files ({})",
                                        file_count as u32,
                                    ),
                                    file_count,
                                    transfer_size
                                )
                                .unwrap_or_else(|_| "badly formatted locale string".into()),
                            )
                            .halign(gtk::Align::Center)
                            .css_classes(["dimmed", "heading"])
                            .build();
                        info_box.append(&files_label);
                    } else {
                        let text_info_label = gtk::Label::builder()
                            .ellipsize(gtk::pango::EllipsizeMode::End)
                            .max_width_chars(36)
                            .label(
                                formatx!(
                                    gettext("Preview ({})"),
                                    clean_preview_text_payload(&event_msg.text_preview().unwrap())
                                )
                                .unwrap_or_else(|_| "badly formatted locale string".into()),
                            )
                            .halign(gtk::Align::Center)
                            .css_classes(["dimmed"])
                            .build();
                        info_box.append(&text_info_label);
                    }

                    let pincode_label = gtk::Label::builder()
                        .label(
                            formatx!(
                                gettext(
                                    // Translators: This is the pin-code for the transfer
                                    "Code: {}"
                                ),
                                client_msg
                                    .metadata
                                    .as_ref()
                                    .unwrap()
                                    .pin_code
                                    .clone()
                                    .unwrap_or_default()
                            )
                            .unwrap_or_else(|_| "badly formatted locale string".into()),
                        )
                        .halign(gtk::Align::Center)
                        .css_classes(["dimmed", "monospace"])
                        .build();
                    info_box.append(&pincode_label);

                    consent_dialog.connect_response(
                        None,
                        clone!(
                            #[weak]
                            receive_state,
                            move |_, response_id| {
                                match response_id {
                                    "accept" => {
                                        receive_state.set_user_action(Some(UserAction::ConsentAccept));
                                    }
                                    "decline" => {
                                        receive_state.set_user_action(Some(UserAction::ConsentDecline));
                                    }
                                    "close" => {
                                        // Incase close is called by us after receiving consent state
                                        // from notification
                                        if receive_state.user_action().is_none() {
                                            receive_state.set_user_action(Some(UserAction::ConsentDecline));
                                        }
                                    }
                                    _ => {
                                        unreachable!()
                                    }
                                };
                            }
                        ),
                    );

                    // TODO: Maybe make this configurable?
                    // Timeout: auto-decline after 1 minute
                    // Since we can't know if the user has simply closed the notification,
                    // we can't use it as a decline response unfortunately. The solution is
                    // to have a 1min timeout for incoming requests.
                    glib::spawn_future_local(clone!(
                        #[weak]
                        win,
                        #[strong]
                        receive_state,
                        #[strong]
                        auto_decline_ctk,
                        async move {
                            tokio::select! {
                                _ = futures_timer::Delay::new(Duration::from_mins(1)) => {
                                    if receive_state.user_action().is_none() {
                                        receive_state.set_user_action(Some(UserAction::ConsentDecline));
                                        win.imp().toast_overlay.add_toast(adw::Toast::new(&gettext("Request timed out")));
                                    }
                                }
                                _ = auto_decline_ctk.cancelled() => {}
                            }
                        }
                    ));

                    let body = formatx!(
                        gettext(
                            // Translators: This is when some device is sharing files or text
                            // e.g. (Someone's Phone wants to share 4 files)
                            // e.g. (Someone's Phone wants to share "lorem ipsum ...")
                            "{} wants to share {}"
                        ),
                        event_msg.device_name(),
                        if let Some(files) = event_msg.files() {
                            formatx!(
                                ngettext("{} File", "{} Files", files.len() as u32),
                                files.len()
                            )
                            .unwrap_or_default()
                        } else {
                            format!(
                                "\"{}\"",
                                clean_preview_text_payload(
                                    &event_msg.text_preview().unwrap(),
                                )
                            )
                        }
                    )
                    .unwrap_or_default();

                    // Use a static id, like the app id
                    // There will only be one request at a time anyways
                    // And, we'll also need to close the notification on exit
                    // or it'll persist otherwise
                    spawn_notification(
                        notification_id.clone(),
                        Notification::new(&gettext("Incoming Transfer"))
                            .default_action("accept")
                            .body(body.as_str())
                            .priority(Priority::High)
                            // Persistent doesn't work (the close button is still there), atleast with gnome portal
                            .display_hint([DisplayHint::Persistent])
                            .button(ashpd::desktop::notification::Button::new(
                                &gettext("Decline"),
                                "consent-decline",
                            ))
                            .button(ashpd::desktop::notification::Button::new(
                                &gettext("Accept"),
                                "consent-accept",
                            )),
                    );

                    consent_dialog.present(Some(&win));

                    // TODO: show a progress dialog for both but with a delay?
                    // Create Progress bar dialog
                    let total_bytes = client_msg.metadata.as_ref().unwrap().total_bytes;
                    receive_state
                        .imp()
                        .eta
                        .borrow_mut()
                        .prepare_for_new_transfer(Some(total_bytes as usize));
                    if event_msg.is_text_type() {
                        progress_stack.set_visible_child_name("progress_text");
                    }
                }
                TransferState::ReceivingFiles => {
                    if !event_msg.is_text_type() {
                        let eta_text = {
                            if let Some(meta) = &client_msg.metadata {
                                receive_state
                                    .imp()
                                    .eta
                                    .borrow_mut()
                                    .step_with(meta.ack_bytes as usize);

                                if meta.total_bytes > 0 {
                                    progress_bar.set_fraction(
                                        meta.ack_bytes as f64 / meta.total_bytes as f64,
                                    );
                                }
                            }

                            formatx!(
                                gettext(
                                    // Translators: {} will be replaced with an estimated remaining time string
                                    // e.g. "About 4 minutes 32 seconds left"
                                    "About {} left"
                                ),
                                receive_state
                                    .imp()
                                    .eta
                                    .borrow()
                                    .get_estimate_string()
                                    // Why does the estimate string has a random whitespace in the front
                                    .trim()
                            )
                            .unwrap_or_else(|_| "badly formatted locale string".into())
                        };
                        eta_label.set_label(&eta_text);
                    }
                }
                TransferState::SendingFiles => {}
                TransferState::Disconnected => {
                    if event_msg.id == init_id {
                        progress_dialog.set_can_close(true);
                        if let Some(UserAction::ConsentAccept) = receive_state.user_action() {
                            progress_dialog.close();
                        } else {
                            consent_dialog.close();
                        }

                        let body = gettext("Unexpected dissconnection");

                        spawn_notification(
                            notification_id.clone(),
                            Notification::new(&event_msg.device_name())
                                .body(body.as_str())
                                .priority(Priority::High)
                                .default_action(None)
                        );

                        win.imp().toast_overlay.add_toast(
                            adw::Toast::builder()
                                .title(&body)
                                .priority(adw::ToastPriority::High)
                                .build(),
                        );

                        // FIXME: If ReceivingFiles is not received within 5~10 seconds of an Accept,
                        // reject request and show this error, it's usually because the sender
                        // disconnected from the network
                    }
                }
                TransferState::Rejected => {}
                TransferState::Cancelled => {
                    progress_dialog.set_can_close(true);
                    if let Some(UserAction::ConsentAccept) = receive_state.user_action() {
                        progress_dialog.close();
                    } else {
                        consent_dialog.close();
                    }

                    // Since Cancelled also triggers on cancellation from the user
                    if !is_user_cancelled.get() {
                        let body = gettext("Transfer cancelled by sender");

                        spawn_notification(
                            notification_id.clone(),
                            Notification::new(&event_msg.device_name())
                                .body(body.as_str())
                                .priority(Priority::High)
                                .default_action(None)
                        );

                        win.imp().toast_overlay.add_toast(
                            adw::Toast::builder()
                                .title(&body)
                                .priority(adw::ToastPriority::High)
                                .build(),
                        );
                    }
                }
                TransferState::Finished => {
                    progress_dialog.set_can_close(true);
                    if let Some(UserAction::ConsentAccept) = receive_state.user_action() {
                        progress_dialog.close();
                    } else {
                        consent_dialog.close();
                    }

                    if let Some(text_data) = event_msg.transferred_text_data() {
                        let text_type = text_data.1;

                        let dialog = adw::Dialog::builder()
                            .content_width(400)
                            .content_height(200)
                            .title(display_text_type(&text_type))
                            .build();

                        let toolbar_view = adw::ToolbarView::builder()
                            .top_bar_style(adw::ToolbarStyle::Flat)
                            .build();
                        dialog.set_child(Some(&toolbar_view));

                        let header_bar = adw::HeaderBar::builder().build();
                        toolbar_view.add_top_bar(&header_bar);

                        let copy_text_button = gtk::Button::builder()
                            .valign(gtk::Align::Center)
                            .hexpand(true)
                            .icon_name("edit-copy-symbolic")
                            .tooltip_text(&gettext("Copy to clipboard"))
                            .css_classes(["circular", "flat"])
                            .build();
                        let save_text_button = gtk::Button::builder()
                            .visible(false)
                            .valign(gtk::Align::Center)
                            .hexpand(true)
                            .icon_name("document-save-symbolic")
                            .tooltip_text(&gettext("Save text as file"))
                            .css_classes(["circular", "flat"])
                            .build();
                        header_bar.pack_start(&copy_text_button);
                        header_bar.pack_start(&save_text_button);

                        let clamp = adw::Clamp::builder()
                            .maximum_size(550)
                            .hexpand(true)
                            .vexpand(true)
                            .build();
                        toolbar_view.set_content(Some(&clamp));

                        let root_box = gtk::Box::builder()
                            .orientation(gtk::Orientation::Vertical)
                            .hexpand(true)
                            .margin_top(6)
                            .margin_bottom(18)
                            .margin_start(18)
                            .margin_end(18)
                            .spacing(18)
                            .build();
                        clamp.set_child(Some(&root_box));

                        let caption_label = gtk::Label::builder()
                            .use_markup(true)
                            .wrap(true)
                            .visible(false)
                            .build();
                        root_box.append(&caption_label);

                        let text_view = gtk::TextView::builder()
                            .top_margin(12)
                            .bottom_margin(12)
                            .left_margin(12)
                            .right_margin(12)
                            .editable(false)
                            .cursor_visible(false)
                            .monospace(true)
                            .wrap_mode(gtk::WrapMode::Word)
                            .build();

                        let text_view_frame = gtk::Frame::builder()
                            .vexpand(true)
                            .child(
                                &gtk::ScrolledWindow::builder()
                                    .vexpand(true)
                                    .child(&text_view)
                                    .build(),
                            )
                            .build();
                        root_box.append(&text_view_frame);

                        let open_uri_button = gtk::Button::builder()
                            .halign(gtk::Align::Center)
                            .valign(gtk::Align::Center)
                            .height_request(50)
                            .label(&gettext("Open"))
                            .css_classes(["pill", "suggested-action"])
                            .build();
                        root_box.append(&open_uri_button);
                        if text_type.clone() as u32 == TextPayloadType::Url as u32 {
                            open_uri_button.set_visible(true);
                        } else {
                            open_uri_button.set_visible(false);
                        }

                        save_text_button.connect_clicked(clone!(
                            #[weak]
                            win,
                            #[weak]
                            text_view,
                            move |_| {
                                let text = text_view.buffer().text(
                                    &text_view.buffer().start_iter(),
                                    &text_view.buffer().end_iter(),
                                    false,
                                );

                                glib::spawn_future_local(async move {
                                    let file = gtk::FileDialog::new()
                                        .save_text_file_future(Some(&win))
                                        .await
                                        .unwrap()
                                        .0;

                                    let text_bytes = text.into_bytes();
                                    file.create_readwrite_future(
                                        gio::FileCreateFlags::REPLACE_DESTINATION,
                                        Default::default(),
                                    )
                                    .await
                                    .unwrap()
                                    .output_stream()
                                    .write_all_future(text_bytes, Default::default())
                                    .await
                                    .unwrap();
                                });
                            }
                        ));

                        let clipboard = win.clipboard();
                        copy_text_button.connect_clicked(clone!(
                            #[weak]
                            text_view,
                            #[strong]
                            clipboard,
                            move |_| {
                                let text = text_view.buffer().text(
                                    &text_view.buffer().start_iter(),
                                    &text_view.buffer().end_iter(),
                                    false,
                                );
                                clipboard.set_text(&text);
                            }
                        ));

                        open_uri_button.connect_clicked(clone!(
                            #[weak]
                            win,
                            #[weak]
                            text_view,
                            move |_| {
                                let url = text_view.buffer().text(
                                    &text_view.buffer().start_iter(),
                                    &text_view.buffer().end_iter(),
                                    false,
                                );

                                gtk::UriLauncher::new(&url).launch(
                                    win.root().and_downcast_ref::<adw::ApplicationWindow>(),
                                    None::<gio::Cancellable>.as_ref(),
                                    |_err| {},
                                );
                            }
                        ));

                        let raw_text = text_data.0;
                        let text = if text_type.clone() as u32 == TextPayloadType::Text as u32 {
                            save_text_button.set_visible(true);
                            clean_text_payload(&raw_text)
                        } else {
                            &raw_text
                        };
                        text_view.set_buffer(Some(&gtk::TextBuffer::builder().text(text).build()));

                        spawn_notification(
                            notification_id.clone(),
                            Notification::new(&event_msg.device_name())
                                .body(
                                    formatx!(
                                        gettext("Received \"{}\""),
                                        if text.len() > 48 {
                                            format!("{}{}", &text[..48], "...")
                                        } else {
                                            text.into()
                                        }
                                    )
                                    .unwrap_or_default()
                                    .as_str()
                                )
                                .priority(Priority::High)
                                .display_hint([DisplayHint::ShowAsNew])
                                .default_action("copy-text")
                                .default_action_target(text)
                                .button(
                                    ashpd::desktop::notification::Button::new(&gettext("Copy"), "copy-text")
                                        .target(text)
                                )
                        );

                        // FIXME: Redo the Wi-Fi view when we've more info such as the Wi-Fi security type
                        // and payload (password) available separately

                        dialog.present(Some(&win));
                    } else {
                        // Received Files
                        let file_count = event_msg.files().unwrap().len();

                        let body = formatx!(
                            ngettext(
                                "{} file received",
                                "{} files received",
                                file_count as u32
                            ),
                            file_count
                        )
                            .unwrap_or_else(|_| "badly formatted locale string".into());

                        let target = win.imp().settings.string("download-folder");
                        spawn_notification(
                            notification_id.clone(),
                            Notification::new(&event_msg.device_name())
                                .body(body.as_str())
                                .priority(Priority::High)
                                .display_hint([DisplayHint::ShowAsNew])
                                .default_action("open-folder")
                                .default_action_target(target.as_str())
                                .button(
                                    ashpd::desktop::notification::Button::new(&gettext("Open"), "open-folder")
                                        .target(target.as_str())
                                )
                        );
                        let toast = adw::Toast::builder()
                            .title(&body)
                            .button_label(&gettext("Open"))
                            .action_name("win.received-files")
                            .priority(adw::ToastPriority::High)
                            .build();
                        win.imp().toast_overlay.add_toast(toast);
                    }
                }
            }
        }
    ));
    receive_state.notify_event();
}
