//! 已读判定（`docs/api/v1.md` § Read and unread）。
//!
//! 文档给出的优先级必须逐条照做，顺序错了会直接改变列表里的未读数：
//!
//! 1. state 有 `readAt` → 已读；
//! 2. state 有 `markedUnread` → 未读（哪怕比地平线老、哪怕 id 低于水位线）；
//! 3. 文章 id ≤ 订阅的 `watermarkId` → 已读；
//! 4. `fetchedAt` 早于地平线（默认 30 天）→ 已读；
//! 5. 其余 → 未读（包括完全没有 state 的文章）。

use crate::rows::{ArticleRow, StateRow, SubscriptionRow};

/// 一次判定的输入。
#[derive(Debug, Clone, Copy)]
pub struct ReadRuleInput<'a> {
    /// 文章行。
    pub article: &'a ArticleRow,
    /// 该文章的 state（可以没有）。
    pub state: Option<&'a StateRow>,
    /// 该文章所属 feed 的订阅行（没有订阅时为 `None`）。
    pub subscription: Option<&'a SubscriptionRow>,
    /// 服务端的 `limits.horizonDays`。
    pub horizon_days: i64,
    /// 当前时间（epoch 毫秒）。
    pub now_ms: i64,
}

/// 是否已读。
pub fn is_read(input: ReadRuleInput<'_>) -> bool {
    read_rule(input).is_read()
}

/// 命中的规则（便于诊断与测试）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadRuleHit {
    /// 规则 1：有 `readAt`。
    ReadAt,
    /// 规则 2：手工标未读。
    MarkedUnread,
    /// 规则 3：id ≤ 水位线。
    Watermark,
    /// 规则 4：早于地平线。
    Horizon,
    /// 规则 5：默认未读。
    Default,
}

impl ReadRuleHit {
    /// 该规则是否判为已读。
    pub fn is_read(self) -> bool {
        matches!(self, Self::ReadAt | Self::Watermark | Self::Horizon)
    }
}

/// 命中哪一条规则。
pub fn read_rule(input: ReadRuleInput<'_>) -> ReadRuleHit {
    if input.state.and_then(|state| state.read_at).is_some() {
        return ReadRuleHit::ReadAt;
    }
    if input.state.is_some_and(|state| state.marked_unread) {
        return ReadRuleHit::MarkedUnread;
    }
    if let Some(subscription) = input.subscription
        && input.article.id <= subscription.watermark_id
    {
        return ReadRuleHit::Watermark;
    }
    let horizon_ms = input
        .horizon_days
        .max(0)
        .saturating_mul(24 * 60 * 60 * 1000);
    if input.now_ms.saturating_sub(input.article.fetched_at) > horizon_ms {
        return ReadRuleHit::Horizon;
    }
    ReadRuleHit::Default
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_790_600_000_000;
    const DAY: i64 = 24 * 60 * 60 * 1000;

    fn article(id: i64, fetched_at: i64) -> ArticleRow {
        ArticleRow {
            id,
            feed_id: 1,
            url: None,
            title: "t".into(),
            author: None,
            published_at: None,
            fetched_at,
            sort_at: fetched_at,
            source_lang: None,
            excerpt: None,
            content_key: None,
            word_count: 0,
            reading_minutes: 0,
            like_count: 0,
            recommend_count: 0,
        }
    }

    fn state(read_at: Option<i64>, marked_unread: bool) -> StateRow {
        StateRow {
            article_id: 1,
            read_at,
            marked_unread,
            liked_at: None,
        }
    }

    fn subscription(watermark: i64) -> SubscriptionRow {
        SubscriptionRow {
            feed_id: 1,
            watermark_id: watermark,
            created_at: 0,
            deleted_at: None,
        }
    }

    fn input<'a>(
        article: &'a ArticleRow,
        state: Option<&'a StateRow>,
        subscription: Option<&'a SubscriptionRow>,
    ) -> ReadRuleInput<'a> {
        ReadRuleInput {
            article,
            state,
            subscription,
            horizon_days: 30,
            now_ms: NOW,
        }
    }

    #[test]
    fn read_at_wins_over_everything() {
        let article = article(1, NOW);
        let state = state(Some(NOW), true);
        assert_eq!(
            read_rule(input(&article, Some(&state), None)),
            ReadRuleHit::ReadAt
        );
    }

    #[test]
    fn marked_unread_wins_over_watermark_and_horizon() {
        let article = article(1, NOW - 100 * DAY);
        let state = state(None, true);
        let subscription = subscription(999);
        assert_eq!(
            read_rule(input(&article, Some(&state), Some(&subscription))),
            ReadRuleHit::MarkedUnread
        );
    }

    #[test]
    fn watermark_reads_old_ids() {
        let article = article(5, NOW);
        let subscription = subscription(5);
        assert_eq!(
            read_rule(input(&article, None, Some(&subscription))),
            ReadRuleHit::Watermark
        );
    }

    #[test]
    fn horizon_reads_old_posts_without_state() {
        let article = article(9, NOW - 31 * DAY);
        assert_eq!(read_rule(input(&article, None, None)), ReadRuleHit::Horizon);
    }

    #[test]
    fn fresh_post_without_state_is_unread() {
        let article = article(9, NOW - DAY);
        assert_eq!(read_rule(input(&article, None, None)), ReadRuleHit::Default);
    }
}
