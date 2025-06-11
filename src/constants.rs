use std::{path::PathBuf, sync::OnceLock};

// https://github.com/flatpak/xdg-desktop-portal/pull/1372/files
#[allow(dead_code)]
pub const XDP_XATTR_HOST_PATH: &str = "xattr::document-portal.host-path";

pub fn packet_log_path() -> &'static PathBuf {
    static PACKET_LOG_PATH: OnceLock<PathBuf> = OnceLock::new();
    PACKET_LOG_PATH.get_or_init(|| dirs::cache_dir().unwrap_or_default().join("packet.log"))
}
