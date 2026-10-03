use crate::types::InputRequest;
use kuvpn::utils::{CancellationToken, CredentialsProvider};
use std::sync::Mutex;
use tokio::sync::{mpsc, oneshot};

#[derive(Debug)]
pub enum GuiInteraction {
    Request(InputRequest),
    MfaPush(String),
    MfaComplete,
    DismissPrompt,
}

pub struct GuiProvider {
    pub interaction_tx: mpsc::Sender<GuiInteraction>,
    pub cancel_token: CancellationToken,
    pub page_guard: Mutex<Option<Box<dyn Fn() -> bool + Send + Sync>>>,
}

impl CredentialsProvider for GuiProvider {
    fn request_text(&self, msg: &str) -> Option<String> {
        self.request(msg, false, false)
    }
    fn request_password(&self, msg: &str) -> Option<String> {
        self.request(msg, true, false)
    }
    fn request_email(&self, msg: &str) -> Option<String> {
        self.request(msg, false, true)
    }
    fn on_mfa_push(&self, code: &str) {
        let _ = self
            .interaction_tx
            .blocking_send(GuiInteraction::MfaPush(code.to_string()));
    }
    fn on_mfa_complete(&self) {
        let _ = self
            .interaction_tx
            .blocking_send(GuiInteraction::MfaComplete);
    }
    fn set_page_guard(&self, guard: Box<dyn Fn() -> bool + Send + Sync>) {
        if let Ok(mut g) = self.page_guard.lock() {
            *g = Some(guard);
        }
    }
    fn clear_page_guard(&self) {
        if let Ok(mut g) = self.page_guard.lock() {
            *g = None;
        }
    }
}

impl GuiProvider {
    fn request(&self, msg: &str, is_password: bool, is_email: bool) -> Option<String> {
        let (tx, mut rx) = oneshot::channel();
        let request = InputRequest {
            msg: msg.to_string(),
            is_password,
            is_email,
            response_tx: tx,
        };

        let _ = self
            .interaction_tx
            .blocking_send(GuiInteraction::Request(request));

        // Wait for response, but also poll for cancellation and page changes
        loop {
            if self.cancel_token.is_cancelled() {
                let _ = self
                    .interaction_tx
                    .blocking_send(GuiInteraction::DismissPrompt);
                return None;
            }

            // Check page guard — if page changed, dismiss the prompt
            let page_changed = self
                .page_guard
                .lock()
                .ok()
                .and_then(|g| g.as_ref().map(|check| !check()))
                .unwrap_or(false);
            if page_changed {
                log::info!("Page changed while prompting, dismissing prompt");
                let _ = self
                    .interaction_tx
                    .blocking_send(GuiInteraction::DismissPrompt);
                return None;
            }

            match rx.try_recv() {
                Ok(val) => return Some(val),
                Err(oneshot::error::TryRecvError::Empty) => {
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(oneshot::error::TryRecvError::Closed) => return None,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancelling_connection_dismisses_a_pending_prompt() {
        let (interaction_tx, mut interaction_rx) = mpsc::channel(4);
        let token = CancellationToken::new();
        let provider = GuiProvider {
            interaction_tx,
            cancel_token: token.clone(),
            page_guard: Mutex::new(None),
        };
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            result_tx
                .send(provider.request_password("Test password"))
                .unwrap();
        });
        let Some(GuiInteraction::Request(_request)) = interaction_rx.blocking_recv() else {
            panic!("expected a password prompt");
        };
        token.cancel();
        assert_eq!(
            result_rx
                .recv_timeout(std::time::Duration::from_secs(2))
                .unwrap(),
            None
        );
        assert!(matches!(
            interaction_rx.blocking_recv(),
            Some(GuiInteraction::DismissPrompt)
        ));
        worker.join().unwrap();
    }
}
