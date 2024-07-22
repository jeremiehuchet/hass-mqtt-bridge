use crate::{
    misc::{app_infos, Sluggable},
    mqtt::{EntityConfiguration, MqttActor, PublishEntityData},
};
use actix::prelude::*;
use async_stream::stream;
use chrono::Duration as ChronoDuration;
use ha_mqtt_discovery::{
    mqtt::{
        binary_sensor::BinarySensor,
        common::{
            Availability, AvailabilityCheck, Device, DeviceConnection, EntityCategory,
            SensorStateClass,
        },
        device_classes::{BinarySensorDeviceClass, SensorDeviceClass},
        sensor::Sensor,
        units::{PercentageUnit, SignalStrengthUnit, TempUnit, Unit},
    },
    Entity,
};
use lazy_static::lazy_static;
use log::{error, info, warn};
use serde_json::Value;
use somfy_protect_client::{
    client::SomfyProtectClient,
    models::{device_definition::Type, DeviceOutput, SiteOutput},
};
use std::{collections::HashMap, fmt::Display, ops::Deref, time::Duration, vec};

const MANUFACTURER: &str = "Somfy";
const ALT_MANUFACTURER: &str = "Myfox";
const VALID_MANUFACTURERS: [&str; 2] = [MANUFACTURER, ALT_MANUFACTURER];
lazy_static! {
    static ref SITES_SCRAPE_INTERVAL: ChronoDuration = ChronoDuration::minutes(5);
    static ref DEVICES_SCRAPE_INTERVAL: ChronoDuration = ChronoDuration::minutes(1);
    static ref SENSORS_EXPIRATION_TIME: ChronoDuration = ChronoDuration::minutes(1);
}

pub struct SomfyActor {
    mqtt_addr: Addr<MqttActor>,
    somfy_client: SomfyProtectClient,
    sites: HashMap<String, AlarmSite>,
}

impl SomfyActor {
    pub fn new(mqtt_addr: Addr<MqttActor>, somfy_client: SomfyProtectClient) -> Self {
        Self {
            mqtt_addr,
            somfy_client,
            sites: HashMap::new(),
        }
    }

    fn execute_sites_scraping(act: &mut SomfyActor, ctx: &mut Context<Self>) {
        let client = act.somfy_client.clone();
        ctx.add_stream(stream! {
            match client.list_sites().await {
                Ok(sites) =>{
                    for site in sites {
                        yield site;
                    }
                },
                Err(error) => error!("error listing sites: {error:?}"),
            }
        });
    }

    fn execute_devices_scraping(act: &mut SomfyActor, ctx: &mut Context<Self>) {
        let client = act.somfy_client.clone();
        let sites: Vec<String> = act.sites.keys().map(String::clone).collect();
        ctx.add_stream(stream! {
            for site_id in sites {
                match client.list_devices(site_id.clone()).await {
                    Ok(devices) =>{
                        for device in devices {
                            yield device;
                        }
                    },
                    Err(error) => error!("error listing devices for site {site_id}: {error:?}"),
                }
            }
        });
    }
}

impl Actor for SomfyActor {
    type Context = Context<SomfyActor>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let sites_scrape_interval = SITES_SCRAPE_INTERVAL.deref();
        info!("Scheduling sites scraping every {sites_scrape_interval}");
        let discovery_interval = sites_scrape_interval
            .to_std()
            .expect("A valid std::Duration");
        ctx.run_later(std::time::Duration::ZERO, Self::execute_sites_scraping);
        ctx.run_interval(discovery_interval, Self::execute_sites_scraping);

        let devices_scrape_interval = DEVICES_SCRAPE_INTERVAL.deref();
        info!("Scheduling devices scraping every {devices_scrape_interval}");
        let devices_scrape_interval = devices_scrape_interval
            .to_std()
            .expect("A valid std::Duration");
        ctx.run_interval(devices_scrape_interval, Self::execute_devices_scraping);
    }
}

impl StreamHandler<SiteOutput> for SomfyActor {
    fn handle(&mut self, item: SiteOutput, ctx: &mut Self::Context) {
        self.sites
            .entry(item.site_id.clone())
            .and_modify(|known_site| {
                // TODO: compare site attributes and trigger sensor config update if necessary
            })
            .or_insert_with(|| {
                let new_site = AlarmSite::new(item);
                info!("Watching {new_site}");
                new_site
            });
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        // override default behavior to keep the actor running
        ctx.run_later(Duration::ZERO, Self::execute_devices_scraping);
    }
}

impl StreamHandler<DeviceOutput> for SomfyActor {
    fn handle(&mut self, item: DeviceOutput, ctx: &mut Self::Context) {
        let site_id = item.site_id.clone();
        let known_site = self.sites.entry(site_id).or_insert_with_key(|site_id| {
            let mut empty_site = AlarmSite::new(SiteOutput::default());
            empty_site.site.site_id = site_id.clone();
            empty_site
        });
        known_site.add_device(item);
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        // override default behavior to keep the actor running
        self.sites
            .values()
            .flat_map(|alarm_site| alarm_site.collect_entities())
            .for_each(|entity| self.mqtt_addr.do_send(entity));
        self.sites
            .values()
            .flat_map(|alarm_site| alarm_site.devices.values())
            .for_each(|alarm_device| {
                self.mqtt_addr.do_send(PublishEntityData::new(
                    alarm_device.state_topic(),
                    alarm_device.payload(),
                ))
            })
    }
}

struct AlarmSite {
    site: SiteOutput,
    devices: HashMap<String, AlarmDevice>,
    box_device_id: Option<String>,
}

impl Display for AlarmSite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = &self.site.site_id;
        let fullname = self
            .site
            .name
            .as_ref()
            .map(|name| format!("{name} (id={id})"))
            .unwrap_or(format!("id={id}"));
        write!(f, "alarm site {fullname}")
    }
}

impl AlarmSite {
    fn new(site: SiteOutput) -> Self {
        Self {
            site,
            devices: HashMap::new(),
            box_device_id: None,
        }
    }

    fn add_device(&mut self, somfy_device: DeviceOutput) {
        if somfy_device.device_definition.r#type == Type::Box {
            self.box_device_id = Some(somfy_device.device_id.clone());
            for alarm_device in self.devices.values_mut() {
                alarm_device.via_device = self.box_device_id.clone();
            }
        }
        let device_id = somfy_device.device_id.clone();
        self.devices.entry(device_id).or_insert_with(|| {
            let new_device = AlarmDevice::new(somfy_device, self.box_device_id.clone());
            info!("Watching {new_device}");
            new_device
        });
    }

    fn collect_entities(&self) -> Vec<EntityConfiguration> {
        self.devices
            .values()
            .flat_map(|d| d.collect_entities())
            .collect()
    }
}

struct AlarmDevice {
    somfy_device: DeviceOutput,
    via_device: Option<String>,
}

impl Display for AlarmDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let id = &self.somfy_device.device_id;
        let fullname = self
            .somfy_device
            .label
            .as_ref()
            .map(|label| format!("{label} (id={id})"))
            .unwrap_or(format!("id={id}"));
        write!(f, "alarm device {fullname}")
    }
}

impl AlarmDevice {
    fn new(somfy_device: DeviceOutput, via_device: Option<String>) -> Self {
        Self {
            somfy_device,
            via_device,
        }
    }

    fn collect_entities(&self) -> Vec<EntityConfiguration> {
        let unique_id = self.unique_id();
        let object_id = self.object_id();

        let st = &self.somfy_device.status;

        let mut availability_checks = Vec::new();
        if st.device_lost.is_some() {
            availability_checks.push(
                AvailabilityCheck::topic("~/state")
                    .value_template("{{ value_json.status.device_lost }}")
                    .payload_available("false")
                    .payload_not_available("true"),
            );
        }

        let availability = Availability::all(availability_checks)
            .expire_after(SENSORS_EXPIRATION_TIME.num_seconds().unsigned_abs());

        let binary_sensor_defaults = BinarySensor::default()
            .topic_prefix(self.topic_prefix())
            .state_topic(self.state_topic())
            .origin(app_infos::origin())
            .device(self.into())
            .availability(availability.clone())
            .payload_on("True")
            .payload_off("False");

        let sensor_defaults = Sensor::default()
            .topic_prefix(self.topic_prefix())
            .state_topic(self.state_topic())
            .origin(app_infos::origin())
            .device(self.into())
            .availability(availability);

        let mut binary_sensors = vec![];
        let mut sensors = vec![];

        if self.somfy_device.status.battery_level.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Battery")
                    .unique_id(format!("{unique_id}-bat"))
                    .object_id(format!("{object_id}-battery_level"))
                    .value_template("{{ value_json.status.battery_level }}")
                    .device_class(SensorDeviceClass::Battery)
                    .state_class(SensorStateClass::Measurement)
                    .unit_of_measurement(Unit::Percentage(PercentageUnit::Percentage))
                    .entity_category(EntityCategory::Diagnostic)
                    .force_update(true),
            );
        }

        if self.somfy_device.status.battery_level_state.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Battery state")
                    .unique_id(format!("{unique_id}-bat-state"))
                    .object_id(format!("{object_id}_battery_state"))
                    .value_template("{{ value_json.status.battery_level_state }}")
                    .device_class(SensorDeviceClass::Enum)
                    .state_class(SensorStateClass::Measurement)
                    .entity_category(EntityCategory::Diagnostic)
                    .icon("mdi:battery-heart-variant"),
            );
        }

        if self.somfy_device.status.battery_status.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Battery status")
                    .unique_id(format!("{unique_id}-bat-status"))
                    .object_id(format!("{object_id}_battery_status"))
                    .value_template("{{ value_json.status.battery_status }}")
                    .device_class(SensorDeviceClass::Battery)
                    .state_class(SensorStateClass::Measurement)
                    .entity_category(EntityCategory::Diagnostic),
            );
        }

        if self.somfy_device.status.ble_level.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Bluetooth LE level")
                    .unique_id(format!("{unique_id}-ble-lvl"))
                    .object_id(format!("{object_id}_ble_level"))
                    .value_template("{{ value_json.status.ble_level }}")
                    .device_class(SensorDeviceClass::SignalStrength)
                    .state_class(SensorStateClass::Measurement)
                    .entity_category(EntityCategory::Diagnostic),
            );
        }

        if self.somfy_device.status.cover_present.is_some() {
            binary_sensors.push(
                binary_sensor_defaults
                    .clone()
                    .name("Cover present")
                    .unique_id(format!("{unique_id}-cover"))
                    .object_id(format!("{object_id}_cover"))
                    .value_template("{{ value_json.status.cover_present }}")
                    .device_class(BinarySensorDeviceClass::Occupancy)
                    .entity_category(EntityCategory::Diagnostic),
            );
        }

        if self.somfy_device.status.fsk_level.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("FSK level")
                    .unique_id(format!("{unique_id}-fsk-level"))
                    .object_id(format!("{object_id}_fsk_level"))
                    .value_template("{{ value_json.status.fsk_level }}")
                    .state_class(SensorStateClass::Measurement)
                    .entity_category(EntityCategory::Diagnostic),
            );
        }

        // generic booleans
        for (attr_name, is_some) in vec![
            ("battery_low", st.battery_low.is_some()),
            ("device_lost", st.device_lost.is_some()),
            ("homekit_capable", st.homekit_capable.is_some()),
            ("recalibrateable", st.recalibrateable.is_some()),
            (
                "recalibration_required",
                st.recalibration_required.is_some(),
            ),
        ] {
            if is_some {
                binary_sensors.push(
                    binary_sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.status.{attr_name} }}}}"))
                        .entity_category(EntityCategory::Diagnostic),
                );
            }
        }

        // generic booleans at another level
        for (attr_name, is_some) in vec![
            ("master", self.somfy_device.master.is_some()),
            (
                "update_available",
                self.somfy_device.update_available.is_some(),
            ),
        ] {
            if is_some {
                binary_sensors.push(
                    binary_sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.{attr_name} }}}}"))
                        .entity_category(EntityCategory::Diagnostic),
                );
            }
        }

        if self.somfy_device.status.keep_alive.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Remote keep alive")
                    .unique_id(format!("{unique_id}-keep-alive"))
                    .object_id(format!("{object_id}_keep_alive"))
                    .value_template("{{ value_json.status.keep_alive }}")
                    .state_class(SensorStateClass::Measurement)
                    .entity_category(EntityCategory::Diagnostic),
            );
        }

        // timestamps
        for (attr_name, is_some) in vec![
            ("last_check_in_state", st.last_check_in_state.is_some()),
            ("last_check_out_state", st.last_check_out_state.is_some()),
            ("last_offline_at", st.last_offline_at.is_some()),
            ("last_online_at", st.last_online_at.is_some()),
            (
                "last_shutter_closed_at",
                st.last_shutter_closed_at.is_some(),
            ),
            (
                "last_shutter_opened_at",
                st.last_shutter_opened_at.is_some(),
            ),
            ("last_status_at", st.last_status_at.is_some()),
            (
                "lora_last_connected_at",
                st.lora_last_connected_at.is_some(),
            ),
            (
                "lora_last_disconnected_at",
                st.lora_last_disconnected_at.is_some(),
            ),
            ("lora_last_offline_at", st.lora_last_offline_at.is_some()),
            ("lora_last_online_at", st.lora_last_online_at.is_some()),
            ("lora_last_test_at", st.lora_last_test_at.is_some()),
            (
                "lora_last_test_success_at",
                st.lora_last_test_success_at.is_some(),
            ),
            ("mfa_last_connected_at", st.mfa_last_connected_at.is_some()),
            (
                "mfa_last_disconnected_at",
                st.mfa_last_disconnected_at.is_some(),
            ),
            ("mfa_last_offline_at", st.mfa_last_offline_at.is_some()),
            ("mfa_last_online_at", st.mfa_last_online_at.is_some()),
            ("mfa_last_test_at", st.mfa_last_test_at.is_some()),
            (
                "mfa_last_test_success_at",
                st.mfa_last_test_success_at.is_some(),
            ),
            ("mounted_at", st.mounted_at.is_some()),
        ] {
            if is_some {
                sensors.push(
                    sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.status.{attr_name} }}}}"))
                        .device_class(SensorDeviceClass::Timestamp)
                        .entity_category(EntityCategory::Diagnostic),
                );
            }
        }

        if st.temperature_at.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Temperature at")
                    .unique_id(format!("{unique_id}-temperature-at"))
                    .object_id(format!("{object_id}_temperature-at"))
                    .value_template(format!("{{{{ value_json.status.temperatureAt }}}}"))
                    .device_class(SensorDeviceClass::Timestamp)
                    .entity_category(EntityCategory::Diagnostic)
                    .suggested_display_precision(1),
            );
        }

        // signal %
        for (attr_name, is_some) in vec![
            ("mfa_quality_percent", st.mfa_quality_percent.is_some()),
            ("lora_quality_percent", st.lora_quality_percent.is_some()),
            ("rlink_quality_percent", st.rlink_quality_percent.is_some()),
            ("wifi_level_percent", st.wifi_level_percent.is_some()),
        ] {
            if is_some {
                sensors.push(
                    sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.status.{attr_name} }}}}"))
                        .device_class(SensorDeviceClass::SignalStrength)
                        .state_class(SensorStateClass::Measurement)
                        .unit_of_measurement(Unit::Percentage(PercentageUnit::Percentage))
                        .entity_category(EntityCategory::Diagnostic),
                );
            }
        }

        // signal dBm
        for (attr_name, is_some) in vec![
            ("rlink_quality", st.rlink_quality.is_some()),
            ("wifi_level", st.wifi_level.is_some()),
        ] {
            if is_some {
                sensors.push(
                    sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.status.{attr_name} }}}}"))
                        .device_class(SensorDeviceClass::SignalStrength)
                        .state_class(SensorStateClass::Measurement)
                        .unit_of_measurement(Unit::SignalStrength(
                            SignalStrengthUnit::DecibelsMilliwatt,
                        ))
                        .entity_category(EntityCategory::Diagnostic),
                );
            }
        }

        // misc modes/states
        for (attr_name, is_some) in vec![
            ("power_mode", st.power_mode.is_some()),
            ("power_state", st.power_state.is_some()),
            ("shutter_state", st.shutter_state.is_some()),
        ] {
            if is_some {
                sensors.push(
                    sensor_defaults
                        .clone()
                        .name(attr_name.replace("_", " "))
                        .unique_id(format!("{unique_id}-{attr_name}"))
                        .object_id(format!("{object_id}_{attr_name}"))
                        .value_template(format!("{{{{ value_json.status.{attr_name} }}}}"))
                        .device_class(SensorDeviceClass::Enum),
                );
            }
        }

        if self.somfy_device.status.temperature.is_some() {
            sensors.push(
                sensor_defaults
                    .clone()
                    .name("Temperature")
                    .unique_id(format!("{unique_id}-temp"))
                    .object_id(format!("{object_id}_temperature"))
                    .value_template("{{ value_json.status.temperature }}")
                    .device_class(SensorDeviceClass::Temperature)
                    .state_class(SensorStateClass::Measurement)
                    .unit_of_measurement(Unit::Temperature(TempUnit::Celsius))
                    .force_update(true),
            );
        }

        if let Some(diagnosis) = &self.somfy_device.diagnosis {}
        if let Some(settings) = &self.somfy_device.settings {}

        let mut entities = Vec::new();
        for sensor in sensors {
            entities.push(EntityConfiguration(Entity::Sensor(sensor)));
        }
        for binary_sensor in binary_sensors {
            entities.push(EntityConfiguration(Entity::BinarySensor(binary_sensor)));
        }
        entities
    }
}

impl Into<Device> for &AlarmDevice {
    fn into(self) -> Device {
        let (manufacturer, model) = self
            .somfy_device
            .device_definition
            .label
            .split_once(" ")
            .and_then(|(manufacturer, model)| {
                if VALID_MANUFACTURERS.contains(&manufacturer) {
                    Some((manufacturer, model))
                } else {
                    None
                }
            })
            .unwrap_or((
                MANUFACTURER,
                self.somfy_device.device_definition.label.as_str(),
            ));
        let mut device = Device::default()
            .name(self.name())
            .add_identifier(self.unique_id())
            .manufacturer(manufacturer)
            .model(model);
        device.connections = self
            .somfy_device
            .mac
            .iter()
            .map(DeviceConnection::mac)
            .collect();
        device.sw_version = self.somfy_device.version.clone();
        device.via_device = self.via_device.clone();
        device
    }
}

trait HomeAssistantDeviceAttributes {
    fn name(&self) -> String;
    fn unique_id(&self) -> String;
    fn object_id(&self) -> String;
    fn topic_prefix(&self) -> String;
    fn state_topic(&self) -> String;
    fn payload(&self) -> Value;
}
impl HomeAssistantDeviceAttributes for &AlarmDevice {
    fn name(&self) -> String {
        let dev_id = &self.somfy_device.device_id;
        let dev_def_label = &self.somfy_device.device_definition.label;
        self.somfy_device
            .label
            .clone()
            .unwrap_or_else(|| format!("{dev_def_label} (id={dev_id})"))
    }

    fn unique_id(&self) -> String {
        let somfy_site_id = &self.somfy_device.site_id;
        let somfy_device_id = &self.somfy_device.device_id;
        format!("{MANUFACTURER}-{somfy_site_id}-{somfy_device_id}").slug()
    }

    fn object_id(&self) -> String {
        let device_type = serde_json::to_string(&self.somfy_device.device_definition.r#type)
            .unwrap_or("unknown".to_string());
        let name = self.name();
        format!("{MANUFACTURER}_{device_type}_{name}").slug()
    }

    fn topic_prefix(&self) -> String {
        let unique_id = self.unique_id();
        format!("somfy-protect/{unique_id}")
    }

    fn state_topic(&self) -> String {
        let topic_prefix = self.topic_prefix();
        format!("{topic_prefix}/state")
    }

    fn payload(&self) -> Value {
        serde_json::to_value(&self.somfy_device)
            .map_err(|error| {
                warn!(
                    "unable to serialize payload to json: {error:?}\n{:?}",
                    self.somfy_device
                )
            })
            .unwrap_or_default()
    }
}
