//! Pairing requests waiting for the person at the computer.
//!
//! A request is answered from a desktop notification or with `iolite approve <code>`.

use std::collections::HashMap;
use std::future::Future;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::oneshot;
use tracing::{info, warn};
use zbus::zvariant::Value;

use crate::config;

const PAIRING_TIMEOUT: Duration = Duration::from_secs(120);

#[zbus::proxy(
    interface = "org.freedesktop.Notifications",
    default_service = "org.freedesktop.Notifications",
    default_path = "/org/freedesktop/Notifications",
    gen_blocking = false
)]
trait Notifications {
    #[allow(clippy::too_many_arguments)]
    fn notify(
        &self,
        app_name: &str,
        replaces_id: u32,
        app_icon: &str,
        summary: &str,
        body: &str,
        actions: &[&str],
        hints: HashMap<&str, Value<'_>>,
        expire_timeout: i32,
    ) -> zbus::Result<u32>;

    fn close_notification(&self, id: u32) -> zbus::Result<()>;

    #[zbus(signal)]
    fn action_invoked(&self, id: u32, action_key: &str) -> zbus::Result<()>;
}

struct Pending {
    device: String,
    decide: Option<oneshot::Sender<bool>>,
}

pub struct Pairing {
    pending: Mutex<HashMap<String, Pending>>,
    notifications: Option<NotificationsProxy<'static>>,
    notification_codes: Mutex<HashMap<u32, String>>,
}

impl Pairing {
    pub async fn new(bus: Option<&zbus::Connection>) -> Arc<Self> {
        let notifications = match bus {
            Some(bus) => NotificationsProxy::new(bus).await.ok(),
            None => None,
        };
        let this = Arc::new(Self {
            pending: Mutex::default(),
            notifications,
            notification_codes: Mutex::default(),
        });
        if let Some(proxy) = this.notifications.clone() {
            tokio::spawn(listen_for_actions(proxy, Arc::downgrade(&this)));
        }
        this
    }

    /// Waits until the request is accepted, declined, times out or `cancelled` resolves.
    pub async fn request<C: Future>(&self, code: &str, device: &str, cancelled: C) -> bool {
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(
            code.to_owned(),
            Pending {
                device: device.to_owned(),
                decide: Some(tx),
            },
        );
        info!(
            "pairing request from {device}, code {}: accept it in the notification or run `iolite approve {code}`",
            spaced(code)
        );
        let notification = self.notify(code, device).await;
        let approved = tokio::select! {
            r = rx => r.unwrap_or(false),
            _ = tokio::time::sleep(PAIRING_TIMEOUT) => false,
            _ = cancelled => false,
        };
        self.pending.lock().unwrap().remove(code);
        if let (Some(proxy), Some(id)) = (&self.notifications, notification) {
            self.notification_codes.lock().unwrap().remove(&id);
            let _ = proxy.close_notification(id).await;
        }
        approved
    }

    /// Returns false if no request with this code is waiting.
    pub fn decide(&self, code: &str, approve: bool) -> bool {
        let tx = self
            .pending
            .lock()
            .unwrap()
            .get_mut(code)
            .and_then(|p| p.decide.take());
        match tx {
            Some(tx) => tx.send(approve).is_ok(),
            None => false,
        }
    }

    pub fn list(&self) -> Vec<(String, String)> {
        self.pending
            .lock()
            .unwrap()
            .iter()
            .map(|(code, p)| (code.clone(), p.device.clone()))
            .collect()
    }

    async fn notify(&self, code: &str, device: &str) -> Option<u32> {
        let proxy = self.notifications.as_ref()?;
        let (summary, body, accept, decline) = if config::korean() {
            (
                "iolite 페어링 요청".to_owned(),
                format!(
                    "{device}에서 이 컴퓨터를 제어하려고 합니다. 기기에 {}가 보일 때만 수락하세요.",
                    spaced(code)
                ),
                "수락",
                "거절",
            )
        } else {
            (
                "iolite pairing request".to_owned(),
                format!(
                    "{device} wants to control this computer. Accept only if it shows {}.",
                    spaced(code)
                ),
                "Accept",
                "Decline",
            )
        };
        let hints = HashMap::from([("urgency", Value::from(2u8))]);
        let result = proxy
            .notify(
                "iolite",
                0,
                "preferences-desktop-remote-desktop",
                &summary,
                &body,
                &["accept", accept, "decline", decline],
                hints,
                0,
            )
            .await;
        match result {
            Ok(id) => {
                self.notification_codes
                    .lock()
                    .unwrap()
                    .insert(id, code.to_owned());
                Some(id)
            }
            Err(e) => {
                warn!("cannot show the pairing notification: {e}");
                None
            }
        }
    }
}

async fn listen_for_actions(proxy: NotificationsProxy<'static>, pairing: Weak<Pairing>) {
    let mut actions = match proxy.receive_action_invoked().await {
        Ok(actions) => actions,
        Err(e) => {
            warn!("cannot follow notification actions: {e}");
            return;
        }
    };
    while let Some(signal) = actions.next().await {
        let Ok(args) = signal.args() else { continue };
        let Some(pairing) = pairing.upgrade() else {
            break;
        };
        let code = pairing
            .notification_codes
            .lock()
            .unwrap()
            .get(args.id())
            .cloned();
        if let Some(code) = code {
            pairing.decide(&code, *args.action_key() == "accept");
        }
    }
}

pub fn spaced(code: &str) -> String {
    match code.get(..3).zip(code.get(3..)) {
        Some((a, b)) => format!("{a} {b}"),
        None => code.to_owned(),
    }
}
