use std::{
    collections::HashMap,
    fmt::{format, Display},
};

use chrono::{DateTime, Duration, Local};
use hass_mqtt_autodiscovery::{
    mqtt::{
        common::{
            Availability, AvailabilityCheck, Device, DeviceConnection, EntityCategory, MqttCommon,
            Origin, ReadOnlyEntity, SensorStateClass,
        },
        number::Number,
        sensor::{Sensor, SensorDeviceClass},
        units::{MassUnit, SignalStrengthUnit, TempUnit, TimeUnit, Unit},
    },
    HomeAssistantMqtt,
};
use log::{debug, error, info, warn};
use rika_firenet_client::model::StatusDetail;
use rika_firenet_client::HasDetailledStatus;
use rika_firenet_client::{RikaFirenetClient, RikaFirenetClientBuilder, StoveStatus};
use rumqttc::v5::{
    mqttbytes::{v5::PublishProperties, QoS},
    AsyncClient,
};
use serde_json::json;
use url::Url;

use crate::misc::{app_infos, some_string, Slug};

#[derive(Clone)]
struct StoveSensors {
    device_id: String,
    state_topic: String,
    payload: StoveStatus,
}

impl StoveSensors {
    fn new(stove_status: StoveStatus) -> Self {
        let manufacturer = &stove_status.oem;
        let model = &stove_status.stove_type;
        let name = &stove_status.name;
        let id = &stove_status.stove_id;
        let unique_id = format!("{manufacturer}_{model}-{name}-{id}").slug();
        StoveSensors {
            state_topic: format!("rika-firenet/{}/state", unique_id),
            device_id: unique_id,
            payload: stove_status,
        }
    }

    fn build_device(&self) -> Device {
        Device {
            name: Some(format!("Stove {}", self.payload.name)),
            identifiers: vec![self.device_id.clone()],
            connections: Vec::new(),
            configuration_url: Some(format!(
                "https://www.rika-firenet.com/web/stove/{}",
                self.payload.stove_id
            )),
            manufacturer: Some(self.payload.oem.clone()),
            model: Some(self.payload.stove_type.clone()),
            suggested_area: None,
            sw_version: Some(
                self.payload
                    .sensors
                    .parameter_version_main_board
                    .to_string(),
            ),
            hw_version: None,
            via_device: None,
        }
    }

    fn build_common(&self) -> MqttCommon {
        MqttCommon {
            topic_prefix: Some(format!("rika-firenet/{}", self.device_id)),
            origin: app_infos::origin(),
            device: self.build_device(),
            entity_category: None,
            icon: None,
            json_attributes_topic: None,
            json_attributes_template: None,
            object_id: None,
            unique_id: None,
            availability: self.build_availability(),
            enabled_by_default: Some(true),
        }
    }

    fn build_availability(&self) -> Availability {
        Availability::single(AvailabilityCheck {
            payload_available: Some("0".to_string()),
            payload_not_available: None,
            topic: "~/state".to_string(),
            value_template: some_string("{{ value_json.lastSeenMinutes }}"),
        })
    }

    fn build_sensor_id(&self, suffix: &str) -> Option<String> {
        Some(format!("{}-{suffix}", self.device_id))
    }

    fn status_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("status"),
                unique_id: self.build_sensor_id("status"),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.status }}"),
            },
            device_class: Some(SensorDeviceClass::Enum),
            expire_after: Some(60),
            force_update: Some(false),
            last_reset_value_template: None,
            name: some_string("Status"),
            suggested_display_precision: None,
            state_class: None,
            unit_of_measurement: None,
        }
    }

    fn room_temperature_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("temp"),
                unique_id: self.build_sensor_id("temp"),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.inputRoomTemperature }}"),
            },
            device_class: Some(SensorDeviceClass::Temperature),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Room temperature"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::Temperature(TempUnit::Celsius)),
        }
    }

    fn flame_temperature_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("flame-temp"),
                unique_id: self.build_sensor_id("flame-temp"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.inputFlameTemperature }}"),
            },
            device_class: Some(SensorDeviceClass::Temperature),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Flame temperature"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::Temperature(TempUnit::Celsius)),
        }
    }

    fn bake_temperature_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("bake-temp"),
                unique_id: self.build_sensor_id("bake-temp"),
                entity_category: Some(EntityCategory::Diagnostic),
                enabled_by_default: Some(false),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.inputBakeTemperature }}"),
            },
            device_class: Some(SensorDeviceClass::Temperature),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Bake temperature"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::Temperature(TempUnit::Celsius)),
        }
    }

    fn wifi_strength_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("wifi-strength"),
                unique_id: self.build_sensor_id("wifi-strength"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.statusWifiStrength }}"),
            },
            device_class: Some(SensorDeviceClass::SignalStrength),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Wifi strength"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::SignalStrength(SignalStrengthUnit::DecibelsMilliwatt)),
        }
    }

    fn pellet_consumption_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("feed-rate-total"),
                unique_id: self.build_sensor_id("feed-rate-total"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.parameterFeedRateTotal }}"),
            },
            device_class: Some(SensorDeviceClass::Weight),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Total Consumption"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::Mass(MassUnit::Kilograms)),
        }
    }

    fn runtime_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("runtime"),
                unique_id: self.build_sensor_id("runtime"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.parameterRuntimePellets }}"),
            },
            device_class: Some(SensorDeviceClass::Duration),
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Total runtime"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: Some(Unit::Time(TimeUnit::Hours)),
        }
    }

    fn ignition_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("ignition-count"),
                unique_id: self.build_sensor_id("ignition-count"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.parameterIgnitionCount }}"),
            },
            device_class: None,
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("Ignition count"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: None,
        }
    }

    fn onoff_cycles_sensor(&self) -> Sensor {
        Sensor {
            common: MqttCommon {
                object_id: self.build_sensor_id("onoff-count"),
                unique_id: self.build_sensor_id("onoff-count"),
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: some_string("{{ value_json.sensors.parameterOnOffCycleCount }}"),
            },
            device_class: None,
            expire_after: Some(60),
            force_update: None,
            last_reset_value_template: None,
            name: some_string("On/Off cycle count"),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: None,
        }
    }

    fn parameter_error_count(&self, number: u8) -> Sensor {
        let id = self.build_sensor_id(&format!("p-err-count-{number}"));
        Sensor {
            common: MqttCommon {
                object_id: id.clone(),
                unique_id: id,
                entity_category: Some(EntityCategory::Diagnostic),
                ..self.build_common()
            },
            ro_entity: ReadOnlyEntity {
                state_topic: self.state_topic.clone(),
                value_template: Some(format!(
                    "{{{{ value_json.sensors.parameterErrorCount{number} }}}}"
                )),
            },
            device_class: None,
            expire_after: Some(60),
            force_update: Some(false),
            last_reset_value_template: None,
            name: Some(format!("Parameter error count {number}")),
            suggested_display_precision: None,
            state_class: Some(SensorStateClass::Measurement),
            unit_of_measurement: None,
        }
    }
}

#[derive(Clone)]
pub struct RikaWatcher {
    client: RikaFirenetClient,
    ha_mqtt: HomeAssistantMqtt,
    stove_ids: Vec<String>,
    stoves: Vec<StoveSensors>,
    next_stoves_discovery: DateTime<Local>,
}

impl RikaWatcher {
    pub fn new(
        base_url: Url,
        username: String,
        password: String,
        ha_mqtt: HomeAssistantMqtt,
    ) -> Self {
        Self {
            client: RikaFirenetClientBuilder::default()
                .base_url(base_url)
                .credentials(username, password)
                .build(),
            ha_mqtt,
            stove_ids: Vec::new(),
            stoves: Vec::new(),
            next_stoves_discovery: Local::now(),
        }
    }

    pub async fn execute(&mut self) {
        let now = Local::now();
        if self.next_stoves_discovery < now {
            self.execute_stoves_discovery().await;
            self.fetch_known_stoves_status().await;
            for stove in self.stoves.clone() {
                self.publish_configuration(stove).await;
            }
        } else {
            debug!(
                "Skipping stoves discovery because next discovery is scheduled on {}",
                self.next_stoves_discovery
            );
        }
        self.fetch_known_stoves_status().await;

        for stove in self.stoves.clone() {
            self.publish_data(stove).await;
        }
    }

    async fn execute_stoves_discovery(&mut self) {
        debug!("Execute stoves discovery");
        match self.client.list_stoves().await {
            Ok(stove_ids) => {
                if stove_ids.len() > 0 {
                    self.stove_ids = stove_ids;
                    self.next_stoves_discovery = Local::now() + Duration::days(1);
                    info!(
                        "Found {} stoves: {}",
                        self.stove_ids.len(),
                        self.stove_ids.join(", ")
                    );
                    info!(
                        "Next stoves discovery scheduled on {}",
                        self.next_stoves_discovery
                    );
                } else {
                    warn!("No stove found on Rika Firenet");
                }
            }
            Err(e) => error!("Error while fetching stoves: {e}"),
        };
    }

    async fn fetch_known_stoves_status(&mut self) {
        for stove_id in self.stove_ids.clone() {
            debug!("Fetching stove {stove_id} status");
            match self.client.status(stove_id.clone()).await {
                Ok(stove_status) => {
                    self.stoves
                        .retain(|stove| stove.payload.stove_id != stove_id);
                    self.stoves.push(StoveSensors::new(stove_status));
                }
                Err(e) => error!("Error fetching stove {stove_id} status: {e}"),
            }
        }
    }
    async fn publish_data(&self, stove: StoveSensors) {
        debug!("Publishing data for {}", stove.device_id);

        let original_payload = serde_json::to_value(&stove.payload).unwrap_or_default();
        let mut enriched_payload = original_payload.as_object().unwrap().clone();
        enriched_payload.insert(
            "status".to_string(),
            status_detail_to_str(&stove.payload.get_status_details()).into(),
        );

        match self
            .ha_mqtt
            .publish_data(&stove.state_topic, &enriched_payload, Some(1200))
            .await
        {
            Ok(_) => debug!("Payload published for {}", stove.device_id),
            Err(e) => error!("Unable to publish payload for {}: {e}", stove.device_id),
        }
    }

    async fn publish_configuration(&self, stove: StoveSensors) {
        debug!("Publishing configuration for {}", stove.device_id);
        let mut sensors = vec![
            stove.status_sensor(),
            stove.room_temperature_sensor(),
            stove.flame_temperature_sensor(),
            stove.bake_temperature_sensor(),
            stove.wifi_strength_sensor(),
            stove.pellet_consumption_sensor(),
            stove.runtime_sensor(),
            stove.ignition_sensor(),
            stove.onoff_cycles_sensor(),
            stove.parameter_error_count(0),
        ];
        for i in 0..19 {
            sensors.push(stove.parameter_error_count(i));
        }
        for sensor in sensors {
            let name = sensor.name.clone().unwrap_or("Unnamed".to_string());
            match self.ha_mqtt.publish_sensor(sensor).await {
                Ok(_) => debug!(
                    "Published '{name}' sensor configuration for {}",
                    stove.device_id
                ),
                Err(e) => error!(
                    "Unable to publish '{name}' sensor configuration for {}: {e}",
                    stove.device_id
                ),
            };
        }
    }
}

fn status_detail_to_str(status: &StatusDetail) -> &str {
    match status {
        StatusDetail::Bake => "Bake",
        StatusDetail::Burnout => "Burnout",
        StatusDetail::Cleaning => "Cleaning",
        StatusDetail::Control => "Control",
        StatusDetail::DeepCleaning => "Deep Cleaning",
        StatusDetail::ExternalRequest => "External Request",
        StatusDetail::FrostProtection => "Frost Protection",
        StatusDetail::Heat => "Heat",
        StatusDetail::Ignition => "Ignition",
        StatusDetail::Off => "Off",
        StatusDetail::Standby => "Standby",
        StatusDetail::Startup => "Startup",
        StatusDetail::Unknown => "Unknown",
        StatusDetail::Wood => "Wook",
        StatusDetail::WoodPresenceControl => "Wood Presence Control",
    }
}
