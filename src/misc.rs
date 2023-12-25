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

pub fn some_string(str: &str) -> Option<String> {
    Some(str.to_string())
}

use regex::Regex;

pub fn hostname() -> String {
    hostname::get()
        .ok()
        .map(|name| format!("{}", name.to_str().unwrap_or("")))
        .unwrap_or("localhost".to_string())
}

pub trait StringUtils {
    fn slug(&self) -> String;
    fn strip_repeated_suffix(&self, suffix: &str) -> String;
}
impl StringUtils for String {
    fn slug(&self) -> String {
        self.as_str().slug()
    }

    fn strip_repeated_suffix(&self, suffix: &str) -> String {
        let mut stripped = self.as_str();
        while let Some(stripped_again) = stripped.strip_suffix(suffix) {
            stripped = stripped_again;
        }
        return stripped.to_string();
    }
}
impl StringUtils for &str {
    fn slug(&self) -> String {
        let renamed_symbols = self.replace("%", "percent");
        Regex::new(r"[^a-zA-Z0-9_-]")
            .unwrap()
            .replace_all(renamed_symbols.as_str(), "_")
            .to_string()
    }

    fn strip_repeated_suffix(&self, suffix: &str) -> String {
        return self.to_string().strip_repeated_suffix(suffix);
    }
}
