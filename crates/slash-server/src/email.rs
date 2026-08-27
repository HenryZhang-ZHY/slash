//! Outbound transactional email through a private, unauthenticated SMTP relay.

use std::sync::Arc;

use lettre::message::Mailbox;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};

#[derive(Clone)]
pub struct InvitationMailer {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
    base_url: Arc<str>,
}

impl InvitationMailer {
    pub fn new(
        host: &str,
        port: u16,
        from_address: &str,
        from_name: &str,
        base_url: &str,
    ) -> Result<Self, String> {
        let address = from_address
            .parse()
            .map_err(|_| "SLASH_EMAIL_FROM must be a valid email address".to_string())?;
        Ok(Self {
            transport: AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
                .port(port)
                .build(),
            from: Mailbox::new(Some(from_name.to_string()), address),
            base_url: Arc::from(base_url.trim_end_matches('/')),
        })
    }

    pub async fn send_team_invitation(
        &self,
        recipient: &str,
        team_name: &str,
        inviter_name: &str,
        token: &str,
    ) -> Result<(), String> {
        let recipient: Mailbox = recipient
            .parse()
            .map_err(|_| "invalid invitation recipient".to_string())?;
        let invitation_url = format!(
            "{}/invitations/accept#token={}",
            self.base_url,
            urlencoding::encode(token)
        );
        let message = Message::builder()
            .from(self.from.clone())
            .to(recipient)
            .subject(format!("You're invited to join {team_name} on Slash"))
            .body(format!(
                "{inviter_name} invited you to join {team_name} on Slash.\n\nAccept the invitation:\n{invitation_url}\n\nThis invitation expires in 7 days. If you were not expecting it, you can ignore this email.\n"
            ))
            .map_err(|error| error.to_string())?;
        self.transport
            .send(message)
            .await
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
