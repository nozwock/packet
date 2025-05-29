set ignore-comments

default:
    @just --list


# Generate packet.pot
pot: potfiles
    # xgettext seems to follow `translatable="no"` in the metainfo file too, so that's nice
    #
    # For `.blp`
    # --keyword=_ \
    # --keyword=C_:1c,2 \
    xgettext \
        --from-code=UTF-8 \
        --add-comments \
        --keyword=_ \
        --keyword=C_:1c,2 \
        --package-name packet \
        --default-domain packet \
        --files-from "po/POTFILES.in" \
        --output "po/packet.pot" \

    # https://github.com/mesonbuild/meson/issues/12368
    # cat "{{ source_directory() / "po" / "LINGUAS" }}" | while read line; do touch "{{ source_directory() / "po" }}/${line}.po"; done
    # https://mesonbuild.com/Localisation.html
    # meson compile packet-pot
    # meson compile packet-update-po

potfiles_path := source_directory() / "po" / "POTFILES.in"

potfiles:
    #!/usr/bin/env bash
    # https://github.com/sharkdp/fd

    # For some reason, gitignore aren't respected when a pattern arg is included
    fd --ignore-file .gitignore -tf --extension blp '.*' "data/resources" > {{ potfiles_path }}
    fd --ignore-file .gitignore -tf --extension rs '.*' src >> {{ potfiles_path }}

    cat <<EOF >> {{ potfiles_path }}
    data/io.github.nozwock.Packet.desktop.in.in
    data/io.github.nozwock.Packet.gschema.xml.in
    data/io.github.nozwock.Packet.metainfo.xml.in.in
    EOF
