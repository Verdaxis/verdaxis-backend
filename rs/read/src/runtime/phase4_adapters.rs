//! Production adapters borrow the caller's transaction and never dispatch jobs.
use std::sync::Arc;
use chrono::{DateTime, Utc};
use serde_json::json;
use sqlx::{PgConnection, Row};
use uuid::Uuid;
use crate::{AppState, business::{admission, approval_mail, profile, transaction::{AttemptError, CommandFuture}}, kyc::{grace::Grace, policy::{self, KycParty, KycPolicy, KycStatus}}};

#[derive(Clone)]
pub struct Events(pub AppState);
impl admission::AdmissionEvents for Events {
    fn visible<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid) -> CommandFuture<'a, bool, AttemptError> {
        Box::pin(async move {
            let query = crate::market::visibility::order_query_sql("SELECT EXISTS(SELECT 1 FROM eligible_orders WHERE owner_user_id=$14)");
            let clock = crate::market::visibility::VisibilityClock::at(Utc::now());
            let row = crate::market::visibility::bind_order_query(&query, &self.0.policy, &clock, &self.0.config, &Default::default())?
                .bind(user).fetch_one(conn).await?;
            Ok(row.get(0))
        })
    }
    fn append<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid, organization: Uuid, public: bool) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            let mut payload = json!({"schema_version":1,"user_id":user});
            if public { payload["_public_market_invalidation"] = json!(true); }
            sqlx::query("INSERT INTO market_event_outbox(id,event_type,aggregate_type,aggregate_id,participant_org_ids,payload) VALUES($1,'market_admission_changed','user',$2,$3,$4)")
                .bind(Uuid::new_v4()).bind(user.to_string()).bind(json!([organization])).bind(payload).execute(conn).await?;
            Ok(())
        })
    }
}
impl profile::ProfileMarketEvents for Events {
    fn visible<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid) -> CommandFuture<'a, bool, AttemptError> { admission::AdmissionEvents::visible(self, conn, user) }
    fn append<'a>(&'a self, conn: &'a mut PgConnection, user: Uuid, organization: Uuid, public: bool) -> CommandFuture<'a, (), AttemptError> { admission::AdmissionEvents::append(self, conn, user, organization, public) }
}

pub struct ProfilePolicy;
impl profile::KycProfilePolicy for ProfilePolicy {
    fn eligibility<'a>(&'a self, conn: &'a mut PgConnection, party: profile::ProfileKycParty, execution_gate_enabled: bool, now: DateTime<Utc>) -> CommandFuture<'a, profile::KycEligibilityView, sqlx::Error> {
        Box::pin(async move {
            let status = match party.kyc_status.as_str() { "APPROVED"=>KycStatus::Approved,"REJECTED"=>KycStatus::Rejected,"SUBMITTED"=>KycStatus::Submitted,_=>KycStatus::Pending };
            let grace: Option<(Uuid,Uuid,DateTime<Utc>)> = sqlx::query_as("SELECT user_id,organization_id,expires_at FROM kyc_execution_grace WHERE user_id=$1 AND organization_id=$2")
                .bind(party.user_id).bind(party.organization_id).fetch_optional(conn).await?;
            let grace = grace.map(|(user_id,organization_id,expires_at)| Grace { user_id,organization_id,expires_at });
            let result = policy::eligibility(&KycPolicy { execution_gate_enabled }, &KycParty { user_id:party.user_id,organization_id:party.organization_id,kyc_organization_id:party.kyc_organization_id,kyc_status:status }, grace.as_ref(), now);
            let reason = match result.reason { policy::KycReason::NotEnforced=>profile::KycReason::NotEnforced,policy::KycReason::Approved=>profile::KycReason::Approved,policy::KycReason::Rejected=>profile::KycReason::Rejected,policy::KycReason::Grace=>profile::KycReason::Grace,policy::KycReason::Required=>profile::KycReason::Required };
            Ok(profile::KycEligibilityView { eligible:result.eligible,reason,grace_expires_at:result.grace_expires_at })
        })
    }
}

pub struct FrozenMail;
impl admission::ApprovalMail for FrozenMail {
    fn freeze_approval_mail<'a>(&'a self, conn: &'a mut PgConnection, marker: &'a admission::ApprovalMailMarker) -> CommandFuture<'a, (), AttemptError> {
        Box::pin(async move {
            let payload = &marker.payload;
            let marker = approval_mail::ApprovalMailMarker { user_id:marker.user_id,transition_id:marker.transition_id,retry_at:marker.retry_at,payload:approval_mail::FrozenApprovalPayload { from:payload.from.clone(),to:payload.to.clone(),subject:payload.subject.clone(),html:payload.html.clone(),reply_to:payload.reply_to.clone() } };
            approval_mail::freeze_approval_mail(conn, &marker).await.map_err(|error| match error { approval_mail::MailError::Database(error)=>AttemptError::Database(error),_=>AttemptError::Application(crate::ApiError::internal()) })
        })
    }
}

pub struct Template { pub sender:String, pub frontend:String }
fn escape(value:&str)->String { value.replace('&',"&amp;").replace('<',"&lt;").replace('>',"&gt;").replace('"',"&quot;").replace('\'',"&#39;") }
impl admission::ApprovalTemplate for Template {
    fn payload(&self,email:&str,first_name:Option<&str>)->admission::FrozenApprovalPayload {
        admission::FrozenApprovalPayload { from:self.sender.clone(),to:[email.into()],subject:"Your Verdaxis account has been approved".into(),html:format!("<p>Congratulations, {}. Your account has been approved.</p><p>Complete any remaining onboarding steps, then explore the marketplace.</p><a href=\"{}/app/marketplace\">Explore the marketplace</a>",escape(first_name.unwrap_or("there")),escape(self.frontend.trim_end_matches('/'))),reply_to:Some("admin@verdaxis.exchange".into()) }
    }
}

pub struct Domains(pub Arc<crate::auth::registration::RegistrationSettings>);
impl crate::business::invitations::request::OrganizationDomainPolicy for Domains {
    fn organization_domain(&self,email:&str)->Option<String> { self.0.organization_domain(email) }
}
