use std::{
    fmt::{Debug, Display},
    time::Duration,
};

use chrono::TimeDelta;
use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use url::Url;

pub(crate) mod app_infos {
    use ha_mqtt_discovery::mqtt::common::Origin;
    use package_info::PackageInfo;
    use package_info_derive::PackageInfo;

    #[derive(PackageInfo)]
    struct CargoPackageInfo;

    pub fn name() -> String {
        CargoPackageInfo::name().unwrap_or("unknown-package".to_string())
    }

    pub fn version() -> String {
        CargoPackageInfo::version().unwrap_or("unknown-version".to_string())
    }

    pub(crate) fn origin() -> Origin {
        Origin {
            name: name(),
            sw_version: CargoPackageInfo::version(),
            support_url: CargoPackageInfo::homepage().or(CargoPackageInfo::repository()),
        }
    }
}

pub fn hostname() -> String {
    hostname::get()
        .ok()
        .map(|name| format!("{}", name.to_str().unwrap_or("")))
        .unwrap_or("localhost".to_string())
}

pub trait Sluggable {
    fn slug(&self) -> String;
}
impl Sluggable for String {
    fn slug(&self) -> String {
        self.as_str().slug()
    }
}
impl Sluggable for &str {
    fn slug(&self) -> String {
        let nfkd = self.nfkd().to_string();
        let without_diacritics = Regex::new(r"\p{M}").unwrap().replace_all(&nfkd, "");
        Regex::new(r"[^a-zA-Z0-9_-]")
            .unwrap()
            .replace_all(&without_diacritics.to_string(), "_")
            .to_string()
    }
}

pub trait SuffixStrip {
    fn strip_repeated_suffix(&self, suffix: &str) -> String;
}
impl SuffixStrip for String {
    fn strip_repeated_suffix(&self, suffix: &str) -> String {
        let mut stripped = self.as_str();
        while let Some(stripped_again) = stripped.strip_suffix(suffix) {
            stripped = stripped_again;
        }
        return stripped.to_string();
    }
}
impl SuffixStrip for &str {
    fn strip_repeated_suffix(&self, suffix: &str) -> String {
        return self.to_string().strip_repeated_suffix(suffix);
    }
}
impl SuffixStrip for Url {
    fn strip_repeated_suffix(&self, suffix: &str) -> String {
        self.to_string().strip_repeated_suffix(suffix)
    }
}

pub trait HumanReadable: Debug {
    fn prettify(&self) -> String;
}

impl HumanReadable for Duration {
    fn prettify(&self) -> String {
        if self.as_secs() < 120 {
            format!("{self:?}")
        } else {
            TimeDelta::from_std(self.clone())
                .expect(&format!(
                    "{self:?} to be a TimeDelta convertible to an std::Duration"
                ))
                .prettify()
        }
    }
}

impl HumanReadable for TimeDelta {
    fn prettify(&self) -> String {
        let seconds_in_min = self.num_seconds() % 60;
        let minutes_in_hour = self.num_minutes() % 60;
        let hours_in_day = self.num_hours() % 24;
        let days = self.num_days();

        if self.num_seconds() < 120 {
            self.to_std()
                .map(|duration| format!("{duration:?}"))
                .unwrap_or(self.to_string())
        } else if self.num_minutes() <= 120 {
            format!(" {}m {seconds_in_min}s", self.num_minutes())
        } else if self.num_hours() < 72 {
            format!(
                " {}h {minutes_in_hour}m {seconds_in_min}s",
                self.num_hours()
            )
        } else if self.num_days() < 72 {
            format!(" {days}d {hours_in_day}h {minutes_in_hour}m {seconds_in_min}s")
        } else {
            format!(" {days}d {hours_in_day}h {minutes_in_hour}m {seconds_in_min}s")
        }
        .replace(" 0d", "")
        .replace(" 0h", "")
        .replace(" 0m", "")
        .replace(" 0s", "")
        .trim()
        .to_string()
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeDelta;

    use crate::misc::{HumanReadable, Sluggable, SuffixStrip};

    #[test]
    fn can_generate_a_slug() {
        assert_eq!("Renée-52".slug(), "Renee-52");
        assert_eq!("Zoë%".slug(), "Zoe_");
        assert_eq!("Adrián 19".slug(), "Adrian_19");
        assert_eq!("Mónic@ [34]".slug(), "Monic___34_");
        assert_eq!("ترجمة".slug(), "_____");
        assert_eq!("emoji👍".slug(), "emoji_");
        assert_eq!("Außensirene".slug(), "Au_ensirene");
        assert_eq!("font variant: ℍℌ".slug(), "font_variant__HH");
        assert_eq!("super/subscript: i⁹ i₉".slug(), "super_subscript__i9_i9");
        assert_eq!("fraction: ¼".slug(), "fraction__1_4");
    }

    #[test]
    fn can_strip_repeated_suffix() {
        assert_eq!("Hello world".strip_repeated_suffix("!"), "Hello world");
        assert_eq!("Hello world!".strip_repeated_suffix("!"), "Hello world");
        assert_eq!(
            "Hello world!!!!!!!!!".strip_repeated_suffix("!"),
            "Hello world"
        );
    }

    #[test]
    fn can_pretty_format_durations() {
        assert_eq!(
            TimeDelta::milliseconds(943).prettify(),
            "943ms",
            "should displat ms only"
        );
        assert_eq!(
            TimeDelta::seconds(32).prettify(),
            "32s",
            "should display seconds only"
        );
        assert_eq!(
            TimeDelta::seconds(100000).prettify(),
            "27h 46m 40s",
            "should display d h m s"
        );
        assert_eq!(
            TimeDelta::seconds(89160).prettify(),
            "24h 46m",
            "should hide empty units"
        );
        assert_eq!(
            TimeDelta::milliseconds(6141600030).prettify(),
            "71d 2h",
            "ms are ignored"
        );
        assert_eq!(
            (TimeDelta::days(120)
                + TimeDelta::hours(11)
                + TimeDelta::minutes(25)
                + TimeDelta::seconds(10))
            .prettify(),
            "120d 11h 25m 10s",
            "days are the highest unit"
        );
        assert_eq!(
            (TimeDelta::hours(71) + TimeDelta::minutes(25) + TimeDelta::seconds(10)).prettify(),
            "71h 25m 10s",
            "hours are the highest unit until we reach 72h"
        );
        assert_eq!(
            (TimeDelta::minutes(119) + TimeDelta::seconds(10)).prettify(),
            "119m 10s",
            "minutes are the highest unit until we reach 120m"
        );
    }
}
