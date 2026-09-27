# Contributing

You can build, and run this project via:

- [**GNOME Builder**][flathub-gnome-builder].
- [**Visual Studio Code**][gh-vscode] with the [Flatpak extension][gh-flatpak-vscode].

  If there hasn't been a recent release of the extension, you can package it yourself with `npx vsce pack --yarn`.

  Also see the [configuration settings](#visual-studio-code) recommended for VS Code.

- [**Manual Build**](#manual-build) using [meson] if you are on a distribution with access to the latest GTK dependencies.

## Manual Build

The project uses [meson] for its build system. You can build the project either natively or in a Flatpak environment.

- Build and run via [meson]:

  ```sh
  # Build
  meson setup build_dir
  meson compile -C build_dir

  # Run
  meson devenv -C build_dir packet

  # Install & Run
  sudo meson install -C build_dir --no-rebuild
  packet
  ```

- Build and run via Flatpak:

  ```sh
  # Build
  flatpak-builder --user flatpak_build_dir \
      build-aux/io.github.nozwock.Packet.Devel.json

  # Run
  flatpak-builder --run flatpak_build_dir \
      build-aux/io.github.nozwock.Packet.Devel.json \
      packet
  ```

## Configuration

### Visual Studio Code

If using VS Code, add this to your `.vscode/settings.json` to prevent `ripgrep` (which VS Code uses internally) from running indefinitely against Flatpak build artifacts ([bilelmoussaoui/flatpak-vscode#242][gh-flatpak-vscode-issue]):

```json
{
  "files.exclude": {
    "**/.git": true,
    "**/.svn": true,
    "**/.hg": true,
    "**/.DS_Store": true,
    "**/Thumbs.db": true,
    "**/_*build/": true,
    "**/.flatpak-builder/": true,
    "**/.flatpak/": true
  }
}
```

Note: Excluding these only from `files.watcherExclude` doesn't seem to work.

[flathub-gnome-builder]: https://flathub.org/en/apps/org.gnome.Builder
[gh-vscode]: https://github.com/microsoft/vscode
[gh-flatpak-vscode]: https://github.com/bilelmoussaoui/flatpak-vscode/
[gh-flatpak-vscode-issue]: https://github.com/bilelmoussaoui/flatpak-vscode/issues/242
[meson]: https://mesonbuild.com/
