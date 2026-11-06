//! The one error shape of API v1 (docs/api-v1.md §2.9): `{"code", "message", "details"}`. `message` is
//! the Chinese text a client shows as is (docs/copywriting.md); `code` is what clients branch on.

use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde_json::{json, Value};
use utoipa::ToSchema;

/// Every non-2xx response body.
#[derive(Serialize, ToSchema, Debug, Clone)]
pub struct ErrorBody {
    /// stable machine-readable code (docs/api-v1.md §2.9)
    pub code: String,
    /// text to show the player as is
    pub message: String,
    /// structure depends on `code`; `{}` when there is nothing
    #[schema(value_type = Object)]
    pub details: Value,
}

/// One rejected field of an `invalid_request`.
#[derive(Serialize, ToSchema, Debug, Clone)]
pub struct FieldError {
    pub field: String,
    /// missing | wrong_type | too_small | too_large | too_short | too_long | not_allowed | bad_format
    pub reason: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct E {
    pub status: u16,
    pub code: &'static str,
    pub message: String,
    pub details: Value,
    /// `Retry-After` (429) / `Allow` (405)
    pub header: Option<(&'static str, String)>,
}

pub type R<T> = Result<T, E>;

impl E {
    pub fn new(status: u16, code: &'static str, message: impl Into<String>) -> E {
        E { status, code, message: message.into(), details: json!({}), header: None }
    }
    pub fn details(mut self, d: Value) -> E {
        self.details = d;
        self
    }
    pub fn body(&self) -> ErrorBody {
        ErrorBody { code: self.code.into(), message: self.message.clone(), details: self.details.clone() }
    }
}

impl IntoResponse for E {
    fn into_response(self) -> Response {
        if self.status >= 500 {
            eprintln!("v1 {} {}: {}", self.status, self.code, self.message);
        }
        let status = StatusCode::from_u16(self.status).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        let mut r = (status, axum::Json(self.body())).into_response();
        if let Some((k, v)) = &self.header {
            if let Ok(v) = HeaderValue::from_str(v) {
                r.headers_mut().insert(*k, v);
            }
        }
        if self.status == 401 || self.status == 403 || self.status >= 500 {
            r.headers_mut().insert("cache-control", HeaderValue::from_static("no-store"));
        }
        r
    }
}

// ---------------------------------------------------------------- the catalogue

pub fn invalid_json() -> E {
    E::new(400, "invalid_json", "请求内容不是有效的 JSON")
}
pub fn unauthenticated() -> E {
    E::new(401, "unauthenticated", "登录后可用")
}
pub fn bad_token() -> E {
    E::new(401, "unauthenticated", "机器人令牌无效")
}
pub fn bad_credentials() -> E {
    E::new(401, "bad_credentials", "用户名或密码不正确")
}
pub fn forbidden(scope: &str) -> E {
    E::new(403, "forbidden", "没有权限").details(json!({"required_scope": scope}))
}
pub fn not_bound() -> E {
    E::new(403, "not_bound", "这个 QQ 号还没有绑定账号")
}
pub fn not_found() -> E {
    E::new(404, "not_found", "没有这个接口")
}
pub fn stock_not_found(id: i64) -> E {
    E::new(404, "stock_not_found", "股票不存在").details(json!({"id": id}))
}
pub fn order_not_found(id: i64) -> E {
    E::new(404, "order_not_found", "委托不存在").details(json!({"id": id}))
}
pub fn ipo_not_found(id: i64) -> E {
    E::new(404, "ipo_not_found", "新股不存在").details(json!({"id": id}))
}
pub fn news_not_found(id: i64) -> E {
    E::new(404, "news_not_found", "资讯不存在").details(json!({"id": id}))
}
pub fn cover_not_found() -> E {
    E::new(404, "cover_not_found", "没有封面")
}
pub fn binding_not_found() -> E {
    E::new(404, "binding_not_found", "绑定不存在")
}
pub fn method_not_allowed(allow: &str) -> E {
    let mut e = E::new(405, "method_not_allowed", "不支持这个操作").details(json!({"allow": allow}));
    if !allow.is_empty() {
        e.header = Some(("allow", allow.to_string()));
    }
    e
}
pub fn username_taken() -> E {
    E::new(409, "username_taken", "用户名已有人使用")
}
pub fn order_finished(status: &str) -> E {
    E::new(409, "order_finished", "委托已结束，无法撤单").details(json!({"status": status}))
}
pub fn already_bound(side: &str) -> E {
    let msg = if side == "qq" { "这个 QQ 号已经绑定了账号" } else { "这个账号已经绑定了 QQ 号，先解绑再绑定" };
    E::new(409, "already_bound", msg).details(json!({"side": side}))
}
pub fn bad_invite() -> E {
    E::new(422, "bad_invite", "邀请码不正确，可以向社团管理员索取")
}
pub fn bind_code_invalid() -> E {
    E::new(422, "bind_code_invalid", "绑定码不正确或已过期")
}
pub fn verification_required(what: &str) -> E {
    E::new(403, "verification_required", format!("完成身份验证后才能{what}"))
}
pub fn agreement_required() -> E {
    E::new(403, "agreement_required", "签署风险揭示书后才能交易")
}
pub fn registration_closed() -> E {
    E::new(403, "registration_closed", "暂未开放注册")
}
pub fn pow_required() -> E {
    E::new(422, "pow_required", "需要先完成验证，重新提交即可")
}
pub fn pow_invalid() -> E {
    E::new(422, "pow_invalid", "验证已失效，重新提交即可")
}
pub fn busy() -> E {
    let mut e = E::new(503, "busy", "当前登录的人较多，稍后重试").details(json!({"retry_after": 5}));
    e.header = Some(("retry-after", "5".into()));
    e
}
pub fn rate_limited(retry_after: u64) -> E {
    let mut e = E::new(429, "rate_limited", "操作太频繁，稍后重试").details(json!({"retry_after": retry_after}));
    e.header = Some(("retry-after", retry_after.to_string()));
    e
}
pub fn internal(why: impl std::fmt::Display) -> E {
    eprintln!("v1 internal: {why}");
    E::new(500, "internal", "服务器暂时无法响应，稍后重试")
}
pub fn unavailable() -> E {
    E::new(503, "unavailable", "服务器正在维护，稍后重试")
}
pub fn sql(e: rusqlite::Error) -> E {
    internal(format!("sqlite: {e}"))
}

/// 400 `invalid_request` for these fields; the top-level message is the first field's.
pub fn invalid(fields: Vec<FieldError>) -> E {
    let message = fields.first().map(|f| f.message.clone()).unwrap_or_else(|| "请求参数不正确".into());
    E::new(400, "invalid_request", message).details(json!({"fields": fields}))
}

/// The player-facing name of a request field.
pub fn label(field: &str) -> &'static str {
    match field {
        "username" => "用户名",
        "password" => "密码",
        "invite" => "邀请码",
        "pow" => "验证",
        "doc" => "文件",
        "version" => "版本",
        "signature" => "签名",
        "quantity" => "数量",
        "price" => "价格",
        "stock_id" | "id" => "股票",
        "side" => "买卖方向",
        "type" => "委托类型",
        "q" => "搜索内容",
        "code" => "绑定码",
        "confirm" => "确认",
        "answers" => "答案",
        "current_password" => "当前密码",
        "new_password" => "新密码",
        "scopes" => "权限",
        "platform" => "平台",
        "external_id" => "QQ 号",
        "trading_day" | "from" | "to" | "listed_since" => "交易日",
        "limit" => "条数",
        "before" | "after" => "翻页位置",
        "status" => "状态",
        "category" => "分类",
        "period" => "排行周期",
        "depth" => "档位数",
        "ids" => "股票列表",
        "reason" => "流水类型",
        "interval" => "周期",
        _ => "参数",
    }
}

pub fn field(field: &str, reason: &str, message: impl Into<String>) -> FieldError {
    FieldError { field: field.into(), reason: reason.into(), message: message.into() }
}

pub fn missing(f: &str) -> FieldError {
    field(f, "missing", format!("缺少{}", label(f)))
}
pub fn wrong_type(f: &str) -> FieldError {
    field(f, "wrong_type", format!("{}格式不正确", label(f)))
}
pub fn bad_format(f: &str) -> FieldError {
    field(f, "bad_format", format!("{}格式不正确", label(f)))
}
pub fn not_allowed(f: &str) -> FieldError {
    field(f, "not_allowed", format!("{}不正确", label(f)))
}
pub fn too_small(f: &str, min: i64) -> FieldError {
    let m = if min == 1 { format!("{}需大于 0", label(f)) } else { format!("{}不能小于 {min}", label(f)) };
    field(f, "too_small", m)
}
pub fn too_large(f: &str, max: i64) -> FieldError {
    field(f, "too_large", format!("{}不能超过 {}", label(f), group(max)))
}

/// 10000000 -> "10,000,000"
pub fn group(n: i64) -> String {
    let s = n.unsigned_abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// cents -> "4,409.00"
pub fn coins(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let a = cents.unsigned_abs();
    format!("{sign}{}.{:02}", group((a / 100) as i64), a % 100)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grouping() {
        assert_eq!(group(10000000), "10,000,000");
        assert_eq!(group(999), "999");
        assert_eq!(coins(440900), "4,409.00");
        assert_eq!(coins(5), "0.05");
    }
}
