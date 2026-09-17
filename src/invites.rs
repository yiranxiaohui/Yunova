use axum::{
    Extension, Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use rand::RngCore;
use serde::Serialize;

use crate::{
    AppState, CurrentUser, InstalledState,
    db::{self, DbKind, Pool},
};

// Crockford-ish alphabet — no 0/O/1/I/L to reduce confusion.
const INVITE_ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";
const INVITE_CODE_LEN: usize = 7;
const MAX_GEN_ATTEMPTS: usize = 8;

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn gen_code() -> String {
    let mut bytes = [0u8; INVITE_CODE_LEN];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes
        .iter()
        .map(|b| INVITE_ALPHABET[(*b as usize) % INVITE_ALPHABET.len()] as char)
        .collect()
}

pub fn normalize_code(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Returns the user's invite code, generating one lazily if missing.
pub async fn ensure_code(pool: &Pool, kind: DbKind, user_id: i64) -> Result<String, sqlx::Error> {
    let sel = db::q(kind, "SELECT invite_code FROM users WHERE id = ?");
    let row: Option<(Option<String>,)> = sqlx::query_as(&sel)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    match row {
        Some((Some(c),)) if !c.is_empty() => return Ok(c),
        Some((_,)) => {}
        None => return Err(sqlx::Error::RowNotFound),
    }

    for _ in 0..MAX_GEN_ATTEMPTS {
        let code = gen_code();
        let upd = db::q(
            kind,
            "UPDATE users SET invite_code = ? WHERE id = ? AND (invite_code IS NULL OR invite_code = '')",
        );
        let r = sqlx::query(&upd)
            .bind(&code)
            .bind(user_id)
            .execute(pool)
            .await;
        match r {
            Ok(out) if out.rows_affected() > 0 => return Ok(code),
            Ok(_) => {
                // Someone else wrote a code — fetch it.
                let row: Option<(Option<String>,)> = sqlx::query_as(&sel)
                    .bind(user_id)
                    .fetch_optional(pool)
                    .await?;
                if let Some((Some(existing),)) = row {
                    if !existing.is_empty() {
                        return Ok(existing);
                    }
                }
            }
            Err(sqlx::Error::Database(d)) if d.is_unique_violation() => continue,
            Err(e) => return Err(e),
        }
    }
    Err(sqlx::Error::RowNotFound)
}

/// Look up the owner of an invite code (normalized), rejecting self-invites.
pub async fn resolve_inviter(
    pool: &Pool,
    kind: DbKind,
    code: &str,
    new_user_id: i64,
) -> Option<i64> {
    let normalized = normalize_code(code);
    if normalized.is_empty() {
        return None;
    }
    let sql = db::q(kind, "SELECT id FROM users WHERE invite_code = ?");
    let row: Option<(i64,)> = sqlx::query_as(&sql)
        .bind(&normalized)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten();
    match row {
        Some((id,)) if id != new_user_id => Some(id),
        _ => None,
    }
}

/// Claims the referral: sets `invited_by` only if it was still NULL.
/// Returns `true` only if this call actually claimed the row — callers MUST
/// gate reward grants on this so a replay can't grant the reward twice.
pub async fn set_invited_by(
    pool: &Pool,
    kind: DbKind,
    user_id: i64,
    inviter_id: i64,
) -> Result<bool, sqlx::Error> {
    let sql = db::q(
        kind,
        "UPDATE users SET invited_by = ? WHERE id = ? AND invited_by IS NULL",
    );
    let result = sqlx::query(&sql)
        .bind(inviter_id)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}

// ---------------------------------------------------------------------------
// endpoints
// ---------------------------------------------------------------------------

#[derive(Serialize)]
struct InviteInfo {
    invite_code: String,
    invited_count: i64,
    total_earned: i64,
    grant_inviter: i64,
    grant_invitee: i64,
}

async fn get_my_invite(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let code = match ensure_code(&installed.pool, installed.kind, user.id).await {
        Ok(c) => c,
        Err(e) => return err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    };

    let count_sql = db::q(
        installed.kind,
        "SELECT COUNT(*) FROM users WHERE invited_by = ?",
    );
    let (invited_count,): (i64,) = sqlx::query_as(&count_sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or((0,));

    // Sum of credits earned via invite rewards (positive deltas with reason starting "invite_reward_inviter").
    // Table was `credit_ledger` until migration 38 renamed it to
    // `balance_ledger`; the old name was left here, so this query errored and
    // the `unwrap_or` below silently reported 0 earned to every inviter.
    let sum_sql = db::q(
        installed.kind,
        "SELECT CAST(COALESCE(SUM(delta), 0) AS BIGINT) FROM balance_ledger
         WHERE user_id = ? AND reason LIKE 'invite_reward_inviter%'",
    );
    let (total_earned,): (i64,) = sqlx::query_as(&sum_sql)
        .bind(user.id)
        .fetch_one(&installed.pool)
        .await
        .unwrap_or((0,));

    let grant_inviter = crate::quota::get_setting_i64(
        &installed.pool,
        installed.kind,
        "invite_grant_inviter",
        100,
    )
    .await;
    let grant_invitee = crate::quota::get_setting_i64(
        &installed.pool,
        installed.kind,
        "invite_grant_invitee",
        100,
    )
    .await;

    Json(InviteInfo {
        invite_code: code,
        invited_count,
        total_earned,
        grant_inviter,
        grant_invitee,
    })
    .into_response()
}

#[derive(Serialize)]
struct InvitedUser {
    id: i64,
    username: String,
    created_at: String,
}

async fn list_invited(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT id, username, created_at FROM users
         WHERE invited_by = ?
         ORDER BY created_at DESC, id DESC
         LIMIT 200",
    );
    let rows: Result<Vec<(i64, String, String)>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_all(&installed.pool)
        .await;
    match rows {
        Ok(rs) => {
            let out: Vec<InvitedUser> = rs
                .into_iter()
                .map(|(id, username, created_at)| InvitedUser {
                    id,
                    username,
                    created_at,
                })
                .collect();
            Json(out).into_response()
        }
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/invites/me", get(get_my_invite))
        .route("/invites/mine", get(list_invited))
}
