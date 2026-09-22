use futures_lite::StreamExt;
use tokio::sync::watch;
use zbus::zvariant::OwnedObjectPath;

const BLUEZ_ADAPTER_INTERFACE: &str = "org.bluez.Adapter1";

/// Finds the object path of a Bluetooth adapter (e.g. `/org/bluez/hci0`) by asking BlueZ
/// for its managed objects, rather than assuming `hci0` always exists: some systems only
/// expose `hci1` or other indices.
async fn find_adapter_path(conn: &zbus::Connection) -> zbus::Result<OwnedObjectPath> {
    let object_manager = zbus::fdo::ObjectManagerProxy::builder(conn)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;

    let mut adapters: Vec<OwnedObjectPath> = object_manager
        .get_managed_objects()
        .await?
        .into_iter()
        .filter(|(_, interfaces)| {
            interfaces
                .keys()
                .any(|interface| interface.as_str() == BLUEZ_ADAPTER_INTERFACE)
        })
        .map(|(path, _)| path)
        .collect();
    adapters.sort_by(|a, b| a.as_str().cmp(b.as_str()));

    adapters.into_iter().next().ok_or_else(|| {
        zbus::Error::Failure("No Bluetooth adapter found".to_string())
    })
}

pub async fn spawn_bluetooth_power_monitor_task(
    conn: zbus::Connection,
    sender: watch::Sender<bool>,
) -> zbus::Result<()> {
    let adapter_path = find_adapter_path(&conn).await?;
    let proxy =
        zbus::Proxy::new(&conn, "org.bluez", adapter_path, BLUEZ_ADAPTER_INTERFACE).await?;

    let mut property_stream = proxy.receive_property_changed::<bool>("Powered").await;
    while let Some(event) = property_stream.next().await {
        if let Ok(powered) = event.get().await {
            _ = sender.send(powered);
        }
    }

    Ok(())
}

pub async fn is_bluetooth_powered(conn: &zbus::Connection) -> zbus::Result<bool> {
    let adapter_path = find_adapter_path(conn).await?;
    let proxy =
        zbus::Proxy::new(conn, "org.bluez", adapter_path, BLUEZ_ADAPTER_INTERFACE).await?;

    let value: bool = proxy.get_property("Powered").await?;

    Ok(value)
}
