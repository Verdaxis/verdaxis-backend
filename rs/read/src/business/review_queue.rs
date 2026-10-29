use axum::http::StatusCode;
use serde_json::{Value, json};
use sqlx::{PgConnection, Row};
use uuid::Uuid;

use super::super::error::refusal;
use crate::ApiError;

const MAX_JOIN_ROWS: i64 = 20;

#[derive(Debug, Clone, Copy)]
pub struct ReviewQuery {
    limit: i64,
}

impl ReviewQuery {
    pub fn new(limit: i64) -> Result<Self, ApiError> {
        if !(1..=100).contains(&limit) {
            return Err(refusal(
                StatusCode::UNPROCESSABLE_ENTITY,
                "VALIDATION_FAILED",
                "The review limit must be between 1 and 100.",
            ));
        }
        Ok(Self { limit })
    }
}

pub async fn detail(conn: &mut PgConnection, user: Uuid) -> Result<Value, ApiError> {
    let users = sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('id',id,'email',email,'email_verified',email_verified,'role',role,'status',status,'organization_id',organization_id,'kyc_status',kyc_status,'kyc_organization_id',kyc_organization_id,'kyc_external_evidence_reference',kyc_external_evidence_reference,'kyc_review_note',kyc_review_note,'kyc_reviewed_by',kyc_reviewed_by,'kyc_reviewed_at',kyc_reviewed_at,'created_at',created_at) AS item FROM users u WHERE id=$1 AND role <> 'ADMIN'")
        .bind(user).fetch_all(&mut *conn).await.map_err(|_| ApiError::database())?;
    project(conn, users).await?.pop().ok_or_else(|| {
        refusal(
            StatusCode::NOT_FOUND,
            "REVIEW_CASE_NOT_FOUND",
            "The review case was not found.",
        )
    })
}

pub async fn list(conn: &mut PgConnection, query: ReviewQuery) -> Result<Value, ApiError> {
    let users = sqlx::query_scalar::<_, Value>("SELECT jsonb_build_object('id',id,'email',email,'email_verified',email_verified,'role',role,'status',status,'organization_id',organization_id,'kyc_status',kyc_status,'kyc_organization_id',kyc_organization_id,'kyc_external_evidence_reference',kyc_external_evidence_reference,'kyc_review_note',kyc_review_note,'kyc_reviewed_by',kyc_reviewed_by,'kyc_reviewed_at',kyc_reviewed_at,'created_at',created_at) AS item FROM users u WHERE role <> 'ADMIN' AND (status='PENDING' OR kyc_status IN ('SUBMITTED','REVIEW_REQUIRED','REJECTED') OR organization_id IS NULL) ORDER BY created_at,id LIMIT $1")
        .bind(query.limit).fetch_all(&mut *conn).await.map_err(|_| ApiError::database())?;
    Ok(json!({"items":project(conn, users).await?,"limit":query.limit}))
}

async fn project(conn: &mut PgConnection, users: Vec<Value>) -> Result<Vec<Value>, ApiError> {
    let ids: Vec<Uuid> = users
        .iter()
        .map(|user| {
            user["id"]
                .as_str()
                .and_then(|id| Uuid::parse_str(id).ok())
                .ok_or_else(ApiError::database)
        })
        .collect::<Result<_, _>>()?;
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let joins = sqlx::query("SELECT user_id,to_jsonb(j) - 'candidate_row' AS item FROM (SELECT r.*,row_number() OVER (PARTITION BY user_id ORDER BY created_at DESC,id) AS candidate_row FROM organization_join_requests r WHERE user_id=ANY($1)) j WHERE candidate_row <= $2 ORDER BY user_id,created_at DESC,id")
        .bind(&ids).bind(MAX_JOIN_ROWS).fetch_all(&mut *conn).await.map_err(|_| ApiError::database())?;
    let mut organization_ids = Vec::new();
    for user in &users {
        for key in ["organization_id", "kyc_organization_id"] {
            if let Some(id) = user[key].as_str() {
                organization_ids.push(Uuid::parse_str(id).map_err(|_| ApiError::database())?);
            }
        }
    }
    for row in &joins {
        let item: Value = row.get("item");
        if let Some(id) = item["organization_id"].as_str() {
            organization_ids.push(Uuid::parse_str(id).map_err(|_| ApiError::database())?);
        }
    }
    organization_ids.sort_unstable();
    organization_ids.dedup();
    let organizations = sqlx::query("SELECT id,jsonb_build_object('id',id,'name',name,'domain',domain,'type',type,'verification_status',verification_status,'tax_id_present',COALESCE(tax_id <> '',false),'country_code',country_code) AS item FROM organizations WHERE id=ANY($1)")
        .bind(&organization_ids).fetch_all(conn).await.map_err(|_| ApiError::database())?;
    let organizations: std::collections::HashMap<String, Value> = organizations
        .into_iter()
        .map(|row| (row.get::<Uuid, _>("id").to_string(), row.get("item")))
        .collect();
    let organization = |id: &Value| {
        id.as_str()
            .and_then(|id| organizations.get(id))
            .cloned()
            .unwrap_or(Value::Null)
    };
    let mut result = Vec::with_capacity(users.len());
    for user in users {
        let mut item = serde_json::Map::new();
        item.insert("user_id".into(), user["id"].clone());
        item.insert("account_status".into(), user["status"].clone());
        for key in [
            "email",
            "email_verified",
            "role",
            "kyc_status",
            "kyc_organization_id",
            "kyc_external_evidence_reference",
            "kyc_review_note",
            "kyc_reviewed_by",
            "kyc_reviewed_at",
            "created_at",
        ] {
            item.insert(key.into(), user[key].clone());
        }
        item.insert(
            "current_organization".into(),
            organization(&user["organization_id"]),
        );
        let requested: Vec<Value> = joins.iter().filter_map(|row| {
            let join:Value=row.get("item");
            if join["user_id"]!=user["id"] { return None; }
            Some(json!({"request_id":join["id"],"status":join["status"],"created_at":join["created_at"],"reviewed_at":join["reviewed_at"],"review_note":join["review_note"],"organization":organization(&join["organization_id"])}))
        }).collect();
        item.insert("requested_organizations".into(), json!(requested));
        result.push(Value::Object(item));
    }
    Ok(result)
}
