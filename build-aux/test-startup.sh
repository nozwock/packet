#!/bin/sh
# Exercise the real application on private session buses, without requiring BlueZ.
set -eu

schema_file=$1
app_id=$2
bus_mode=$3
test_dir=$(mktemp -d)
packet_pid=

cleanup() {
    result=$?
    trap - EXIT
    if [ -n "$packet_pid" ]; then
        kill "$packet_pid" 2>/dev/null || true
        wait "$packet_pid" 2>/dev/null || true
    fi
    if [ "$result" -ne 0 ] && [ -f "$test_dir/packet.log" ]; then
        cat "$test_dir/packet.log"
    fi
    rm -rf "$test_dir"
    exit "$result"
}
trap cleanup EXIT
trap 'exit 1' HUP INT TERM

mkdir "$test_dir/schemas"
cp "$schema_file" "$test_dir/schemas/"
glib-compile-schemas "$test_dir/schemas"
export GSETTINGS_SCHEMA_DIR="$test_dir/schemas"
export GSETTINGS_BACKEND=memory
export XDG_CONFIG_HOME="$test_dir/config"
export XDG_CACHE_HOME="$test_dir/cache"
export XDG_DATA_HOME="$test_dir/data"
export GTK_A11Y=none
export GSK_RENDERER=cairo

case "$bus_mode" in
    unavailable) export DBUS_SYSTEM_BUS_ADDRESS="unix:path=$test_dir/no-system-bus" ;;
    private) export DBUS_SYSTEM_BUS_ADDRESS="$DBUS_SESSION_BUS_ADDRESS" ;;
    *) exit 1 ;;
esac

packet > "$test_dir/packet.log" 2>&1 &
packet_pid=$!
object_path=/$(printf '%s' "$app_id" | tr . /)

device_name() {
    gdbus call --session --timeout 1 --dest "$app_id.Api" --object-path "$object_path" \
        --method org.freedesktop.DBus.Properties.Get io.github.nozwock.Packet1 DeviceName
}

for attempt in $(seq 1 50); do
    kill -0 "$packet_pid"
    if device_name >/dev/null 2>&1; then
        # The API can register just before another startup task panics.
        sleep 2
        kill -0 "$packet_pid"
        device_name
        # GLib can catch a task panic while leaving the application API alive.
        if grep -q "panicked at" "$test_dir/packet.log"; then
            exit 1
        fi
        exit 0
    fi
    sleep 0.1
done
exit 1
