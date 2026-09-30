use std::{
    cell::RefCell,
    fmt,
    io::Read,
    panic::Location,
    path::{Path, PathBuf},
    rc::Rc,
    time::Instant,
};

use adw::prelude::*;
use ashpd::desktop::notification::Notification;
use gettextrs::{gettext, ngettext};
use gtk::{
    gio,
    glib::{self, clone},
};

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

pub fn is_url(text: &str) -> bool {
    let trimmed = text.trim();

    let mut finder = linkify::LinkFinder::new();
    finder.kinds(&[linkify::LinkKind::Url]);
    finder.spans(trimmed).next().is_some_and(|it| {
        it.kind() == Some(&linkify::LinkKind::Url) && it.start() == 0 && it.end() == trimmed.len()
    })
}

pub fn xdg_data_dirs() -> Vec<PathBuf> {
    std::env::var_os("XDG_DATA_DIRS")
        .and_then(|it| {
            let paths = std::env::split_paths(&it)
                .map(PathBuf::from)
                .filter(|it| it.is_absolute())
                .collect::<Vec<_>>();
            (!paths.is_empty()).then_some(paths)
        })
        .unwrap_or_else(|| {
            vec![
                PathBuf::from("/usr/local/share"),
                PathBuf::from("/usr/share"),
            ]
        })
}

/// Based on strict byte-by-byte comparison.
// https://users.rust-lang.org/t/efficient-way-of-checking-if-two-files-have-the-same-content/74735/11
pub fn is_file_same(file1: impl AsRef<Path>, file2: impl AsRef<Path>) -> anyhow::Result<bool> {
    use std::io::BufReader;
    let mut reader1 = BufReader::new(fs_err::File::open(file1.as_ref())?);
    let mut reader2 = BufReader::new(fs_err::File::open(file2.as_ref())?);

    let mut buf1 = [0u8; 4096];
    let mut buf2 = [0u8; 4096];

    loop {
        let bytes_read1 = reader1.read(&mut buf1)?;
        let bytes_read2 = reader2.read(&mut buf2)?;

        if bytes_read1 != bytes_read2 || buf1 != buf2 {
            return Ok(false);
        }

        assert_eq!(bytes_read1, bytes_read2); // Sanity check
        if bytes_read1 == 0 {
            // EOF
            break;
        }
    }

    Ok(true)
}

pub struct SignalBlockGuard<'a, O: glib::object::ObjectExt> {
    widget: O,
    id: Option<std::cell::Ref<'a, glib::SignalHandlerId>>,
}

impl<'a, O: glib::object::ObjectExt + Clone> SignalBlockGuard<'a, O> {
    #[track_caller]
    pub fn new(
        widget: &O,
        handler_id: &'a std::cell::RefCell<Option<glib::SignalHandlerId>>,
    ) -> Self {
        let id = std::cell::Ref::filter_map(handler_id.borrow(), |opt| opt.as_ref()).ok();
        if let Some(id) = id.as_ref() {
            widget.block_signal(id);
        } else {
            let caller = Location::caller();
            debug_assert!(
                false,
                "SignalHandlerId is not set before blocking signal at {caller}"
            );
            tracing::warn!("SignalHandlerId is not set before blocking signal at {caller}");
        }

        Self {
            widget: widget.clone(),
            id,
        }
    }
}

impl<O: glib::object::ObjectExt> Drop for SignalBlockGuard<'_, O> {
    fn drop(&mut self) {
        if let Some(id) = self.id.as_ref() {
            self.widget.unblock_signal(id);
        }
    }
}

pub fn spawn_notification(id: String, notification: Notification) {
    glib::spawn_future_local(async move {
        if let Err(err) = async move || -> anyhow::Result<()> {
            use ashpd::desktop::notification::*;
            let proxy = NotificationProxy::new().await?;

            // `display-hint` was added in version 2 of the portal interface,
            // older portals (e.g. xdg-desktop-portal 1.18) reject the whole
            // notification if it's present.
            let notification = if proxy.version() < 2 {
                notification.display_hint([])
            } else {
                notification
            };

            proxy.add_notification(&id, notification).await?;

            Ok(())
        }()
        .await
        {
            tracing::warn!(%err, "Failed to show notification");
        }
    });
}

pub fn remove_notification(id: String) {
    glib::spawn_future_local(async move {
        _ = async move || -> anyhow::Result<()> {
            use ashpd::desktop::notification::*;
            let proxy = NotificationProxy::new().await?;

            proxy.remove_notification(&id).await?;

            Ok(())
        }()
        .await;
    });
}

pub fn strip_user_home_prefix<P: AsRef<Path>>(path: P) -> PathBuf {
    if let Some(home) = dirs::home_dir()
        && let Ok(stripped) = path.as_ref().strip_prefix(&home)
    {
        return PathBuf::from("~").join(stripped);
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

fn estimator_weight(age: f64) -> f64 {
    const EXPONENTIAL_WEIGHTING_SECONDS: f64 = 15.0;
    0.1_f64.powf(age / EXPONENTIAL_WEIGHTING_SECONDS)
}

/// Taken from indicatif:
/// https://github.com/console-rs/indicatif/blob/ccb0d811751d343e428bb2ca6729dd5aa50906ea/src/state.rs#L422-L532
#[derive(Debug, Clone)]
struct Estimator {
    smoothed_steps_per_sec: f64,
    double_smoothed_steps_per_sec: f64,
    prev_steps: u64,
    prev_time: Instant,
    start_time: Instant,
}

impl Default for Estimator {
    fn default() -> Self {
        Self::new(Instant::now())
    }
}

impl Estimator {
    pub fn new(now: Instant) -> Self {
        Self {
            smoothed_steps_per_sec: 0.0,
            double_smoothed_steps_per_sec: 0.0,
            prev_steps: 0,
            prev_time: now,
            start_time: now,
        }
    }

    pub fn record(&mut self, new_steps: u64, now: Instant) {
        // sanity check: don't record data if time or steps have not advanced
        if new_steps <= self.prev_steps || now <= self.prev_time {
            // Reset on backwards seek to prevent breakage from seeking to the end for length determination
            if new_steps < self.prev_steps {
                self.prev_steps = new_steps;
                self.reset(now);
            }
            return;
        }

        let delta_steps = new_steps - self.prev_steps;
        let delta_t = (now - self.prev_time).as_secs_f64();

        // the rate of steps we saw in this update
        let new_steps_per_second = delta_steps as f64 / delta_t;

        // update the estimate: a weighted average of the old estimate and new data
        let weight = estimator_weight(delta_t);
        self.smoothed_steps_per_sec =
            self.smoothed_steps_per_sec * weight + new_steps_per_second * (1.0 - weight);

        let delta_t_start = (now - self.start_time).as_secs_f64();
        let total_weight = 1.0 - estimator_weight(delta_t_start);
        let normalized_smoothed_steps_per_sec = if total_weight > 0.0 {
            self.smoothed_steps_per_sec / total_weight
        } else {
            new_steps_per_second
        };

        // determine the double smoothed value (EWA smoothing of the single EWA)
        self.double_smoothed_steps_per_sec = self.double_smoothed_steps_per_sec * weight
            + normalized_smoothed_steps_per_sec * (1.0 - weight);

        self.prev_steps = new_steps;
        self.prev_time = now;
    }

    /// Reset the state of the estimator. Once reset, estimates will not depend on any data prior
    /// to `now`.
    pub fn reset(&mut self, now: Instant) {
        self.smoothed_steps_per_sec = 0.0;
        self.double_smoothed_steps_per_sec = 0.0;

        // only reset prev_time, not prev_steps
        self.prev_time = now;
        self.start_time = now;
    }

    /// Average time per step in seconds, using double exponential smoothing
    pub fn steps_per_second(&self, now: Instant) -> f64 {
        let delta_t = (now.saturating_duration_since(self.prev_time)).as_secs_f64();
        let reweight = estimator_weight(delta_t);

        let delta_t_start = (now.saturating_duration_since(self.start_time)).as_secs_f64();
        let total_weight = 1.0 - estimator_weight(delta_t_start);
        if total_weight <= 0.0 {
            return 0.0;
        }

        let sps = self.smoothed_steps_per_sec * reweight / total_weight;
        let dsps = self.double_smoothed_steps_per_sec * reweight + sps * (1.0 - reweight);
        let rate = dsps / total_weight;

        if rate.is_nan() || rate.is_infinite() || rate <= 0.0 {
            0.0
        } else {
            rate
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct DataTransferEta {
    // Making it pub so we can check if Estimator is in initial state
    // Need to do this because the RefCell<Option<DataTransferEtaBoxed>> wouldn't
    // satisfy glib::Property
    pub total_len: usize,
    est: Estimator,
}

impl DataTransferEta {
    pub fn new(len: usize) -> Self {
        Self {
            total_len: len,
            est: Estimator::new(Instant::now()),
        }
    }

    pub fn step_with(&mut self, total_transferred: u64) {
        self.est.record(total_transferred as u64, Instant::now());
    }

    pub fn reset_with(&mut self, total_len: Option<usize>) {
        if let Some(total_len) = total_len {
            self.total_len = total_len;
        }
        let now = Instant::now();
        self.est.prev_steps = 0;
        self.est.reset(now);
    }

    pub fn eta_fmt(&self) -> String {
        if self.total_len == 0 {
            return HumanReadable(f64::NAN).to_string();
        }

        if self.est.prev_steps >= self.total_len as u64 {
            return HumanReadable(0.0).to_string();
        }

        let speed = self.est.steps_per_second(Instant::now());
        if speed <= 0.0 {
            return HumanReadable(f64::NAN).to_string();
        }

        let remaining = (self.total_len as u64 - self.est.prev_steps) as f64;
        let eta_secs = remaining / speed;
        HumanReadable(eta_secs).to_string()
    }
}

#[derive(Debug, Clone, Copy)]
struct HumanReadable(f64);

impl fmt::Display for HumanReadable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sec = self.0;

        if sec.is_nan() || sec.is_infinite() || sec < 0.0 || sec > 3600. * 24. * 365. {
            return write!(f, "{}", gettext("Unknown"));
        }

        let total_secs = sec.round() as u64;
        let h = total_secs / 3600;
        let min = (total_secs % 3600) / 60;
        let s = total_secs % 60;

        if h > 0 {
            if min > 0 {
                write!(
                    f,
                    "{} {} {} {}",
                    h,
                    ngettext("hour", "hours", h as u32),
                    min,
                    ngettext("minute", "minutes", min as u32),
                )
            } else {
                write!(f, "{} {}", h, ngettext("hour", "hours", h as u32))
            }
        } else if min > 0 {
            if s > 0 {
                write!(
                    f,
                    "{} {} {} {}",
                    min,
                    ngettext("minute", "minutes", min as u32),
                    s,
                    ngettext("second", "seconds", s as u32),
                )
            } else {
                write!(f, "{} {}", min, ngettext("minute", "minutes", min as u32))
            }
        } else {
            write!(f, "{} {}", s, ngettext("second", "seconds", s as u32))
        }
    }
}

fn update_link_tags(buffer: &gtk::TextBuffer, link_tag: &gtk::TextTag) {
    let (start, end) = (buffer.start_iter(), buffer.end_iter());
    // Clear all link tags
    buffer.remove_tag(link_tag, &start, &end);

    let text = buffer.text(&start, &end, false);

    let mut finder = linkify::LinkFinder::new();
    finder.kinds(&[linkify::LinkKind::Url]);

    let mut char_offset = 0;
    let mut byte_offset = 0;

    for span in finder.spans(&text) {
        if span.kind() != Some(&linkify::LinkKind::Url) {
            continue;
        }

        char_offset += text[byte_offset..span.start()].chars().count();
        let link_chars = text[span.start()..span.end()].chars().count();

        if let Ok(start_offset) = i32::try_from(char_offset)
            && let Ok(end_offset) = i32::try_from(char_offset + link_chars)
        {
            buffer.apply_tag(
                link_tag,
                &buffer.iter_at_offset(start_offset),
                &buffer.iter_at_offset(end_offset),
            );

            char_offset += link_chars;
            byte_offset = span.end();
        }
    }
}

fn connect_buffer_for_link_tags(
    buffer: &gtk::TextBuffer,
    active_buffer: &Rc<RefCell<Option<gtk::TextBuffer>>>,
) {
    if let Some(old_buf) = active_buffer.borrow_mut().take()
        && let Some(tag) = old_buf.tag_table().lookup("url_link")
    {
        let (start, end) = (old_buf.start_iter(), old_buf.end_iter());
        old_buf.remove_tag(&tag, &start, &end);
    }

    let link_tag = if let Some(tag) = buffer.tag_table().lookup("url_link") {
        tag
    } else {
        let style_manager = adw::StyleManager::default();
        let accent_color = style_manager.accent_color().to_rgba();

        let link_tag = gtk::TextTag::builder()
            .name("url_link")
            .underline(gtk::pango::Underline::Single)
            .foreground_rgba(&accent_color)
            .underline_rgba(&accent_color)
            .build();

        _ = buffer.tag_table().add(&link_tag);
        link_tag
    };

    update_link_tags(buffer, &link_tag);

    *active_buffer.borrow_mut() = Some(buffer.clone());
}

pub type ClickableLinksCleanup = Box<dyn FnOnce()>;

pub fn setup_clickable_links(text_view: &gtk::TextView) -> ClickableLinksCleanup {
    if text_view.is_editable() {
        debug_assert!(
            false,
            "setup_clickable_links is only supported on non-editable TextViews"
        );
        tracing::warn!("setup_clickable_links is only supported on non-editable TextViews");
        return Box::new(|| {});
    }

    let active_buffer: Rc<RefCell<Option<gtk::TextBuffer>>> = Rc::new(RefCell::new(None));

    connect_buffer_for_link_tags(&text_view.buffer(), &active_buffer);

    let buffer_notify_id = text_view.connect_buffer_notify(clone!(
        #[strong]
        active_buffer,
        move |text_view| {
            connect_buffer_for_link_tags(&text_view.buffer(), &active_buffer);
        }
    ));

    let click = gtk::GestureClick::new();
    click.connect_released(clone!(
        #[weak]
        text_view,
        move |gesture, n_press, x, y| {
            if n_press != 1 || text_view.buffer().has_selection() {
                return;
            }

            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);

            if let Some(link_tag) = text_view.buffer().tag_table().lookup("url_link")
                && let Some(iter) = text_view.iter_at_location(bx, by)
                && iter.has_tag(&link_tag)
            {
                let mut start = iter.clone();
                if !start.starts_tag(Some(&link_tag)) {
                    start.backward_to_tag_toggle(Some(&link_tag));
                }
                let mut end = iter.clone();
                if !end.ends_tag(Some(&link_tag)) {
                    end.forward_to_tag_toggle(Some(&link_tag));
                }

                let url = text_view.buffer().text(&start, &end, false);
                let root = text_view.root();
                let window = root.as_ref().and_then(|r| r.downcast_ref::<gtk::Window>());

                let _ = gtk::UriLauncher::new(&url).launch(
                    window,
                    None::<gio::Cancellable>.as_ref(),
                    |_| {},
                );

                gesture.set_state(gtk::EventSequenceState::Claimed);
            }
        }
    ));
    text_view.add_controller(click.clone());

    // Change mouse cursor on hover
    let motion = gtk::EventControllerMotion::new();
    motion.connect_motion(clone!(
        #[weak]
        text_view,
        move |_, x, y| {
            let (bx, by) =
                text_view.window_to_buffer_coords(gtk::TextWindowType::Widget, x as i32, y as i32);

            let is_over_link = text_view
                .buffer()
                .tag_table()
                .lookup("url_link")
                .is_some_and(|link_tag| {
                    text_view
                        .iter_at_location(bx, by)
                        .is_some_and(|iter| iter.has_tag(&link_tag))
                });

            if is_over_link {
                text_view.set_cursor_from_name(Some("pointer"));
            } else {
                text_view.set_cursor_from_name(None);
            }
        }
    ));
    motion.connect_leave(clone!(
        #[weak]
        text_view,
        move |_| {
            text_view.set_cursor_from_name(None);
        }
    ));
    text_view.add_controller(motion.clone());

    let text_view = text_view.downgrade();
    Box::new(clone!(
        #[strong]
        click,
        #[strong]
        motion,
        move || {
            if let Some(text_view) = text_view.upgrade() {
                text_view.remove_controller(&click);
                text_view.remove_controller(&motion);
                text_view.disconnect(buffer_notify_id);
                text_view.set_cursor_from_name(None);
            }

            if let Some(buf) = active_buffer.borrow_mut().take()
                && let Some(tag) = buf.tag_table().lookup("url_link")
            {
                let (start, end) = (buf.start_iter(), buf.end_iter());
                buf.remove_tag(&tag, &start, &end);
            }
        }
    ))
}
