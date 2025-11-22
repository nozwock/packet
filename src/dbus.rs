use std::sync::{Arc, LazyLock};

use tokio::sync::{
    Mutex,
    mpsc::{Receiver, Sender, channel},
};
use zbus::{Connection, object_server::InterfaceRef};

use crate::config::{DBUS_API_NAME, DBUS_API_PATH};

#[derive(Debug)]
pub struct Packet {
    pub visibility: bool,
    visibility_tx: Arc<Mutex<Sender<bool>>>,
    pub visibility_rx: Arc<Mutex<Receiver<bool>>>,
}

#[zbus::interface(name = "io.github.nozwock.Packet1")]
impl Packet {
    #[zbus(property)]
    pub async fn device_visibility(&self) -> bool {
        self.visibility
    }

    /// Also sends the param to the channel associated with `visibility_tx`.
    #[zbus(property)]
    pub async fn set_device_visibility(&mut self, visibility: bool) {
        self.visibility = visibility;
        _ = self
            .visibility_tx
            .lock()
            .await
            .send(visibility)
            .await
            .inspect_err(|err| tracing::warn!(%err));
    }
}

static CONNECTION: LazyLock<Mutex<Option<Connection>>> = LazyLock::new(|| Mutex::new(None));

pub async fn get_connection() -> Option<Connection> {
    CONNECTION.lock().await.as_ref().cloned()
}

pub async fn create_connection(visibility: bool) -> anyhow::Result<Connection> {
    let mut conn_guard = CONNECTION.lock().await;

    if let Some(conn) = conn_guard.as_ref() {
        Ok(conn.clone())
    } else {
        let (tx, rx) = channel::<bool>(1);
        let conn = zbus::connection::Builder::session()?
            .name(DBUS_API_NAME)?
            .serve_at(
                DBUS_API_PATH,
                Packet {
                    visibility: visibility,
                    visibility_tx: Arc::new(Mutex::new(tx)),
                    visibility_rx: Arc::new(Mutex::new(rx)),
                },
            )?
            .build()
            .await?;
        *conn_guard = Some(conn.clone());

        Ok(conn)
    }
}

/// # Panics
/// Panics if `CONNECTION` is `None`.
pub async fn packet_iface() -> InterfaceRef<Packet> {
    get_connection()
        .await
        .expect("Session should be created before getting iface")
        .object_server()
        .interface::<_, Packet>(DBUS_API_PATH)
        .await
        .expect("Interface should be on the object path")
}
