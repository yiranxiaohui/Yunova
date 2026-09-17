use axum::{
    Extension, Json, Router,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde::{Deserialize, Serialize};

use crate::{
    AppState, CurrentUser, InstalledState,
    db::{self},
};

#[derive(Serialize, Deserialize, sqlx::FromRow)]
pub struct UpstreamSettings {
    pub protocol: String,
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    #[serde(serialize_with = "ser_bool_int", deserialize_with = "de_bool")]
    pub use_proxy: i64,

    // Image-generation config. Optional: if absent, image mode is disabled
    // for the user regardless of chat protocol.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_base_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_api_key: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image_model: Option<String>,
    #[serde(
        default,
        serialize_with = "ser_opt_bool",
        deserialize_with = "de_opt_bool",
        skip_serializing_if = "Option::is_none"
    )]
    pub image_use_proxy: Option<i64>,
}

fn ser_bool_int<S: serde::Serializer>(v: &i64, s: S) -> Result<S::Ok, S::Error> {
    s.serialize_bool(*v != 0)
}

fn de_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<i64, D::Error> {
    let b = bool::deserialize(d)?;
    Ok(if b { 1 } else { 0 })
}

fn ser_opt_bool<S: serde::Serializer>(v: &Option<i64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(n) => s.serialize_bool(*n != 0),
        None => s.serialize_none(),
    }
}

fn de_opt_bool<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<i64>, D::Error> {
    let b: Option<bool> = Option::deserialize(d)?;
    Ok(b.map(|b| if b { 1 } else { 0 }))
}

fn err(status: StatusCode, msg: impl Into<String>) -> Response {
    (status, msg.into()).into_response()
}

fn validate(s: &UpstreamSettings) -> Result<(), Response> {
    match s.protocol.as_str() {
        "openai" | "claude" | "gemini" => {}
        _ => return Err(err(StatusCode::BAD_REQUEST, "invalid protocol")),
    }
    if s.base_url.len() > 512 {
        return Err(err(StatusCode::BAD_REQUEST, "base_url too long"));
    }
    if s.api_key.len() > 512 {
        return Err(err(StatusCode::BAD_REQUEST, "api_key too long"));
    }
    if s.model.len() > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "model too long"));
    }
    match s.image_protocol.as_deref() {
        None | Some("openai") | Some("gemini") => {}
        _ => return Err(err(StatusCode::BAD_REQUEST, "invalid image_protocol")),
    }
    if s.image_base_url.as_deref().map_or(0, str::len) > 512 {
        return Err(err(StatusCode::BAD_REQUEST, "image_base_url too long"));
    }
    if s.image_api_key.as_deref().map_or(0, str::len) > 512 {
        return Err(err(StatusCode::BAD_REQUEST, "image_api_key too long"));
    }
    if s.image_model.as_deref().map_or(0, str::len) > 128 {
        return Err(err(StatusCode::BAD_REQUEST, "image_model too long"));
    }
    Ok(())
}

async fn get_settings(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(
        installed.kind,
        "SELECT protocol, base_url, api_key, model, use_proxy,
                image_protocol, image_base_url, image_api_key, image_model, image_use_proxy
         FROM user_settings WHERE user_id = ?",
    );
    let row: Result<Option<UpstreamSettings>, _> = sqlx::query_as(&sql)
        .bind(user.id)
        .fetch_optional(&installed.pool)
        .await;
    match row {
        Ok(Some(s)) => Json(s).into_response(),
        Ok(None) => err(StatusCode::NOT_FOUND, "no cloud settings"),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn put_settings(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
    Json(body): Json<UpstreamSettings>,
) -> Response {
    if let Err(r) = validate(&body) {
        return r;
    }
    // Stored as 0/1 integers on both backends, so the wire's bools are kept
    // in the integer domain rather than bound as SQL booleans.
    let use_proxy: i64 = if body.use_proxy != 0 { 1 } else { 0 };
    let image_use_proxy: Option<i64> = body.image_use_proxy.map(|v| if v != 0 { 1 } else { 0 });
    let now = db::now_expr(installed.kind);

    let sql = db::q(
        installed.kind,
        &format!(
            "INSERT INTO user_settings
                 (user_id, protocol, base_url, api_key, model, use_proxy,
                  image_protocol, image_base_url, image_api_key, image_model, image_use_proxy,
                  updated_at)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, {now})
             ON CONFLICT (user_id) DO UPDATE SET
                 protocol = excluded.protocol,
                 base_url = excluded.base_url,
                 api_key = excluded.api_key,
                 model = excluded.model,
                 use_proxy = excluded.use_proxy,
                 image_protocol = excluded.image_protocol,
                 image_base_url = excluded.image_base_url,
                 image_api_key = excluded.image_api_key,
                 image_model = excluded.image_model,
                 image_use_proxy = excluded.image_use_proxy,
                 updated_at = {now}"
        ),
    );

    let r = sqlx::query(&sql)
        .bind(user.id)
        .bind(&body.protocol)
        .bind(&body.base_url)
        .bind(&body.api_key)
        .bind(&body.model)
        .bind(use_proxy)
        .bind(&body.image_protocol)
        .bind(&body.image_base_url)
        .bind(&body.image_api_key)
        .bind(&body.image_model)
        .bind(image_use_proxy)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

async fn delete_settings(
    Extension(installed): Extension<InstalledState>,
    Extension(user): Extension<CurrentUser>,
) -> Response {
    let sql = db::q(installed.kind, "DELETE FROM user_settings WHERE user_id = ?");
    let r = sqlx::query(&sql)
        .bind(user.id)
        .execute(&installed.pool)
        .await;
    match r {
        Ok(_) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => err(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()),
    }
}

pub fn routes() -> Router<AppState> {
    Router::new().route(
        "/settings",
        get(get_settings).put(put_settings).delete(delete_settings),
    )
}
