use anyhow::{anyhow, bail, Error};
use anyhow::{Context, Result};
use chrono::TimeDelta;
use regex::Regex;
use std::{ops::RangeInclusive, time::Duration};

pub fn parse_time_delta(arg: &str) -> Result<Duration, Error> {
    let arg = arg.trim();
    let (amount, unit) = Regex::new(r"^(\d+)(ms|s|m|h|d)$")
        .unwrap()
        .captures(arg)
        .map(|captures| captures.extract())
        .map(|(_, [amount, unit])| (amount, unit))
        .ok_or(anyhow!("invalid duration: {arg}"))?;
    let amount: i64 = amount.parse()?;
    match unit {
        "d" => Ok(TimeDelta::days(amount)),
        "h" => Ok(TimeDelta::hours(amount)),
        "m" => Ok(TimeDelta::minutes(amount)),
        "s" => Ok(TimeDelta::seconds(amount)),
        "ms" => Ok(TimeDelta::milliseconds(amount)),
        _ => Err(anyhow!("invalid duration: {arg}")),
    }?
    .abs()
    .to_std()
    .map_err(|err| Error::new(err).context(format!("invalid duration: {arg}")))
}

pub fn parse_time_delta_range(arg: &str) -> Result<RangeInclusive<Duration>, Error> {
    let arg = arg.trim();
    match Regex::new(r"^([^.]*\S)[.]{2}(\S[^.]*)$")
        .unwrap()
        .captures(arg)
        .map(|captures| captures.extract())
        .map(|(_, [start, end])| (parse_time_delta(start), parse_time_delta(end)))
        .ok_or(anyhow!("invalid range syntax: {arg}"))?
    {
        (Ok(start), Ok(end)) => Ok(start..=end),
        (Ok(_), Err(_)) => bail!("invalid end duration: {arg}"),
        (Err(sta__rt), Ok(_)) => bail!("invalid start duration: {arg}"),
        (Err(_), Err(_)) => bail!("invalid start and end durations: {arg}"),
    }
}

#[cfg(test)]
mod tests {
    use std::{ops::RangeInclusive, time::Duration};

    use crate::cli::{parse_time_delta, parse_time_delta_range};
    use chrono::TimeDelta;

    fn to_std_range(time_delta_range: RangeInclusive<TimeDelta>) -> RangeInclusive<Duration> {
        let start = time_delta_range.start().to_std().unwrap();
        let end = time_delta_range.end().to_std().unwrap();
        start..=end
    }

    #[test]
    fn can_parse_valid_time_deltas() {
        assert_eq!(
            parse_time_delta("32d").unwrap(),
            TimeDelta::days(32).to_std().unwrap()
        );
        assert_eq!(
            parse_time_delta("   21h ").unwrap(),
            TimeDelta::hours(21).to_std().unwrap()
        );
        assert_eq!(
            parse_time_delta("37m").unwrap(),
            TimeDelta::minutes(37).to_std().unwrap()
        );
        assert_eq!(
            parse_time_delta("53s").unwrap(),
            TimeDelta::seconds(53).to_std().unwrap()
        );
        assert_eq!(
            parse_time_delta("738ms").unwrap(),
            TimeDelta::milliseconds(738).to_std().unwrap()
        );
    }

    #[test]
    fn can_raise_invalid_format_messages() {
        assert_eq!(
            parse_time_delta("32 d").unwrap_err().to_string(),
            "invalid duration: 32 d"
        );
        assert_eq!(
            parse_time_delta("  food").unwrap_err().to_string(),
            "invalid duration: food"
        );
        assert_eq!(
            parse_time_delta("32y").unwrap_err().to_string(),
            "invalid duration: 32y"
        );
        assert_eq!(
            parse_time_delta("32 y").unwrap_err().to_string(),
            "invalid duration: 32 y"
        );
    }

    #[test]
    fn can_parse_valid_time_delta_rangess() {
        assert_eq!(
            parse_time_delta_range(" 8m..12m   ").unwrap(),
            to_std_range(TimeDelta::minutes(8)..=TimeDelta::minutes(12))
        );
        assert_eq!(
            parse_time_delta_range("21h..40d").unwrap(),
            to_std_range(TimeDelta::hours(21)..=TimeDelta::days(40))
        );
        assert_eq!(
            parse_time_delta_range("1s..5h").unwrap(),
            to_std_range(TimeDelta::seconds(1)..=TimeDelta::hours(5))
        );
        assert_eq!(
            parse_time_delta_range("25h..1d").unwrap(),
            to_std_range(TimeDelta::hours(25)..=TimeDelta::hours(24))
        );
    }

    #[test]
    fn can_raise_invalid_time_delta_rangess() {
        assert_eq!(
            parse_time_delta_range("   8m-12m").unwrap_err().to_string(),
            "invalid range syntax: 8m-12m"
        );
        assert_eq!(
            parse_time_delta_range("1s ..5h").unwrap_err().to_string(),
            "invalid range syntax: 1s ..5h"
        );
        assert_eq!(
            parse_time_delta_range("21h..foo").unwrap_err().to_string(),
            "invalid end duration: 21h..foo"
        );
        assert_eq!(
            parse_time_delta_range("1y..5h").unwrap_err().to_string(),
            "invalid start duration: 1y..5h"
        );
        assert_eq!(
            parse_time_delta_range("foo..bar").unwrap_err().to_string(),
            "invalid start and end durations: foo..bar"
        );
    }
}
