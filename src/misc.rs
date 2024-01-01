use regex::Regex;
use unicode_normalization::UnicodeNormalization;
use url::Url;

pub(crate) mod app_infos {
    use hass_mqtt_autodiscovery::mqtt::common::Origin;
    use package_info::PackageInfo;
    use package_info_derive::PackageInfo;

    #[derive(PackageInfo)]
    struct CargoPackageInfo {}

    pub fn name() -> String {
        CargoPackageInfo::name().unwrap_or("unknown-package".to_string())
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

#[cfg(test)]
mod tests {
    use crate::misc::{Sluggable, SuffixStrip};

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
}
