//! Physical request populations and the counts explicitly served for them.
use crate::{
    json::Value,
    stream::{field, unsigned},
    ContractError, ContractResult, SAFE_INTEGER,
};
use serde::{Deserialize, Serialize};

fn refusal(detail: &str) -> ContractError {
    ContractError::InvalidRecord(format!(
        "invalid physical usage: {detail}; inspect the complete original recording with its matching client"
    ))
}

fn count_sum(left: u64, right: u64) -> ContractResult<u64> {
    left.checked_add(right)
        .filter(|sum| *sum <= SAFE_INTEGER)
        .ok_or_else(|| refusal("count exceeds the contract's exact integer range"))
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServedUsage {
    #[serde(rename = "promptTokenCount")]
    pub prompt: u64,
    #[serde(rename = "candidatesTokenCount")]
    pub output: u64,
    #[serde(rename = "thoughtsTokenCount")]
    pub thoughts: u64,
    #[serde(rename = "cachedContentTokenCount")]
    pub cached: u64,
    #[serde(rename = "totalTokenCount")]
    pub total: u64,
}

impl ServedUsage {
    pub fn validate(self) -> ContractResult<()> {
        if [
            self.prompt,
            self.output,
            self.thoughts,
            self.cached,
            self.total,
        ]
        .into_iter()
        .any(|count| count > SAFE_INTEGER)
            || self.prompt.checked_add(self.output) != Some(self.total)
            || self.thoughts > self.output
            || self.cached > self.prompt
        {
            return Err(refusal("served token counts are inconsistent"));
        }
        Ok(())
    }

    pub fn checked_add(self, other: Self) -> ContractResult<Self> {
        self.validate()?;
        other.validate()?;
        let result = Self {
            prompt: count_sum(self.prompt, other.prompt)?,
            output: count_sum(self.output, other.output)?,
            thoughts: count_sum(self.thoughts, other.thoughts)?,
            cached: count_sum(self.cached, other.cached)?,
            total: count_sum(self.total, other.total)?,
        };
        result.validate()?;
        Ok(result)
    }

    pub(crate) fn read(value: Value<'_>, line: usize) -> ContractResult<Self> {
        let count = |key| unsigned(field(value, key, line)?, key, SAFE_INTEGER);
        let result = Self {
            prompt: count("promptTokenCount")?,
            output: count("candidatesTokenCount")?,
            thoughts: count("thoughtsTokenCount")?,
            cached: count("cachedContentTokenCount")?,
            total: count("totalTokenCount")?,
        };
        result.validate()?;
        Ok(result)
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GenerationUsageSummary {
    pub requests: u64,
    pub usage_reports: u64,
    pub unfinalized_requests: u64,
    pub unreported_usage_requests: u64,
    #[serde(deserialize_with = "required_nullable")]
    pub usage: Option<ServedUsage>,
}

impl GenerationUsageSummary {
    pub fn validate(self) -> ContractResult<()> {
        if [
            self.requests,
            self.usage_reports,
            self.unfinalized_requests,
            self.unreported_usage_requests,
        ]
        .into_iter()
        .any(|count| count > SAFE_INTEGER)
            || self
                .usage_reports
                .checked_add(self.unfinalized_requests)
                .and_then(|count| count.checked_add(self.unreported_usage_requests))
                != Some(self.requests)
            || (self.usage_reports == 0) != self.usage.is_none()
        {
            return Err(refusal("request population is inconsistent"));
        }
        if let Some(usage) = self.usage {
            usage.validate()?;
        }
        Ok(())
    }

    pub(crate) fn admit_request(self) -> ContractResult<Self> {
        self.validate()?;
        Ok(Self {
            requests: count_sum(self.requests, 1)?,
            unfinalized_requests: count_sum(self.unfinalized_requests, 1)?,
            ..self
        })
    }

    pub(crate) fn finalize(self, usage: Option<ServedUsage>) -> ContractResult<Self> {
        self.validate()?;
        let unfinalized_requests = self
            .unfinalized_requests
            .checked_sub(1)
            .ok_or_else(|| refusal("outcome has no unfinalized request"))?;
        let result = match usage {
            Some(usage) => {
                usage.validate()?;
                Self {
                    unfinalized_requests,
                    usage_reports: count_sum(self.usage_reports, 1)?,
                    usage: Some(match self.usage {
                        Some(previous) => previous.checked_add(usage)?,
                        None => usage,
                    }),
                    ..self
                }
            }
            None => Self {
                unfinalized_requests,
                unreported_usage_requests: count_sum(self.unreported_usage_requests, 1)?,
                ..self
            },
        };
        result.validate()?;
        Ok(result)
    }

    pub fn checked_add(self, other: Self) -> ContractResult<Self> {
        self.validate()?;
        other.validate()?;
        let result = Self {
            requests: count_sum(self.requests, other.requests)?,
            usage_reports: count_sum(self.usage_reports, other.usage_reports)?,
            unfinalized_requests: count_sum(self.unfinalized_requests, other.unfinalized_requests)?,
            unreported_usage_requests: count_sum(
                self.unreported_usage_requests,
                other.unreported_usage_requests,
            )?,
            usage: match (self.usage, other.usage) {
                (Some(left), Some(right)) => Some(left.checked_add(right)?),
                (left, right) => left.or(right),
            },
        };
        result.validate()?;
        Ok(result)
    }

    pub(crate) fn read(value: Value<'_>, line: usize) -> ContractResult<Self> {
        let count = |key| unsigned(field(value, key, line)?, key, SAFE_INTEGER);
        let usage = field(value, "usage", line)?;
        let result = Self {
            requests: count("requests")?,
            usage_reports: count("usageReports")?,
            unfinalized_requests: count("unfinalizedRequests")?,
            unreported_usage_requests: count("unreportedUsageRequests")?,
            usage: if usage.is_null() {
                None
            } else {
                Some(ServedUsage::read(usage, line)?)
            },
        };
        result.validate()?;
        Ok(result)
    }

    pub(crate) fn require_summary(self, value: Value<'_>, line: usize) -> ContractResult<()> {
        if self != Self::read(value, line)? {
            return Err(refusal(
                "summary contradicts recorded requests and outcomes",
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn served(output: u64) -> ServedUsage {
        ServedUsage {
            prompt: 0,
            output,
            thoughts: 0,
            cached: 0,
            total: output,
        }
    }

    #[test]
    fn absent_and_zero_reports_preserve_distinct_request_populations() {
        let pending = GenerationUsageSummary::default()
            .admit_request()
            .unwrap()
            .admit_request()
            .unwrap();
        assert_eq!(pending.unfinalized_requests, 2);
        let absent = pending.finalize(None).unwrap();
        assert_eq!(absent.usage, None);
        assert_eq!(absent.unreported_usage_requests, 1);
        let zero = absent.finalize(Some(served(0))).unwrap();
        assert_eq!(zero.usage, Some(served(0)));
        assert_eq!(zero.usage_reports, 1);
        assert_eq!(zero.unreported_usage_requests, 1);
        assert_eq!(zero.unfinalized_requests, 0);
        assert!(zero.finalize(None).is_err());
    }

    #[test]
    fn totals_reject_overflow_and_invalid_partitions_before_replacement() {
        let total = GenerationUsageSummary::default()
            .admit_request()
            .unwrap()
            .finalize(Some(served(SAFE_INTEGER)))
            .unwrap()
            .admit_request()
            .unwrap();
        assert!(total.finalize(Some(served(1))).is_err());
        assert_eq!(total.unfinalized_requests, 1);
        assert_eq!(total.usage, Some(served(SAFE_INTEGER)));
        assert!(GenerationUsageSummary {
            usage_reports: 1,
            ..GenerationUsageSummary::default()
        }
        .validate()
        .is_err());
        assert!(GenerationUsageSummary {
            usage: Some(served(0)),
            ..GenerationUsageSummary::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn combining_scopes_retains_unreported_and_unfinalized_requests() {
        let internal = GenerationUsageSummary::default()
            .admit_request()
            .unwrap()
            .finalize(None)
            .unwrap();
        let visible = GenerationUsageSummary::default()
            .admit_request()
            .unwrap()
            .finalize(Some(served(7)))
            .unwrap();
        let pending = GenerationUsageSummary::default().admit_request().unwrap();
        let total = internal
            .checked_add(visible)
            .unwrap()
            .checked_add(pending)
            .unwrap();
        assert_eq!(total.requests, 3);
        assert_eq!(total.usage_reports, 1);
        assert_eq!(total.unreported_usage_requests, 1);
        assert_eq!(total.unfinalized_requests, 1);
        assert_eq!(total.usage.unwrap().output, 7);
    }

    #[test]
    fn public_summary_requires_explicit_nullable_usage() {
        let absent = r#"{"requests":0,"usageReports":0,"unfinalizedRequests":0,"unreportedUsageRequests":0}"#;
        assert!(serde_json::from_str::<GenerationUsageSummary>(absent).is_err());
        let encoded = serde_json::to_string(&GenerationUsageSummary::default()).unwrap();
        let decoded: GenerationUsageSummary = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, GenerationUsageSummary::default());
        decoded.validate().unwrap();
    }
}
