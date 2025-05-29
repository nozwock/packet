use futures_lite::StreamExt;
use tokio::sync::watch;

pub async fn spawn_bluetooth_power_monitor_task(
    conn: zbus::Connection,
    sender: watch::Sender<bool>,
) -> zbus::Result<()> {
    let proxy =
        zbus::Proxy::new(&conn, "org.bluez", "/org/bluez/hci0", "org.bluez.Adapter1").await?;

    let mut property_stream = proxy.receive_property_changed::<bool>("Powered").await;
    while let Some(event) = property_stream.next().await {
        if let Ok(powered) = event.get().await {
            _ = sender.send(powered);
        }
    }

    Ok(())
}

pub async fn is_bluetooth_powered(conn: &zbus::Connection) -> zbus::Result<bool> {
    let proxy =
        zbus::Proxy::new(conn, "org.bluez", "/org/bluez/hci0", "org.bluez.Adapter1").await?;

    let value: bool = proxy.get_property("Powered").await?;

    Ok(value)
}
