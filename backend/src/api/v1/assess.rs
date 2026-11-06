//! 风险测评 and the 融资融券 knowledge test (docs/v2-account.md §5, §6): the questions, the answers,
//! and what they unlock (opening credit needs C4 or above, a passed test and the margin disclosure).

use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::{Extension, Path, State};
use axum::http::HeaderMap;
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};

use super::error::{self, sql, ErrorBody, E, R};
use super::types::*;
use super::{json_body, read, Caller};
use crate::api::AppState;
use crate::assessment as a;

fn kind_of(raw: &str) -> R<&'static str> {
    match raw {
        "risk" => Ok("risk"),
        "margin-quiz" => Ok("margin-quiz"),
        _ => Err(E::new(404, "not_found", "没有这项测评")),
    }
}

/// 测评题目
#[utoipa::path(get, path = "/api/v1/assessments/{kind}", tag = "account",
    params(("kind" = String, Path, description = "risk（风险测评）或 margin-quiz（融资融券知识测试）")),
    responses((status = 200, body = Assessment), (status = "default", body = ErrorBody)))]
pub async fn questions(Path(raw): Path<String>) -> R<Json<Assessment>> {
    let kind = kind_of(&raw)?;
    let (version, list, pass) = match kind {
        "risk" => (a::RISK_VERSION, a::RISK, None),
        _ => (a::MARGIN_QUIZ_VERSION, a::MARGIN_QUIZ, Some(a::MARGIN_QUIZ_PASS)),
    };
    Ok(Json(Assessment {
        kind: kind.into(),
        version,
        pass_score: pass,
        questions: list
            .iter()
            .map(|q| AssessmentQuestion { text: q.text.into(), options: q.options.iter().map(|o| o.to_string()).collect() })
            .collect(),
    }))
}

fn answers(b: &serde_json::Map<String, serde_json::Value>) -> R<Vec<i64>> {
    b.get("answers")
        .and_then(|v| v.as_array())
        .and_then(|xs| xs.iter().map(|x| x.as_i64()).collect::<Option<Vec<_>>>())
        .ok_or_else(|| error::invalid(vec![error::wrong_type("answers")]))
}

fn taken_today(c: &Connection, table: &str, id: i64, day: &str) -> R<i64> {
    c.query_row(&format!("SELECT COUNT(*) FROM {table} WHERE account_id=? AND day=?"), params![id, day], |r| r.get(0))
        .map_err(sql)
}

/// The latest 风险测评 of `id`, valid or not.
pub fn risk_of(st: &AppState, c: &Connection, id: i64) -> R<Option<RiskResult>> {
    let row: Option<(i64, i64, String)> = c
        .query_row("SELECT level, score, taken_at FROM risk_assessments WHERE account_id=? ORDER BY id DESC LIMIT 1", [id], |r| {
            Ok((r.get(0)?, r.get(1)?, r.get(2)?))
        })
        .optional()
        .map_err(sql)?;
    Ok(row.map(|(level, score, taken_at)| {
        let l = &a::LEVELS[(level.clamp(1, 5) - 1) as usize];
        let days = st.cfg.eligibility.assessment_valid_days;
        let valid = crate::kyc::days_since(&taken_at, &st.clock.today()) < days;
        let until = chrono::NaiveDate::parse_from_str(&taken_at[..10.min(taken_at.len())], "%Y-%m-%d")
            .map(|d| (d + chrono::Duration::days(days)).format("%Y-%m-%d").to_string())
            .unwrap_or_default();
        RiskResult {
            level: format!("C{level}"),
            name: l.name.into(),
            description: l.description.into(),
            motto: l.motto.into(),
            score,
            taken_at: ts(taken_at),
            valid_until: day(until),
            valid,
        }
    }))
}

/// What opening credit still needs (on top of the engine's own checks: days and net assets).
/// 身份验证 stands in for a waiting period against throwaway accounts (docs/v2-account.md §6).
pub fn credit_steps(st: &AppState, c: &Connection, id: i64) -> R<CreditSteps> {
    let verified = !st.cfg.eligibility.require_kyc || crate::kyc::method(c, id).map_err(sql)?.is_some();
    let risk = risk_of(st, c, id)?;
    let min = st.cfg.eligibility.credit_min_level;
    let risk_ok = risk.as_ref().is_some_and(|r| r.valid && r.level[1..].parse::<i64>().unwrap_or(0) >= min);
    let quiz_passed: bool = c
        .query_row("SELECT EXISTS(SELECT 1 FROM quiz_results WHERE account_id=? AND quiz='margin' AND passed=1 AND version=?)", params![id, a::MARGIN_QUIZ_VERSION], |r| r.get(0))
        .map_err(sql)?;
    let disclosure_signed: bool = c
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM account_agreements WHERE account_id=? AND doc='margin_disclosure' AND version=?)",
            params![id, st.cfg.margin_disclosure],
            |r| r.get(0),
        )
        .map_err(sql)?;
    Ok(CreditSteps { verified, risk_ok, min_level: format!("C{min}"), quiz_passed, disclosure_signed, disclosure_version: st.cfg.margin_disclosure })
}

/// The first unmet step, as a `credit_not_eligible` error.
pub fn credit_refusal(s: &CreditSteps) -> Option<E> {
    let msg = if !s.verified {
        "完成身份验证后才能开通".to_string()
    } else if !s.risk_ok {
        format!("风险测评为 {} 及以上才能开通", s.min_level)
    } else if !s.quiz_passed {
        "通过融资融券知识测试后才能开通".to_string()
    } else if !s.disclosure_signed {
        "签署《融资融券风险揭示书》后才能开通".to_string()
    } else {
        return None;
    };
    Some(E::new(422, "credit_not_eligible", msg))
}

/// 提交风险测评或知识测试
#[utoipa::path(post, path = "/api/v1/me/assessments/{kind}", tag = "account",
    params(("kind" = String, Path, description = "risk 或 margin-quiz")),
    request_body = AnswersRequest,
    responses((status = 200, body = AssessmentResult), (status = "default", body = ErrorBody)))]
pub async fn submit(
    State(st): State<Arc<AppState>>,
    Extension(caller): Extension<Caller>,
    Path(raw): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> R<Json<AssessmentResult>> {
    let id = caller.player()?;
    let kind = kind_of(&raw)?;
    let b = json_body(&headers, &body)?;
    let ans = answers(&b)?;
    let (today, now) = (st.clock.today(), st.clock.ts());
    let per_day = st.cfg.eligibility.assessment_per_day;
    let table = if kind == "risk" { "risk_assessments" } else { "quiz_results" };
    let done = read(&st, move |c| taken_today(c, table, id, &today)).await?;
    if done >= per_day {
        return Err(E::new(422, "assessment_limit", format!("每天最多测评 {per_day} 次，明天再试")));
    }
    let json = serde_json::to_string(&ans).map_err(error::internal)?;
    let day = st.clock.today();
    if kind == "risk" {
        let (score, level) = a::score_risk(&ans).ok_or_else(|| error::invalid(vec![error::wrong_type("answers")]))?;
        super::bind::side_exec(
            &st,
            "INSERT INTO risk_assessments(account_id, version, answers, score, level, taken_at, day) VALUES (?,?,?,?,?,?,?)",
            params![id, a::RISK_VERSION, json, score, level.level, now, day],
        )?;
        let st2 = st.clone();
        let risk = read(&st, move |c| risk_of(&st2, c, id)).await?;
        return Ok(Json(AssessmentResult { risk, quiz: None }));
    }
    let right = a::score_quiz(&ans).ok_or_else(|| error::invalid(vec![error::wrong_type("answers")]))?;
    let passed = right >= a::MARGIN_QUIZ_PASS;
    super::bind::side_exec(
        &st,
        "INSERT INTO quiz_results(account_id, quiz, version, score, passed, taken_at, day) VALUES (?,?,?,?,?,?,?)",
        params![id, "margin", a::MARGIN_QUIZ_VERSION, right, passed, now, day],
    )?;
    let review = a::MARGIN_QUIZ
        .iter()
        .zip(&ans)
        .map(|(q, given)| QuizReview { correct: *given as usize == q.answer, answer: q.answer as i64, explanation: q.explanation.into() })
        .collect();
    Ok(Json(AssessmentResult {
        risk: None,
        quiz: Some(QuizResult { score: right, total: a::MARGIN_QUIZ.len() as i64, pass_score: a::MARGIN_QUIZ_PASS, passed, review }),
    }))
}
