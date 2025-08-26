## 🔨 Building and Installing

To build and install `packet` using [Meson](https://mesonbuild.com/) and [Ninja](https://ninja-build.org/):

```bash
# Set up the build directory
meson setup builddir

# Compile the project
meson compile -C builddir

# Install the binary and resources (requires sudo)
sudo meson install -C builddir

#Once installed, you can run the application with:
packet
#After installation, the packet binary is available system-wide.


# <img src="data/icons/io.github.nozwock.Packet.svg" /> Packet

A partial implementation of Google's Quick Share protocol that lets you send and receive files wirelessly from Android devices using Quick Share, or another device with Packet.

<div align="center">
    <img src="data/resources/screenshots/packet-receive.png" alt="screenshot" />
</div>

## Installation

[![flathub-installs-badge]][flathub]

<a href="https://flathub.org/apps/details/io.github.nozwock.Packet">
<img src="https://flathub.org/api/badge?svg&locale=en&dark" width="190px" />
</a>

#### Nightly
Nightly Flatpak builds are available from [here][nightly-build].

## Requirements
Since only the Wi-Fi LAN medium is implemented, Packet requires Bluetooth to be enabled and the devices to be connected to a Wi-Fi network with mDNS.

## Translations
If you'd like to help translate Packet to your native language, you can do so using the [Weblate][translation-platform] platform.

[![Translation status][translation-status-widget]][translation-platform]

## FAQ

#### Can't send to app from other devices

Your firewall may be blocking Packet's port. Enable *Static Port* in Preferences and allow it through the firewall. See issue [#35](https://github.com/nozwock/packet/issues/35).

#### "Couldn't open files"

This error occurs if the file is invalid (e.g. empty) or, on Flatpak, if you're sending files (via the Nautilus plugin) from a location Packet can't access.

Consider granting "All system files" access via Flatseal to allow sending from any location. See [Downloads folder keeps resetting](#downloads-folder-keeps-resetting) for details.

#### Downloads folder keeps resetting

In Flatpak, folder access is temporary and resets after a session restart because static access can't be requested. To set a permanent downloads folder, grant access in advance using Flatseal or run:

```sh
flatpak override --user io.github.nozwock.Packet --filesystem='/path/to/your/folder/here'
```

## Plugin Requirements

<!-- Don't change the heading since a link to it is being used in the app. -->

To use the Nautilus plugin, install the required packages:

- Ubuntu/Debian:\
`sudo apt install python3-dbus python3-nautilus`
- Fedora:\
`sudo dnf install python3-dbus nautilus-python`
- Arch:\
`sudo pacman -S python-dbus nautilus-python`
- Fedora Silverblue (rpm-ostree):\
`rpm-ostree install python3-dbus nautilus-python`

## Acknowledgments
- [Dominik Baran][dominik] for creating the icon and working on the app's design.
- [NearDrop][neardrop] for reverse-engineering the closed-source Quick Share implementation in Android's GMS.
- [rquickshare] for their internal Rust implementation of the Quick Share protocol.

## Code of Conduct
Packet follows the [GNOME Code of Conduct][gnome-coc].

[nightly-build]: https://nightly.link/nozwock/packet/workflows/ci/main?preview
[translation-platform]: https://hosted.weblate.org/engage/packet/
[translation-status-widget]: https://hosted.weblate.org/widget/packet/multi-auto.svg
[dominik]: https://gitlab.gnome.org/wallaby
[neardrop]: https://github.com/grishka/NearDrop/
[rquickshare]: https://github.com/Martichou/rquickshare/
[flathub]: https://flathub.org/apps/details/io.github.nozwock.Packet
[flathub-installs-badge]: https://img.shields.io/badge/dynamic/json?label=Installs&url=https%3A%2F%2Fflathub.org%2Fapi%2Fv2%2Fstats%2Fio.github.nozwock.Packet&query=%24.installs_total&logo=flathub&color=007ec6
[gnome-coc]: https://conduct.gnome.org/
