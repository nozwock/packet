use adw::prelude::*;

use crate::window::Window;

// https://gitlab.gnome.org/World/fractal/-/blob/main/src/utils/toast.rs
pub(crate) fn add_toast(widget: &gtk::Widget, toast: adw::Toast) {
    if let Some(dialog) = widget
        .ancestor(adw::PreferencesDialog::static_type())
        .and_downcast::<adw::PreferencesDialog>()
    {
        dialog.add_toast(toast);
    } else if let Some(root) = widget.root() {
        if let Some(window) = root.downcast_ref::<Window>() {
            window.add_toast(toast);
        } else {
            panic!("Trying to display a toast when the parent doesn't support it");
        }
    }
}
