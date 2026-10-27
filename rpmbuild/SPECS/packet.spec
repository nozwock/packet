Name:           packet
Version:        0.7.3
Release:        1%{?dist}
Summary:        Quick Share client for Linux

%define debug_package %{nil}

License:        GPL-3.0-or-later
URL:            https://github.com/nozwock/packet
Source0:        https://github.com/nozwock/packet/archive/%{version}/packet-%{version}.tar.gz

BuildRequires:  meson >= 0.59.0
BuildRequires:  ninja-build
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  pkgconfig
BuildRequires:  gtk4-devel >= 4.22.0
BuildRequires:  libadwaita-devel >= 1.9.0
BuildRequires:  dbus-devel
BuildRequires:  protobuf-compiler
BuildRequires:  blueprint-compiler
BuildRequires:  glib2-devel
BuildRequires:  desktop-file-utils
BuildRequires:  appstream
BuildRequires:  gettext

Requires:       gtk4 >= 4.22.0
Requires:       libadwaita >= 1.9.0

%description
A partial implementation of Google's Quick Share protocol that lets you
send and receive files wirelessly from Android devices using Quick Share,
or another device with Packet.

%prep
%autosetup -n packet-%{version}

%build
meson setup builddir --prefix=/usr --buildtype=release
ninja -C builddir

%install
DESTDIR=%{buildroot} ninja -C builddir install

%files
%{_bindir}/packet
%{_datadir}/applications/io.github.nozwock.Packet.desktop
%{_datadir}/metainfo/io.github.nozwock.Packet.metainfo.xml
%{_datadir}/metainfo/io.github.nozwock.Packet.releases.xml
%{_datadir}/icons/hicolor/*/apps/io.github.nozwock.Packet.svg
%{_datadir}/icons/hicolor/symbolic/apps/io.github.nozwock.Packet-symbolic.svg
%{_datadir}/glib-2.0/schemas/io.github.nozwock.Packet.gschema.xml
%{_datadir}/dbus-1/services/io.github.nozwock.Packet.service
%{_datadir}/dbus-1/services/io.github.nozwock.Packet.Api.service
%{_datadir}/locale/*/LC_MESSAGES/packet.mo
%{_datadir}/packet/
%license LICENSE

%changelog
* Sat Oct 24 2026 Fame <fame@famelinuxpc> - 0.7.3-1
- Initial RPM package release
