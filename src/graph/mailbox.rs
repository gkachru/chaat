//! The signed-in person's automatic replies (Out of Office), kept by
//! Exchange in their mailbox settings (MailboxSettings.ReadWrite). Teams
//! shows the person as out of office while they are on.

use serde_json::{Value, json};

use super::types as g;
use super::{Graph, GraphError, convert};
use crate::model::{AutoReplies, ExternalAudience, OutOfOffice};

impl Graph {
    /// `GET /me/mailboxSettings/automaticRepliesSetting`.
    pub async fn out_of_office(&self) -> Result<OutOfOffice, GraphError> {
        let raw: g::AutomaticRepliesSetting = self
            .get("me/mailboxSettings/automaticRepliesSetting")
            .await?;
        Ok(convert::out_of_office(raw))
    }

    /// `PATCH /me/mailboxSettings` with the automatic replies alone.
    pub async fn set_out_of_office(&self, value: &OutOfOffice) -> Result<(), GraphError> {
        self.patch(
            "me/mailboxSettings",
            &json!({ "automaticRepliesSetting": automatic_replies(value) }),
        )
        .await
    }
}

/// Whether `error` means the account has no Exchange Online mailbox, so it
/// has no automatic replies to set.
pub fn no_mailbox(error: &GraphError) -> bool {
    match error {
        GraphError::NotFound => true,
        GraphError::Http { code, .. } => {
            code.eq_ignore_ascii_case("MailboxNotEnabledForRESTAPI")
                || code.eq_ignore_ascii_case("MailboxNotFound")
        }
        _ => false,
    }
}

fn automatic_replies(value: &OutOfOffice) -> Value {
    let mut body = json!({
        "status": match value.replies {
            AutoReplies::Off => "disabled",
            AutoReplies::On => "alwaysEnabled",
            AutoReplies::Scheduled => "scheduled",
        },
        "externalAudience": match value.audience {
            ExternalAudience::None => "none",
            ExternalAudience::Contacts => "contactsOnly",
            ExternalAudience::All => "all",
        },
        "internalReplyMessage": reply_html(&value.internal),
        "externalReplyMessage": reply_html(&value.external),
    });
    // Off and on leave Exchange's last window as it is.
    if value.replies == AutoReplies::Scheduled
        && let (Some(start), Some(end)) = (value.start, value.end)
    {
        let at =
            |millis| json!({ "dateTime": crate::time::to_utc_civil(millis), "timeZone": "UTC" });
        body["scheduledStartDateTime"] = at(start);
        body["scheduledEndDateTime"] = at(end);
    }
    body
}

/// Plain text as the HTML Outlook expects, one line per line.
fn reply_html(text: &str) -> String {
    text.trim()
        .lines()
        .map(crate::html::escape)
        .collect::<Vec<_>>()
        .join("<br>")
}

#[cfg(test)]
mod tests {
    use super::super::testing;
    use super::*;
    use wiremock::matchers::{body_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn graph(server: &MockServer) -> Graph {
        Graph::with_base(
            reqwest::Client::new(),
            testing::session("at", &server.uri()).await,
            &server.uri(),
        )
    }

    #[tokio::test]
    async fn reads_automatic_replies_as_plain_text() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/mailboxSettings/automaticRepliesSetting"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "status": "scheduled",
                "externalAudience": "contactsOnly",
                "scheduledStartDateTime": {"dateTime": "2026-11-24T08:00:00.0000000", "timeZone": "UTC"},
                "scheduledEndDateTime": {"dateTime": "2026-11-28T17:30:00.0000000", "timeZone": "UTC"},
                "internalReplyMessage": "<html><head><style>p{}</style></head><body><p>Away until Friday.</p><p>Ask Ada &amp; Grace.</p></body></html>",
                "externalReplyMessage": ""
            })))
            .expect(1)
            .mount(&server)
            .await;
        let value = graph(&server).await.out_of_office().await.unwrap();
        assert_eq!(value.replies, AutoReplies::Scheduled);
        assert_eq!(value.audience, ExternalAudience::Contacts);
        assert_eq!(
            value.start.map(crate::time::to_iso).as_deref(),
            Some("2026-11-24T08:00:00.000Z")
        );
        assert_eq!(
            value.end.map(crate::time::to_iso).as_deref(),
            Some("2026-11-28T17:30:00.000Z")
        );
        assert_eq!(value.internal, "Away until Friday.\nAsk Ada & Grace.");
        assert_eq!(value.external, "");
    }

    #[tokio::test]
    async fn writes_automatic_replies() {
        let server = MockServer::start().await;
        Mock::given(method("PATCH"))
            .and(path("/me/mailboxSettings"))
            .and(body_json(serde_json::json!({"automaticRepliesSetting": {
                "status": "scheduled",
                "externalAudience": "all",
                "internalReplyMessage": "Away &lt;until&gt; Friday.<br>Ask Ada.",
                "externalReplyMessage": "Away.",
                "scheduledStartDateTime": {"dateTime": "2026-11-24T08:00:00", "timeZone": "UTC"},
                "scheduledEndDateTime": {"dateTime": "2026-11-28T17:30:00", "timeZone": "UTC"}
            }})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        Mock::given(method("PATCH"))
            .and(path("/me/mailboxSettings"))
            .and(body_json(serde_json::json!({"automaticRepliesSetting": {
                "status": "disabled",
                "externalAudience": "none",
                "internalReplyMessage": "",
                "externalReplyMessage": ""
            }})))
            .respond_with(ResponseTemplate::new(200))
            .expect(1)
            .mount(&server)
            .await;
        let graph = graph(&server).await;
        let scheduled = OutOfOffice {
            replies: AutoReplies::Scheduled,
            start: crate::time::parse_iso("2026-11-24T08:00:00Z"),
            end: crate::time::parse_iso("2026-11-28T17:30:00Z"),
            internal: " Away <until> Friday.\nAsk Ada.\n".into(),
            external: "Away.".into(),
            audience: ExternalAudience::All,
        };
        graph.set_out_of_office(&scheduled).await.unwrap();
        graph
            .set_out_of_office(&OutOfOffice::default())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn an_account_without_a_mailbox_is_recognised() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/me/mailboxSettings/automaticRepliesSetting"))
            .respond_with(ResponseTemplate::new(404).set_body_json(serde_json::json!({
                "error": {"code": "MailboxNotEnabledForRESTAPI", "message": "secret text"}
            })))
            .mount(&server)
            .await;
        let error = graph(&server).await.out_of_office().await.unwrap_err();
        assert!(no_mailbox(&error));
        assert!(!no_mailbox(&GraphError::Decode));
        assert!(no_mailbox(&GraphError::Http {
            status: 400,
            code: "MailboxNotEnabledForRESTAPI".into()
        }));
    }
}
