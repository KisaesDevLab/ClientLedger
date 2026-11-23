//! LAN multi-user resources (P33 D3 step 9): the host's sessions (U-290),
//! pairing and host-mode audit events (U-290), single-user maintenance
//! (U-293) and the change feed (U-294).
//!
//! - Sessions and audit events follow the engine as built (U-290, see
//!   `target/plans/P33-engine-status.md`). The sessions routes are host-only
//!   and super-admin; the audit route takes the launch token and no session.
//! - **Maintenance and the change feed are provisional**: U-293 and U-294
//!   are not built. Their calls are refused locally (`Local(Unsupported)`,
//!   nothing sent) until the engine advertises `maintenance-v1` or
//!   `changes-v1`; the shapes below are the plan's (plan U-293, U-294) and
//!   are checked again when the engine lands. Without `changes-v1` the app
//!   polls the ledger-version stamp instead (Q-P33i).
//!
//! Nothing here carries an amount, a name of a record or a memo; session
//! rows carry the user's e-mail and display name, which the host's setup
//! screen shows (never logged: `Debug` is redacted).

use serde::{Deserialize, Serialize};

use crate::client::{AttemptAuth, Call, Method, TimeoutClass};
use crate::errors::{ApiError, LocalValidation};
use crate::idempotency::StaticAuth;
use crate::resources::operations::{
    CHANGES, LAN_AUDIT_EVENT, LAN_SESSION_END, LAN_SESSIONS, LAN_SESSIONS_END_DEVICE,
    MAINTENANCE_END, MAINTENANCE_START, MAINTENANCE_STATUS,
};
use crate::session::{Client, Session, decode, json_body};
use crate::wire::Capabilities;

/// One signed-in session chain (`GET /desktop/sessions`).
#[derive(Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanSession {
    /// The chain id (`sid`), for [`Session::end_lan_session`].
    pub id: String,
    /// The user.
    #[serde(default)]
    pub user_id: Option<String>,
    /// The user's e-mail (shown on the host only).
    #[serde(default)]
    pub email: Option<String>,
    /// The user's name (shown on the host only).
    #[serde(default)]
    pub display_name: Option<String>,
    /// The workstation's device fingerprint (64 hex); `None`: the host app.
    #[serde(default)]
    pub device_id: Option<String>,
    /// When the chain signed in.
    #[serde(default)]
    pub signed_in_at: Option<String>,
    /// The last refresh.
    #[serde(default)]
    pub last_refresh_at: Option<String>,
    /// When the refresh token runs out.
    #[serde(default)]
    pub expires_at: Option<String>,
    /// This is the caller's own session.
    #[serde(default)]
    pub current: bool,
}

impl std::fmt::Debug for LanSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LanSession")
            .field(
                "device",
                &self.device_id.as_deref().map(|d| &d[..d.len().min(8)]),
            )
            .field("current", &self.current)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct SessionsWire {
    #[serde(default)]
    sessions: Vec<LanSession>,
}

#[derive(Deserialize)]
struct RevokedWire {
    #[serde(default)]
    revoked: u32,
}

/// A host event for the engine's audit trail (M7).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum LanAuditEvent {
    /// A workstation was paired.
    LanDevicePaired,
    /// A workstation was removed.
    LanDeviceRemoved,
    /// The host's security certificate was made again.
    LanHostKeyRegenerated,
    /// Sharing on the office network was turned on.
    LanHostModeOn,
    /// Sharing on the office network was turned off.
    LanHostModeOff,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AuditWire<'a> {
    event: LanAuditEvent,
    #[serde(skip_serializing_if = "Option::is_none")]
    device_id: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    label: Option<&'a str>,
}

/// True for a device fingerprint as the engine takes it (64 hex).
#[must_use]
pub fn is_device_id(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}

impl Client {
    /// Records a host event in the audit trail (launch token, no session;
    /// never retried). `label` is cut to 100 characters.
    ///
    /// # Errors
    /// `Local(InvalidValue)` for a device id that is not 64 hex (nothing
    /// sent); 409 `NOT_INITIALIZED` before first run; any [`ApiError`].
    pub async fn lan_audit_event(
        &self,
        event: LanAuditEvent,
        device_id: Option<&str>,
        label: Option<&str>,
    ) -> Result<(), ApiError> {
        if device_id.is_some_and(|d| !is_device_id(d)) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let label = label.map(|l| {
            let end = l.char_indices().nth(100).map_or(l.len(), |(i, _)| i);
            &l[..end]
        });
        let body = json_body(&AuditWire {
            event,
            device_id,
            label,
        })?;
        let call = Call::at(Method::Post, LAN_AUDIT_EVENT.route())
            .json_body(body)
            .timeout(TimeoutClass::Auth);
        self.exec()
            .run(
                &LAN_AUDIT_EVENT,
                call,
                &StaticAuth(AttemptAuth::default()),
                None,
            )
            .await?;
        Ok(())
    }
}

/// Who holds maintenance (U-293, `maintenance-v1`).
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct MaintenanceState {
    /// Maintenance is on: only the holder may work.
    pub active: bool,
    /// The holder's session chain (compare with the signed-in one).
    pub holder_session: Option<String>,
    /// The holder's user id.
    pub holder_user: Option<String>,
    /// When it started (ISO).
    pub started_at: Option<String>,
}

/// What happened to one record (U-294).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ChangeOp {
    /// Created or changed: read it again.
    Upsert,
    /// Deleted.
    Delete,
    /// Moved out of this viewer's sight (a hidden account): drop it.
    Left,
}

/// One change (U-294, `changes-v1`): ids and kinds only.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChangeItem {
    /// The table: `transactions`, `accounts`, `contacts`, `items`,
    /// `tags`, `companies`, ...
    pub kind: String,
    /// The record's id.
    pub id: String,
    /// The latest operation on it.
    pub op: ChangeOp,
}

/// One page of the change feed.
#[derive(Clone, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ChangePage {
    /// Where the next read starts.
    pub cursor: Option<String>,
    /// Changes when the books were replaced (restore, import, migration).
    pub feed_epoch: Option<String>,
    /// The cursor is too old or from another epoch: read everything again.
    pub reset: bool,
    /// What changed since the cursor.
    pub changes: Vec<ChangeItem>,
}

impl Session {
    fn require(&self, cap: &str) -> Result<(), ApiError> {
        if self.client().capabilities().has(cap) {
            Ok(())
        } else {
            Err(ApiError::Local(LocalValidation::Unsupported))
        }
    }

    /// The signed-in sessions (host, super admin), newest refresh first.
    ///
    /// # Errors
    /// `Local(Unsupported)` without `lan-sessions-v1`; `Forbidden`; any
    /// [`ApiError`].
    pub async fn lan_sessions(&self) -> Result<Vec<LanSession>, ApiError> {
        self.require(Capabilities::LAN_SESSIONS)?;
        let call = Call::at(Method::Get, LAN_SESSIONS.route());
        let reply = self.send(&LAN_SESSIONS, call, None).await?;
        Ok(decode::<SessionsWire>(&reply, "sessions")?.sessions)
    }

    /// Ends one session chain; answers how many rows ended.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `NotFound` when it already ended; any
    /// [`ApiError`].
    pub async fn end_lan_session(&self, id: &str) -> Result<u32, ApiError> {
        self.require(Capabilities::LAN_SESSIONS)?;
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let call = Call::new(
            Method::Delete,
            LAN_SESSION_END.route(),
            format!("/api/v1/desktop/sessions/{id}"),
        )
        .timeout(TimeoutClass::Auth);
        let reply = self.send(&LAN_SESSION_END, call, None).await?;
        Ok(decode::<RevokedWire>(&reply, "sessions")?.revoked)
    }

    /// Ends every session of one workstation (Remove); 0 is fine.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Local(InvalidValue)` for a bad device id;
    /// any [`ApiError`].
    pub async fn end_device_sessions(&self, device_id: &str) -> Result<u32, ApiError> {
        self.require(Capabilities::LAN_SESSIONS)?;
        if !is_device_id(device_id) {
            return Err(ApiError::Local(LocalValidation::InvalidValue));
        }
        let mut call =
            Call::at(Method::Delete, LAN_SESSIONS_END_DEVICE.route()).timeout(TimeoutClass::Auth);
        call.query = vec![("device".to_string(), device_id.to_ascii_lowercase())];
        let reply = self.send(&LAN_SESSIONS_END_DEVICE, call, None).await?;
        Ok(decode::<RevokedWire>(&reply, "sessions")?.revoked)
    }

    /// Maintenance now (U-293, provisional).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `maintenance-v1`; any [`ApiError`].
    pub async fn maintenance(&self) -> Result<MaintenanceState, ApiError> {
        self.require(Capabilities::MAINTENANCE)?;
        let call = Call::at(Method::Get, MAINTENANCE_STATUS.route());
        let reply = self.send(&MAINTENANCE_STATUS, call, None).await?;
        decode(&reply, "maintenance")
    }

    /// Takes maintenance: the engine stops new saves, waits for running
    /// ones, then ends the other sessions, unless `keep_sessions` (a
    /// routine hold: payroll, imports; PR-8 S6): the others then only get
    /// 503 `MAINTENANCE` while it lasts and stay signed in. An engine
    /// without `maintenance-sessions-v1` ignores the flag and ends them.
    /// Never retried; a busy engine answers `Lan(MaintenanceBusy)`.
    ///
    /// # Errors
    /// `Local(Unsupported)`; `Lan(MaintenanceBusy)`; `Forbidden`; any
    /// [`ApiError`].
    pub async fn start_maintenance(
        &self,
        keep_sessions: bool,
    ) -> Result<MaintenanceState, ApiError> {
        self.require(Capabilities::MAINTENANCE)?;
        let body: &[u8] = if keep_sessions {
            br#"{"keepSessions":true}"#
        } else {
            b"{}"
        };
        let call = Call::at(Method::Post, MAINTENANCE_START.route())
            .json_body(body.to_vec())
            .timeout(TimeoutClass::Write);
        let reply = self.send(&MAINTENANCE_START, call, None).await?;
        decode(&reply, "maintenance")
    }

    /// Gives maintenance back. Never retried.
    ///
    /// # Errors
    /// `Local(Unsupported)`; any [`ApiError`].
    pub async fn end_maintenance(&self) -> Result<MaintenanceState, ApiError> {
        self.require(Capabilities::MAINTENANCE)?;
        let call = Call::at(Method::Delete, MAINTENANCE_END.route()).timeout(TimeoutClass::Auth);
        let reply = self.send(&MAINTENANCE_END, call, None).await?;
        decode(&reply, "maintenance")
    }

    /// Reads the change feed after `cursor` (`None`: from now).
    ///
    /// # Errors
    /// `Local(Unsupported)` without `changes-v1` (poll the ledger-version
    /// stamp instead); `NoCompanyOpen`; any [`ApiError`].
    pub async fn changes(&self, cursor: Option<&str>) -> Result<ChangePage, ApiError> {
        self.require(Capabilities::CHANGES)?;
        let mut call = Call::at(Method::Get, CHANGES.route());
        if let Some(c) = cursor {
            call.query = vec![("cursor".to_string(), c.to_string())];
        }
        let reply = self.send(&CHANGES, call, None).await?;
        decode(&reply, "changes")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resources::testkit::{json, query, requests, session_with_caps};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer};

    const DEV: &str = "ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12ab12";

    #[tokio::test]
    async fn sessions_need_the_capability_and_end_by_device() {
        let server = MockServer::start().await;
        let s = session_with_caps(&server, &[]).await;
        assert!(matches!(
            s.lan_sessions().await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/sessions"))
            .respond_with(json(
                200,
                &format!(
                    r#"{{"sessions":[{{"id":"c1","userId":"u","email":"a@b.c","displayName":"A",
                    "deviceId":"{DEV}","signedInAt":"x","current":false}},{{"id":"c2","deviceId":null,"current":true}}]}}"#
                ),
            ))
            .mount(&server)
            .await;
        Mock::given(method("DELETE"))
            .and(path("/api/v1/desktop/sessions"))
            .respond_with(json(200, r#"{"revoked":2}"#))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::LAN_SESSIONS]).await;
        let rows = s.lan_sessions().await.expect("rows");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].device_id.as_deref(), Some(DEV));
        assert!(rows[1].current && rows[1].device_id.is_none());
        assert!(!format!("{rows:?}").contains("a@b.c"));
        assert_eq!(s.end_device_sessions(DEV).await.expect("end"), 2);
        let d = requests(&server, "DELETE").await;
        assert_eq!(query(&d[0]), format!("device={DEV}"));
        assert!(matches!(
            s.end_device_sessions("nothex").await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
        assert!(matches!(
            s.end_lan_session("a/b").await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
    }

    #[tokio::test]
    async fn an_audit_event_is_sent_once_with_the_device() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/desktop/audit-events"))
            .respond_with(json(201, r#"{"recorded":true}"#))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[]).await;
        s.client()
            .lan_audit_event(
                LanAuditEvent::LanDevicePaired,
                Some(DEV),
                Some("Front desk"),
            )
            .await
            .expect("recorded");
        let r = requests(&server, "POST").await;
        assert_eq!(r.len(), 1);
        let body: serde_json::Value = serde_json::from_slice(&r[0].body).expect("json");
        assert_eq!(body["event"], "lan_device_paired");
        assert_eq!(body["deviceId"], DEV);
        // No session token on the launch-token route.
        assert!(r[0].headers.get("authorization").is_none());
        assert!(matches!(
            s.client()
                .lan_audit_event(LanAuditEvent::LanDeviceRemoved, Some("x"), None)
                .await,
            Err(ApiError::Local(LocalValidation::InvalidValue))
        ));
    }

    /// S12: after the generation moves, the next request reads
    /// `/desktop/info` again first; a failed re-read drops every
    /// capability (the no-capability policy) and is tried again.
    #[tokio::test]
    async fn a_new_generation_reads_desktop_info_again() {
        use crate::endpoint::{EndpointState, EngineEndpoint, FixedEndpoint};
        use crate::session::Scope;
        use cl_core::Secret;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/info"))
            .respond_with(json(
                200,
                r#"{"initialized":true,"capabilities":["lan-sessions-v1"]}"#,
            ))
            .up_to_n_times(2)
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/info"))
            .respond_with(json(500, r#"{"error":{"message":"x"}}"#))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/sessions"))
            .respond_with(json(200, r#"{"sessions":[]}"#))
            .mount(&server)
            .await;
        let port = server.address().port();
        let src = std::sync::Arc::new(FixedEndpoint::new(port, Secret::new("t".repeat(64)), 1));
        let s = crate::session::tests::session_with(src.clone(), Scope::Workspace);
        s.client().desktop_info().await.expect("info");
        let infos = |reqs: &[wiremock::Request]| {
            reqs.iter()
                .filter(|r| r.url.path() == "/api/v1/desktop/info")
                .count()
        };
        s.lan_sessions().await.expect("same generation");
        assert_eq!(infos(&requests(&server, "GET").await), 1);
        let restart = |g| {
            src.set(EndpointState::Ready(EngineEndpoint {
                port,
                token: Secret::new("t".repeat(64)),
                generation: g,
                clock_offset_ms: 0,
            }));
        };
        restart(2);
        // This request sees the new generation; the next one re-reads.
        s.lan_sessions().await.expect("sessions");
        s.lan_sessions().await.expect("after the re-read");
        assert_eq!(infos(&requests(&server, "GET").await), 2);
        restart(3);
        s.lan_sessions().await.expect("sessions");
        // The re-read fails (this request was already past its capability
        // check): no capability is assumed any more.
        s.lan_sessions()
            .await
            .expect("sent before the re-read failed");
        assert!(matches!(
            s.lan_sessions().await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        assert!(!s.client().capabilities().has(Capabilities::LAN_SESSIONS));
    }

    #[tokio::test]
    async fn maintenance_and_the_feed_wait_for_their_capabilities() {
        let server = MockServer::start().await;
        let s = session_with_caps(&server, &[]).await;
        assert!(matches!(
            s.maintenance().await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        assert!(matches!(
            s.changes(None).await,
            Err(ApiError::Local(LocalValidation::Unsupported))
        ));
        assert_eq!(requests(&server, "POST").await.len(), 0);
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/changes"))
            .respond_with(json(
                200,
                r#"{"cursor":"9","feedEpoch":"e1","reset":false,
                   "changes":[{"kind":"transactions","id":"t1","op":"upsert"},
                              {"kind":"accounts","id":"a1","op":"left"}]}"#,
            ))
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/api/v1/desktop/maintenance"))
            .respond_with(json(
                409,
                r#"{"error":{"code":"MAINTENANCE_BUSY","message":"busy"}}"#,
            ))
            .mount(&server)
            .await;
        let s =
            session_with_caps(&server, &[Capabilities::CHANGES, Capabilities::MAINTENANCE]).await;
        let page = s.changes(Some("8")).await.expect("page");
        assert_eq!(page.cursor.as_deref(), Some("9"));
        assert_eq!(page.changes[0].kind, "transactions");
        assert_eq!(page.changes[0].op, ChangeOp::Upsert);
        assert_eq!(page.changes[1].op, ChangeOp::Left);
        let g = requests(&server, "GET").await;
        assert!(g.iter().any(|r| query(r) == "cursor=8"));
        assert!(matches!(
            s.start_maintenance(false).await,
            Err(ApiError::Lan(crate::LanRefusal::MaintenanceBusy))
        ));
        // Never retried.
        assert_eq!(requests(&server, "POST").await.len(), 1);
    }

    /// PR-8 S6: a routine hold asks the engine to keep the other sessions;
    /// a restore's hold sends nothing extra (they end).
    #[tokio::test]
    async fn a_routine_hold_asks_to_keep_the_other_sessions() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api/v1/desktop/maintenance"))
            .respond_with(json(
                200,
                r#"{"active":true,"holderSession":"c","holderUser":"u","startedAt":"x"}"#,
            ))
            .mount(&server)
            .await;
        let s = session_with_caps(&server, &[Capabilities::MAINTENANCE]).await;
        s.start_maintenance(true).await.expect("routine");
        s.start_maintenance(false).await.expect("restore");
        let posts = requests(&server, "POST").await;
        let body = |i: usize| String::from_utf8(posts[i].body.clone()).expect("utf8");
        assert_eq!(body(0), r#"{"keepSessions":true}"#);
        assert_eq!(body(1), "{}");
    }

    /// PR-8 S7: the hold follows the engine's own LAN flag, not whether
    /// this app's sharing started; never on a workstation's link.
    #[tokio::test]
    async fn the_engine_lan_flag_decides_the_hold_on_the_host_only() {
        use crate::endpoint::{EndpointSource, EndpointState, FixedEndpoint};
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api/v1/desktop/info"))
            .respond_with(json(
                200,
                r#"{"initialized":true,"lan":true,"capabilities":["maintenance-v1"]}"#,
            ))
            .mount(&server)
            .await;
        let s = crate::resources::testkit::company_session(&server);
        assert!(!s.client().lan_host_engine(), "unknown before the info read");
        s.client().desktop_info().await.expect("info");
        assert!(s.client().lan_host_engine());
        // The same engine seen through a workstation's link (saves never wait).
        struct NoWait(std::sync::Arc<FixedEndpoint>);
        impl EndpointSource for NoWait {
            fn current(&self) -> EndpointState {
                self.0.current()
            }
            fn writes_wait(&self) -> bool {
                false
            }
        }
        let port = server.address().port();
        let fixed = std::sync::Arc::new(FixedEndpoint::new(
            port,
            cl_core::Secret::new("t".repeat(64)),
            1,
        ));
        let ws = crate::session::tests::session_with(
            std::sync::Arc::new(NoWait(fixed)),
            crate::session::Scope::Workspace,
        );
        ws.client().desktop_info().await.expect("info");
        assert!(!ws.client().lan_host_engine());
    }
}
