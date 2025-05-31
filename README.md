# <img src="data/icons/io.github.nozwock.Packet.svg" /> Packet

A partial implementation of Google's Quick Share protocol that lets you send and receive files wirelessly from Android devices using Quick Share, or another device with Packet.

<div align="center">
    <img src="data/resources/screenshots/packet-receive.png" alt="screenshot" />
</div>

## Installation

<a href="https://flathub.org/apps/details/io.github.nozwock.Packet">
<img src="https://flathub.org/api/badge?svg&locale=en&dark" width="190px" />
</a>

#### Nightly
Nightly Flatpak builds are available from [here][nightly-build].

## Requirements
Packet requires Bluetooth to be enabled and the devices to be connected to a Wi-Fi network with mDNS.

## Translations
If you'd like to help translate Packet to your native language, you can do so using the [Weblate][translation-platform] platform.

[![Translation status][translation-status-widget]][translation-platform]

## FAQ

#### Downloads folder keeps resetting

In Flatpak, folder access is temporary and resets after a session restart because static access can't be requested. To set a permanent downloads folder, grant access in advance using Flatseal or run:

```sh
flatpak override --user io.github.nozwock.Packet --filesystem='/path/to/your/folder/here'
```

## Acknowledgments
- [Dominik Baran][dominik] for creating the icon and working on the app's design.
- [NearDrop][neardrop] for reverse-engineering the closed-source Quick Share implementation in Android's GMS.
- [rquickshare] for their internal Rust implementation of the Quick Share protocol.

[nightly-build]: https://nightly.link/nozwock/packet/workflows/ci/main?preview
[translation-platform]: https://hosted.weblate.org/engage/packet/
[translation-status-widget]: https://hosted.weblate.org/widget/packet/multi-auto.svg
[dominik]: https://gitlab.gnome.org/wallaby
[neardrop]: https://github.com/grishka/NearDrop/
[rquickshare]: https://github.com/Martichou/rquickshare/
