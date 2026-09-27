use std::{
    cell::RefCell,
    collections::VecDeque,
    fmt,
    io::Read,
    panic::Location,
    path::{Path, PathBuf},
    rc::Rc,
    time,
};

use adw::prelude::*;
use ashpd::desktop::notification::Notification;
use gettextrs::ngettext;
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
        _ = async move || -> anyhow::Result<()> {
            use ashpd::desktop::notification::*;
            let proxy = NotificationProxy::new().await?;

            proxy.add_notification(&id, notification).await?;

            Ok(())
        }()
        .await;
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
