use std::path::PathBuf;

use anyhow::Context;

use crate::{
    config::{DATADIR, PKGDATADIR},
    utils::{is_file_same, xdg_data_dirs},
};

pub trait Plugin {
    /// Installs and updates the plugin.
    ///
    /// Run it under a separate thread if you don't want it to block.
    fn install_plugin(&self) -> anyhow::Result<()>;
    fn uninstall_plugin(&self) -> anyhow::Result<()>;
}

pub trait FileBasedPlugin: Plugin {
    fn plugin_files(&self) -> &[PathBuf];
    fn install_dir(&self) -> Option<PathBuf>;
    /// It's the path to show to the user for troubleshooting purposes.
    fn help_install_dir() -> &'static str;
}
impl<T: FileBasedPlugin> Plugin for T {
    fn install_plugin(&self) -> anyhow::Result<()> {
        let missing_plugin_files = self
            .plugin_files()
            .into_iter()
            .filter(|it| !it.exists())
            .collect::<Vec<_>>();

        if missing_plugin_files.len() > 0 {
            anyhow::bail!("Missing plugin files: {:?}", missing_plugin_files);
        }

        let install_dir = self.install_dir().with_context(|| {
            anyhow::anyhow!(
                "Couldn't find the directory to move the plugin files into: {:?}",
                self.plugin_files()
            )
        })?;

        tracing::debug!(
            plugin = std::any::type_name::<T>(),
            ?install_dir,
            plugin_files = ?self.plugin_files(),
            "Installing plugin"
        );

        for (src_path, dest_path) in self
            .plugin_files()
            .into_iter()
            .filter_map(|file_path| {
                file_path
                    .file_name() // filtering out None
                    .map(|name| install_dir.join(name))
                    .map(|dest_path| (file_path, dest_path))
            })
            .filter(|(src_path, dest_path)| {
                let should_copy_file = !(dest_path
                    .exists()
                    .then(|| is_file_same(src_path, &dest_path).ok())
                    .flatten()
                    .unwrap_or_default());
                should_copy_file
            })
        // TODO: Probably something like rayon would make sense here in case
        // it's expected to process many plugin files
        {
            // Symlinking won't work if the app is running under Flatpak, so
            // just copy instead
            tracing::debug!(from = ?src_path, to = ?dest_path,"Copying plugin file");
            fs_err::copy(&src_path, &dest_path)?;
        }

        Ok(())
    }

    fn uninstall_plugin(&self) -> anyhow::Result<()> {
        let install_dir = self.install_dir().with_context(|| {
            anyhow::anyhow!(
                "Couldn't find the directory to delete the plugin files from: {:?}",
                self.plugin_files()
            )
        })?;

        tracing::debug!(
            plugin = std::any::type_name::<T>(),
            ?install_dir,
            plugin_files = ?self.plugin_files(),
            "Uninstalling plugin"
        );

        for file_path in self
            .plugin_files()
            .into_iter()
            .filter_map(|it| it.file_name().map(|name| install_dir.join(name)))
            .filter(|it| it.is_file())
        {
            tracing::debug!(?file_path, "Removing plugin file");
            fs_err::remove_file(file_path)?;
        }

        Ok(())
    }
}

#[derive(Debug, Clone)]
pub struct NautilusPlugin {
    files: Vec<PathBuf>,
}

impl FileBasedPlugin for NautilusPlugin {
    fn plugin_files(&self) -> &[PathBuf] {
        self.files.as_slice()
    }

    fn install_dir(&self) -> Option<PathBuf> {
        let mut base_dirs = xdg_data_dirs();

        // In some package formats (like nixpkg), the paths that we're looking into for the nautilus-python directory
        // may not be enough, so here is another one based on the DESTDIR (e.g., `DESTDIR/share`) that's set during the
        // meson build.
        //
        // Here is a snippet where the package maintainers are manually symlinking the plugin script to the required
        // directory since the app is not able to figure it out by itself.
        // https://github.com/NixOS/nixpkgs/pull/416076/files#diff-2b073efb0973697970f4ba24dec07b65f7aea950aa3f48ba4f2d4a92827ac998R74-R76
        base_dirs.insert(0, PathBuf::from(DATADIR));

        // https://gitlab.gnome.org/GNOME/nautilus-python/-/tree/master#running-extensions
        if let Some(data_home_dir) = std::env::var_os("XDG_DATA_HOME")
            .and_then(|it| (!it.is_empty()).then_some(PathBuf::from(it)))
        {
            base_dirs.insert(0, data_home_dir);
        }
        if let Some(home) = dirs::home_dir() {
            base_dirs.insert(0, home.join(".local/share"));
        }

        base_dirs
            .into_iter()
            .map(|it| it.join("nautilus-python/extensions"))
            .find(|it| it.is_dir())
    }

    fn help_install_dir() -> &'static str {
        "~/.local/share/nautilus-python/extensions"
    }
}

impl Default for NautilusPlugin {
    fn default() -> Self {
        Self {
            files: vec![PathBuf::from(PKGDATADIR).join("plugins/packet_nautilus.py")],
        }
    }
}

impl NautilusPlugin {
    pub fn new() -> Self {
        Self::default()
    }
}
