use std::{
    collections::VecDeque,
    fmt,
    path::{Path, PathBuf},
    time::{self},
};

use gettextrs::ngettext;

#[macro_export]
macro_rules! impl_deref_for_newtype {
    ($type:ty, $target:ty) => {
        impl std::ops::Deref for $type {
            type Target = $target;

            fn deref(&self) -> &Self::Target {
                &self.0
            }
        }

        impl std::ops::DerefMut for $type {
            fn deref_mut(&mut self) -> &mut Self::Target {
                &mut self.0
            }
        }
    };
}

pub fn strip_user_home_prefix<P: AsRef<Path>>(path: P) -> PathBuf {
    if let Some(home) = dirs::home_dir() {
        if path.as_ref().starts_with(&home) {
            return PathBuf::from("~").join(path.as_ref().strip_prefix(&home).unwrap());
        }
    }

    path.as_ref().into()
}

/// Flatpak uses get_user_special_dir to get xdg directories, and so if it fails
/// due to there being no `XDG_DOWNLOAD_DIR` and `user-dirs.dirs`, Flatpak will simply
/// refuse to mount xdg-download in the sandbox. Leaving us with nothing.
///
/// In that case, simply ask for the download folder from the user.
pub fn xdg_download_with_fallback() -> PathBuf {
    /// `$XDG_DATA_HOME/Downloads`
    fn download_dir_fallback() -> PathBuf {
        let fallback = dirs::data_dir().unwrap_or_default().join("Downloads");
        if !std::fs::exists(&fallback).unwrap_or_default() {
            _ = fs_err::create_dir_all(&fallback).inspect_err(|err| tracing::warn!(%err));
        }

        fallback
    }

    match dirs::home_dir() {
        Some(home_dir) => {
            let fallback = download_dir_fallback();
            match dirs::download_dir() {
                Some(download_dir) => {
                    if std::fs::exists(&download_dir).unwrap_or_default() {
                        download_dir
                    } else {
                        tracing::warn!(
                            ?home_dir,
                            ?download_dir,
                            ?fallback,
                            "Found XDG_DOWNLOAD_DIR but it doesn't exist"
                        );
                        fallback
                    }
                }
                None => {
                    tracing::warn!(?home_dir, ?fallback, "Couldn't find XDG_DOWNLOAD_DIR");
                    fallback
                }
            }
        }
        None => {
            let fallback = download_dir_fallback();
            tracing::warn!(
                ?fallback,
                "Couldn't get user's HOME while trying to get XDG_DOWNLOAD_DIR"
            );
            fallback
        }
    }
}

const STEPS_TRACK_COUNT: usize = 5;

/// Proudly stolen from:\
/// https://github.com/Manishearth/rustup.rs/blob/1.0.0/src/rustup-cli/download_tracker.rs
#[derive(Debug, Clone, better_default::Default)]
pub struct DataTransferEta {
    // Making it pub so we can check if Estimator is in initial state
    // Need to do this because the RefCell<Option<DataTransferEtaBoxed>> wouldn't
    // satisfy glib::Property
    pub total_len: usize,
    total_transferred: usize,

    transferred_this_sec: usize,

    #[default(VecDeque::with_capacity(STEPS_TRACK_COUNT))]
    transferred_last_few_secs: VecDeque<usize>,

    last_sec: Option<time::Instant>,
    seconds_elapsed: usize,
}

impl DataTransferEta {
    pub fn new(len: usize) -> Self {
        Self {
            total_len: len,
            ..Default::default()
        }
    }

    pub fn step_with(&mut self, total_transferred: usize) {
        let len = total_transferred - self.total_transferred;
        self.transferred_this_sec += len;
        self.total_transferred = total_transferred;

        let current_time = time::Instant::now();

        match self.last_sec {
            None => {
                self.last_sec = Some(current_time);
            }
            Some(start) => {
                let elapsed = current_time - start;

                if elapsed.as_secs_f64() >= 1.0 {
                    self.seconds_elapsed += 1;

                    self.last_sec = Some(current_time);
                    if self.transferred_last_few_secs.len() == STEPS_TRACK_COUNT {
                        self.transferred_last_few_secs.pop_back();
                    }
                    self.transferred_last_few_secs
                        .push_front(self.transferred_this_sec);
                    self.transferred_this_sec = 0;
                }
            }
        };
    }

    pub fn prepare_for_new_transfer(&mut self, total_len: Option<usize>) {
        if let Some(total_len) = total_len {
            self.total_len = total_len;
        }
        self.total_transferred = 0;
        self.transferred_this_sec = 0;
        self.transferred_last_few_secs.clear();
        self.seconds_elapsed = 0;
        self.last_sec = None;
    }

    pub fn get_estimate_string(&self) -> String {
        let sum = self
            .transferred_last_few_secs
            .iter()
            .fold(0., |a, &v| a + v as f64);
        let len = self.transferred_last_few_secs.len();
        let speed = if len > 0 { sum / len as f64 } else { 0. };

        let total_len = self.total_len as f64;
        let remaining = total_len - self.total_transferred as f64;
        let eta_h = HumanReadable(remaining / speed);

        eta_h.to_string()
    }
}

#[derive(Debug, Clone, Copy)]
struct HumanReadable(f64);

impl fmt::Display for HumanReadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sec = self.0;

        if sec.is_infinite() {
            write!(f, "Unknown")
        } else {
            // we're doing modular arithmetic, treat as integer
            let sec = self.0 as u32;
            if sec > 6_000 {
                let h = sec / 3600;
                let min = sec % 3600;

                write!(
                    f,
                    "{:3} {} {:2} {}",
                    h,
                    ngettext("hour", "hours", h),
                    min,
                    ngettext("minute", "minutes", min)
                )
            } else if sec > 100 {
                let min = sec / 60;
                let sec = sec % 60;

                write!(
                    f,
                    "{:3} {} {:2} {}",
                    min,
                    ngettext("minute", "minutes", min),
                    sec,
                    ngettext("second", "seconds", sec)
                )
            } else {
                write!(f, "{:3.0} {}", sec, ngettext("second", "seconds", sec))
            }
        }
    }
}
