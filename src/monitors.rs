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

    object_manager
        .get_managed_objects()
        .await?
        .into_iter()
        .filter(|(_, interfaces)| {
            interfaces
                .keys()
                .any(|interface| interface.as_str() == BLUEZ_ADAPTER_INTERFACE)
        })
        .map(|(path, _)| path)
        .min_by(|a, b| a.as_str().cmp(b.as_str()))
        .ok_or_else(|| zbus::Error::Failure("No Bluetooth adapter found".to_string()))
}

pub async fn spawn_bluetooth_power_monitor_task(
    conn: zbus::Connection,
    sender: watch::Sender<bool>,
) -> zbus::Result<()> {
    let object_manager = zbus::fdo::ObjectManagerProxy::builder(&conn)
        .destination("org.bluez")?
        .path("/")?
        .build()
        .await?;

    let mut interfaces_added = object_manager.receive_interfaces_added().await?;
    let mut interfaces_removed = object_manager.receive_interfaces_removed().await?;

    let mut current_adapter: Option<(OwnedObjectPath, zbus::proxy::PropertyStream<bool>)> = None;

    let new_current_adapter = async |path: OwnedObjectPath| {
        let proxy = zbus::Proxy::new(&conn, "org.bluez", path.clone(), BLUEZ_ADAPTER_INTERFACE)
            .await
            .ok()?;
        let powered_stream = proxy.receive_property_changed::<bool>("Powered").await;
        if let Ok(powered) = proxy.get_property::<bool>("Powered").await {
            _ = sender.send(powered);
        }
        Some((path, powered_stream))
    };

    if let Ok(path) = find_adapter_path(&conn).await {
        current_adapter = new_current_adapter(path).await;
    } else {
        _ = sender.send(false);
    }

    loop {
        tokio::select! {
            Some(added) = interfaces_added.next() => {
                if let Ok(args) = added.args() && args.interfaces_and_properties().contains_key(BLUEZ_ADAPTER_INTERFACE) {
                    let path: OwnedObjectPath = args.object_path().to_owned().into();
                    current_adapter = new_current_adapter(path).await;
                }
            }
            Some(removed) = interfaces_removed.next() => {
                if let Ok(args) = removed.args() {
                    let should_reset = if let Some((current_path, _)) = &current_adapter {
                        args.object_path().as_str() == current_path.as_str()
                            || args.interfaces().iter().any(|i| i.as_str() == BLUEZ_ADAPTER_INTERFACE)
                    } else {
                        false
                    };

                    if should_reset {
                        current_adapter = None;
                        _ = sender.send(false);

                        if let Ok(new_path) = find_adapter_path(&conn).await {
                            current_adapter = new_current_adapter(new_path).await;
                        }
                    }
                }
            }
            event = async {
                match &mut current_adapter {
                    Some((_, powered_stream)) => powered_stream.next().await,
                    None => std::future::pending().await,
                }
            } => {
                if let Some(event) = event && let Ok(powered) = event.get().await {
                    _ = sender.send(powered);
                }
            }
        }
    }
}

pub async fn is_bluetooth_powered(conn: &zbus::Connection) -> zbus::Result<bool> {
    let adapter_path = find_adapter_path(conn).await?;
    let proxy = zbus::Proxy::new(conn, "org.bluez", adapter_path, BLUEZ_ADAPTER_INTERFACE).await?;

    let value: bool = proxy.get_property("Powered").await?;

    Ok(value)
}
