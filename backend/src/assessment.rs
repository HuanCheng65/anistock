//! 风险测评 and the 融资融券 knowledge test (docs/v2-account.md §5, §6). The texts live here so the
//! server can score them; the web page renders what `GET /api/v1/assessments/{kind}` sends.

/// One question: the text and its options. For the risk questionnaire option i scores i + 1.
pub struct Question {
    pub text: &'static str,
    pub options: &'static [&'static str],
    /// knowledge test only: the index of the right option and why
    pub answer: usize,
    pub explanation: &'static str,
}

const fn q(text: &'static str, options: &'static [&'static str]) -> Question {
    Question { text, options, answer: 0, explanation: "" }
}
const fn k(text: &'static str, options: &'static [&'static str], answer: usize, explanation: &'static str) -> Question {
    Question { text, options, answer, explanation }
}

pub const RISK_VERSION: i64 = 1;

pub const RISK: &[Question] = &[
    q("你参与股票交易或模拟交易的经验", &["没有", "不到 1 年", "1–3 年", "3 年以上"]),
    q("你对安易交易规则的了解程度", &["刚开始了解", "了解买卖和分红", "了解冷静期、新股认购等规则", "了解融资融券"]),
    q("你在安易的主要目标", &["获得稳定的分红", "兼顾分红和价格收益", "主要获得价格收益", "争取排行靠前"]),
    q("你通常计划持有一支股票多长时间", &["一季以上", "一季左右", "几周", "几天以内"]),
    q("持有的股票一周内下跌 20%，你通常会怎么做", &["全部卖出", "卖出一部分", "继续持有", "买入更多"]),
    q("持有的作品口碑明显下滑时，你通常会怎么做", &["尽快卖出", "再观察几集", "看股价是否已经反映", "趁下跌买入"]),
    q("你能接受的最大亏损", &["10% 以内", "10%–30%", "30%–50%", "50% 以上"]),
    q("你对融资买入（借入资金买入股票）的态度", &["不考虑", "了解后可能少量使用", "有把握时会使用", "会经常使用"]),
];

/// A risk level: `C1`..`C5`.
pub struct Level {
    pub level: i64,
    pub name: &'static str,
    pub description: &'static str,
    /// the result card's subtitle (docs/v2-account.md §5.2, a personality line: pending approval)
    pub motto: &'static str,
}

pub const LEVELS: [Level; 5] = [
    Level { level: 1, name: "保守型", description: "优先保证本金，能接受较小的价格波动", motto: "完结之后再看" },
    Level { level: 2, name: "稳健型", description: "以分红为主要收益，能接受短期的价格波动", motto: "看过口碑再追" },
    Level { level: 3, name: "平衡型", description: "兼顾分红和价格收益，能接受一定的本金亏损", motto: "先看三集再决定" },
    Level { level: 4, name: "积极型", description: "追求较高收益，能接受较大的本金亏损", motto: "首播当天就追" },
    Level { level: 5, name: "激进型", description: "追求高收益，能接受本金大部分亏损，可以使用杠杆", motto: "看完预告就开始追" },
];

/// The score (8–32) and level of a set of answers (option indexes), or `None` when malformed.
pub fn score_risk(answers: &[i64]) -> Option<(i64, &'static Level)> {
    if answers.len() != RISK.len() {
        return None;
    }
    let mut score = 0;
    for (a, qn) in answers.iter().zip(RISK) {
        if *a < 0 || *a as usize >= qn.options.len() {
            return None;
        }
        score += a + 1;
    }
    let level = match score {
        ..=12 => 0,
        13..=17 => 1,
        18..=22 => 2,
        23..=27 => 3,
        _ => 4,
    };
    Some((score, &LEVELS[level]))
}

pub const MARGIN_QUIZ_VERSION: i64 = 1;
pub const MARGIN_QUIZ_PASS: i64 = 8;

pub const MARGIN_QUIZ: &[Question] = &[
    k(
        "可用保证金为 10,000 星辉时，最多可以融资买入多少",
        &["5,000 星辉", "10,000 星辉", "20,000 星辉", "没有上限"],
        1,
        "融资保证金比例是 100%：新增 1 星辉融资，需要 1 星辉可用保证金。",
    ),
    k(
        "维持担保比例怎么计算",
        &["（现金 + 持仓市值）÷ 负债", "负债 ÷ 净资产", "持仓市值 ÷ 初始资金"],
        0,
        "维持担保比例 =（现金 + 持仓市值）÷ 负债，按最新价计算，持仓不打折。",
    ),
    k(
        "日结时维持担保比例低于 130%，会怎样",
        &["立即强制平仓", "收到追加担保物通知，下一次日结时仍低于 130% 则强制平仓", "没有影响"],
        1,
        "低于 130% 时发出追保通知。到下一次日结仍低于 130%，转入强制平仓，直到回到 150% 以上或负债清零。",
    ),
    k(
        "盘中维持担保比例低于多少时，立即强制平仓",
        &["150%", "130%", "110%"],
        2,
        "盘中低于 110%，或净资产为负时，立即强制平仓。",
    ),
    k(
        "融资利率是多少",
        &["固定 3%", "政策利率 + 3%", "政策利率 + 5%"],
        1,
        "融资利率 = 政策利率 + 3%（年化），每天日结时计息。政策利率上调，融资成本随之上升。",
    ),
    k(
        "融券卖出的价格有什么限制",
        &["不能低于最新成交价，只能用限价委托", "可以用市价委托", "不能高于最新成交价"],
        0,
        "融券卖出只能用限价委托，价格不能低于最新成交价。",
    ),
    k(
        "融券卖出所得的资金可以用来做什么",
        &["买入其他股票", "认购新股", "只能用于买券还券"],
        2,
        "融券卖出所得冻结，只能用于买券还券；还清后剩余部分解冻。",
    ),
    k(
        "欠券期间这支股票分红，会怎样",
        &["不受影响", "按欠券数量 × 每股分红补偿给出借人", "由出借人承担"],
        1,
        "这是分红补偿：借来卖掉的股票在买方手里，买方领分红，借券人另外补偿出借人。",
    ),
    k(
        "持仓卖完后仍然资不抵债（穿仓），排行榜的累计收益率会怎样",
        &["申请重整后从 0 重新计算", "此后一直记为 −100%", "不受影响"],
        1,
        "穿仓后可以申请破产重整继续游戏，但本段收益按净资产为 0 计算，累计收益率此后一直是 −100%。",
    ),
    k(
        "融资借入的资金可以用于认购新股吗",
        &["可以", "不可以"],
        1,
        "新股认购只能用自己的可用资金，融资借款和冻结的融券卖出所得都不能用。",
    ),
];

pub const FUTURES_QUIZ_VERSION: i64 = 1;
pub const FUTURES_QUIZ_PASS: i64 = 4;

/// 热度期货知识测试 (docs/v3-futures-ui.md §4): the settlement is an average, daily marking to market, what a
/// point is worth, margin and liquidation.
pub const FUTURES_QUIZ: &[Question] = &[
    k(
        "热度期货到期时按什么结算",
        &["到期那天的市场预期", "合约期 28 天热度指数的平均值", "最后一天的热度指数"],
        1,
        "到期按合约期内 28 天公布的热度指数的平均值结算。每天公布一个值，越接近到期，能猜的部分越少。",
    ),
    k(
        "以 90.00 押更热 2 手，最终平均热度是 95.00，赚多少",
        &["50 星辉", "950 星辉", "100 星辉"],
        2,
        "每手每 1 点 10 星辉：（95 − 90）× 2 手 × 10 = 100 星辉。",
    ),
    k(
        "还没到期，期货的盈亏什么时候计入账户",
        &["每天日结时按结算价划转", "到期时一次结清", "平仓时才结算"],
        0,
        "每天日结按当天的结算价算一次盈亏并划转（盯市），所以持仓期间账户每天都会变化。",
    ),
    k(
        "期货亏损使信用账户的维持担保比例低于追保线，会怎样",
        &["没有影响，到期才算", "和融资一样收到追保通知，仍不足时强制平仓，先平期货", "合约自动作废"],
        1,
        "期货用信用账户的保证金交易，亏损计入维持担保比例。追保、强制平仓的规则和融资融券相同，强制平仓时先平期货。",
    ),
    k(
        "开仓 1 手需要准备多少资金",
        &["合约的全部名义价值", "不需要任何资金", "名义价值的一部分作为保证金，股票也可以充当"],
        2,
        "开仓冻结名义价值约 15% 的初始保证金。保证金来自信用账户，可用资金不够时，持有的股票可以充当（相当于融资）。",
    ),
];

/// A knowledge test: the questions, the version recorded with each result, the score that passes, and the name
/// stored in `quiz_results.quiz`.
pub struct Quiz {
    pub name: &'static str,
    pub version: i64,
    pub pass: i64,
    pub questions: &'static [Question],
}

pub const MARGIN: Quiz = Quiz { name: "margin", version: MARGIN_QUIZ_VERSION, pass: MARGIN_QUIZ_PASS, questions: MARGIN_QUIZ };
pub const FUTURES: Quiz = Quiz { name: "futures", version: FUTURES_QUIZ_VERSION, pass: FUTURES_QUIZ_PASS, questions: FUTURES_QUIZ };

/// Correct answers, or `None` when malformed.
pub fn score_quiz(quiz: &Quiz, answers: &[i64]) -> Option<i64> {
    if answers.len() != quiz.questions.len() {
        return None;
    }
    let mut right = 0;
    for (a, qn) in answers.iter().zip(quiz.questions) {
        if *a < 0 || *a as usize >= qn.options.len() {
            return None;
        }
        right += i64::from(*a as usize == qn.answer);
    }
    Some(right)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn risk_scores_map_to_levels() {
        assert_eq!(score_risk(&[0; 8]).map(|(s, l)| (s, l.level)), Some((8, 1)));
        assert_eq!(score_risk(&[3; 8]).map(|(s, l)| (s, l.level)), Some((32, 5)));
        assert_eq!(score_risk(&[2, 2, 2, 2, 1, 1, 1, 1]).map(|(s, l)| (s, l.level)), Some((20, 3)));
        assert_eq!(score_risk(&[2, 2, 2, 2, 2, 2, 2, 2]).map(|(s, l)| (s, l.level)), Some((24, 4)));
        assert!(score_risk(&[0; 7]).is_none());
        assert!(score_risk(&[4, 0, 0, 0, 0, 0, 0, 0]).is_none());
    }

    #[test]
    fn quiz_answers_are_in_range() {
        for quiz in [&MARGIN, &FUTURES] {
            for qn in quiz.questions {
                assert!(qn.answer < qn.options.len(), "{}", qn.text);
            }
            assert!(quiz.pass <= quiz.questions.len() as i64);
        }
        let right: Vec<i64> = MARGIN_QUIZ.iter().map(|q| q.answer as i64).collect();
        assert_eq!(score_quiz(&MARGIN, &right), Some(10));
        assert_eq!(score_quiz(&MARGIN, &[0; 10]), Some(MARGIN_QUIZ.iter().filter(|q| q.answer == 0).count() as i64));
    }
}
