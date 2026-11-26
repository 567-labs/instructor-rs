//! Request-local retry accounting for streaming and non-streaming requests.
use crate::{providers::WireRequest, Client, Error, Event, FailedAttempt, Request, StopReason};
use serde_json::Value;
use std::time::Duration;
use tokio::time::Instant;

pub(crate) struct Attempts {
    started: Instant,
    total_tokens: Option<u64>,
    failures: Vec<FailedAttempt>,
}

impl Attempts {
    pub fn new() -> Self {
        Self {
            started: Instant::now(),
            total_tokens: Some(0),
            failures: Vec::new(),
        }
    }

    pub fn next(&self) -> u64 {
        self.failures.len() as u64 + 1
    }

    pub fn usage(&mut self, tokens: Option<u64>) -> Option<u64> {
        self.total_tokens = self
            .total_tokens
            .zip(tokens)
            .map(|(a, b)| a.saturating_add(b));
        self.total_tokens
    }

    fn delay(&self, req: &Request, failed: &FailedAttempt) -> Result<Duration, StopReason> {
        if !failed.error.retryable()
            && (req.retry_policy.is_none()
                || !matches!(
                    failed.error,
                    Error::Http { .. } | Error::Transport(_) | Error::Provider(_)
                ))
        {
            return Err(StopReason::Terminal);
        }
        if let Some(budget) = req.token_budget {
            let used = self
                .total_tokens
                .ok_or(StopReason::UsageUnavailable { budget })?;
            if used >= budget {
                return Err(StopReason::TokenBudget { budget });
            }
        }
        if failed.attempt > u64::from(req.max_retries) {
            return Err(StopReason::RetriesExhausted);
        }
        let delay = match &req.retry_policy {
            Some(policy) => policy(failed).ok_or(StopReason::Policy)?,
            None => Duration::ZERO,
        };
        if Instant::now().checked_add(delay).is_none()
            || req
                .retry_timeout
                .is_some_and(|limit| self.started.elapsed().saturating_add(delay) >= limit)
        {
            return Err(StopReason::Timeout);
        }
        Ok(delay)
    }

    fn finish(&mut self, reason: StopReason) -> Error {
        if reason == StopReason::Terminal && self.failures.len() == 1 {
            return self.failures.pop().unwrap().error;
        }
        Error::Retry {
            reason,
            total_tokens: self.total_tokens,
            attempts: std::mem::take(&mut self.failures),
        }
    }
}

impl Client {
    pub(crate) async fn reject(
        &self,
        req: &Request,
        wire: &mut WireRequest,
        attempts: &mut Attempts,
        raw: Value,
        error: Error,
    ) -> Result<(), Error> {
        let attempt = attempts.next();
        let needs_feedback = error.retryable();
        self.emit(
            req,
            attempt,
            if needs_feedback {
                Event::ParseError(&error)
            } else {
                Event::CompletionError(&error)
            },
        );
        if raw.is_null() {
            attempts.usage(None);
        }
        let failed = FailedAttempt {
            attempt,
            completion: raw,
            error,
        };
        let delay = attempts.delay(req, &failed);
        if delay.is_ok() && needs_feedback {
            self.config.reask(
                wire,
                &failed.completion,
                &format!("Correct the previous response. {}", failed.error),
            );
        }
        attempts.failures.push(failed);
        let reason = match delay {
            Ok(delay) => {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if req
                    .retry_timeout
                    .is_none_or(|limit| attempts.started.elapsed() < limit)
                {
                    return Ok(());
                }
                StopReason::Timeout
            }
            Err(reason) => reason,
        };
        let error = attempts.finish(reason);
        self.emit(req, attempt, Event::LastAttempt(&error));
        Err(error)
    }
}
