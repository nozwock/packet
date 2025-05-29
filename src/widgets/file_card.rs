use adw::prelude::*;
use adw::subclass::prelude::*;
use formatx::formatx;
use gettextrs::{gettext, ngettext};
use gtk::{
    gio::{self, FileQueryInfoFlags},
    glib::{self, clone},
};

use crate::window::PacketApplicationWindow;

// These are the icons that Files/nautilus uses
// https://gitlab.gnome.org/GNOME/adwaita-icon-theme/-/tree/master/Adwaita/scalable?ref_type=heads
pub const ADWAITA_MIMETYPE_ICON_NAMES: [&str; 27] = [
    "application-certificate",
    "application-x-addon",
    "application-x-executable",
    "application-x-firmware",
    "application-x-generic",
    "application-x-sharedlib",
    "audio-x-generic",
    "font-x-generic",
    "image-x-generic",
    "inode-directory",
    "inode-symlink",
    "model",
    "package-x-generic",
    "text-html",
    "text-x-generic",
    "text-x-preview",
    "text-x-script",
    "video-x-generic",
    "x-office-addressbook",
    "x-office-document-template",
    "x-office-document",
    "x-office-drawing",
    "x-office-presentation-template",
    "x-office-presentation",
    "x-office-spreadsheet-template",
    "x-office-spreadsheet",
    "x-package-repository",
];

pub fn get_mimetype_icon_name(file: &gio::File, symbolic: bool) -> Option<String> {
    let themed_icon = file
        .query_info(
            "standard::icon",
            FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()?
        .icon()
        .and_downcast::<gio::ThemedIcon>()?;

    let icon = themed_icon
        .names()
        .into_iter()
        .map(|it| it.to_string())
        .find_map(|it| {
            for f in ADWAITA_MIMETYPE_ICON_NAMES {
                if format!("{f}-symbolic") == it {
                    return if symbolic {
                        Some(it)
                    } else {
                        Some(it.replace("-symbolic", ""))
                    };
                }
            }
            None
        });

    Some(icon?)
}

pub fn create_file_card(
    win: &PacketApplicationWindow,
    model: &gio::ListStore,
    model_item: &gio::File,
) -> adw::Bin {
    let imp = win.imp();

    let root_bin = adw::Bin::new();
    let _box = gtk::Box::builder().build();
    let root_box = gtk::Box::builder()
        .margin_start(12)
        .margin_end(12)
        .margin_top(12)
        .margin_bottom(12)
        .spacing(12)
        .build();
    root_bin.set_child(Some(&_box));
    _box.append(&root_box);
    let file_avatar = gtk::Image::builder()
        .icon_name(
            &get_mimetype_icon_name(&model_item, false).unwrap_or("application-x-generic".into()),
        )
        .pixel_size(48)
        .css_classes(["icon-dropshadow"])
        .build();
    root_box.append(&file_avatar);

    let filename_label = gtk::Label::builder()
        .label(model_item.basename().unwrap().to_string_lossy())
        .xalign(0.)
        .hexpand(true)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::Char)
        .build();
    root_box.append(&filename_label);

    let remove_file_button = gtk::Button::builder()
        .valign(gtk::Align::Center)
        .halign(gtk::Align::Center)
        .icon_name("cross-large-symbolic")
        .css_classes(["flat", "circular"])
        .tooltip_text(&gettext("Remove"))
        .build();
    root_box.append(&remove_file_button);

    remove_file_button.connect_clicked(clone!(
        #[weak]
        imp,
        #[weak]
        model,
        #[weak]
        model_item,
        move |_| {
            if let Some(pos) = model.find(&model_item) {
                model.remove(pos);
            }

            imp.manage_files_header.set_title(
                &formatx!(
                    ngettext("{} File", "{} Files", model.n_items()),
                    model.n_items() as usize
                )
                .unwrap_or_else(|_| "badly formatted locale string".into()),
            );

            if model.n_items() == 0 {
                imp.main_nav_view.pop();
            }
        }
    ));

    root_bin
}
