use actix::prelude::*;
use async_stream::stream;
use chrono::Duration;
use hass_mqtt_autodiscovery::mqtt::{
    common::{
        Availability, AvailabilityCheck, Device, EntityCategory, MqttCommon, ReadOnlyEntity,
        SensorStateClass,
    },
    sensor::{Sensor, SensorDeviceClass},
    units::{MassUnit, SignalStrengthUnit, TempUnit, TimeUnit, Unit},
};
use lazy_static::lazy_static;
use log::{debug, error, info};
use rika_firenet_client::model::StatusDetail;
use rika_firenet_client::HasDetailledStatus;
use rika_firenet_client::{RikaFirenetClient, RikaFirenetClientBuilder, StoveStatus};
use std::{collections::HashMap, fmt::Display, ops::Deref};
use url::Url;

use crate::{
    misc::{app_infos, some_string, StringUtils},
    mqtt::{MqttActor, PublishEntityData, RegisterEntity},
};

lazy_static! {
    static ref RIKA_DISCOVERY_INTERVAL: Duration = Duration::days(7);
    static ref RIKA_STATUS_INTERVAL: Duration = Duration::seconds(10);
}

pub struct RikaActor {
    mqtt_addr: Addr<MqttActor>,
    client: RikaFirenetClient,
    stoves: HashMap<String, RikaEntities>,
}

impl RikaActor {
    pub fn new(
        mqtt_addr: Addr<MqttActor>,
        base_url: Url,
        username: String,
        password: String,
    ) -> Self {
        RikaActor {
            mqtt_addr,
            client: RikaFirenetClientBuilder::default()
                .base_url(base_url.to_string().strip_repeated_suffix("/"))
                .credentials(username, password)
                .build(),
            stoves: HashMap::new(),
        }
    }

    fn execute_stove_discovery(act: &mut RikaActor, ctx: &mut Context<Self>) {
        let client = act.client.clone();
        ctx.add_stream(stream! {
            match client.list_stoves().await {
                Ok(stove_ids) => {
                    for stove_id in stove_ids {
                        match client.status(stove_id.clone()).await {
                            Ok(status) => {
                                yield status;
                            },
                            Err(error) => error!("error fetching stove id={stove_id} status: {error}"),
                        }
                    }
                },
                Err(error) => error!("error listing stoves: {error:?}"),
            }
        });
    }

    fn execute_stove_scraper(act: &mut RikaActor, ctx: &mut Context<Self>) {
        let known_stove_ids: Vec<String> = act.stoves.keys().map(String::clone).collect();
        let client = act.client.clone();
        ctx.add_stream(stream! {
            for stove_id in known_stove_ids {
                match client.status(stove_id.clone()).await {
                    Ok(status) => {
                        yield status;
                    },
                    Err(error) => error!("error fetching stove id={stove_id} status: {error}"),
                }
            }
        });
    }
}

impl Actor for RikaActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let discovery_interval = RIKA_DISCOVERY_INTERVAL.deref();
        info!("Scheduling stoves discovery every {discovery_interval}");
        let discovery_interval = discovery_interval.to_std().expect("A valid std::Duration");
        ctx.run_later(std::time::Duration::ZERO, Self::execute_stove_discovery);
        ctx.run_interval(discovery_interval, Self::execute_stove_discovery);

        let status_interval = RIKA_STATUS_INTERVAL.deref();
        debug!("Scheduling stove data update every {status_interval}");
        let status_interval = status_interval.to_std().expect("A valid std::Duration");
        ctx.run_interval(status_interval, Self::execute_stove_scraper);
    }
}

impl StreamHandler<StoveStatus> for RikaActor {
    fn handle(&mut self, stove_status: StoveStatus, ctx: &mut Self::Context) {
        let stove_id = stove_status.stove_id.clone();
        let new_entities = RikaEntities::from(&stove_status);
        let old_entities = self.stoves.insert(stove_id, new_entities.clone());

        if Some(&new_entities) != old_entities.as_ref() {
            debug!("Publishing configuration for {new_entities}");
            for entity in new_entities.collect() {
                self.mqtt_addr.do_send(RegisterEntity::Sensor(entity));
            }
        }

        debug!("Publishing data for {new_entities}");

        let original_payload = serde_json::to_value(&stove_status).unwrap_or_default();
        let mut enriched_payload = original_payload.as_object().unwrap().clone();
        enriched_payload.insert(
            "status".to_string(),
            status_detail_to_str(&stove_status.get_status_details()).into(),
        );

        self.mqtt_addr.do_send(PublishEntityData::new(
            new_entities.state_topic.clone(),
            enriched_payload.into(),
        ));
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        // override default behavior to keep the actor running
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

#[derive(PartialEq, Clone)]
struct RikaEntities {
    display_name: String,
    state_topic: String,
    status_sensor: Sensor,
    room_temperature_sensor: Sensor,
    flame_temperature_sensor: Sensor,
    bake_temperature_sensor: Sensor,
    wifi_strength_sensor: Sensor,
    pellet_consumption_sensor: Sensor,
    runtime_sensor: Sensor,
    ignition_sensor: Sensor,
    onoff_cycles_sensor: Sensor,
    parameter_error_count: Vec<Sensor>,
}

impl Display for RikaEntities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RikaEntities {}", self.display_name)
    }
}

impl RikaEntities {
    fn collect(&self) -> Vec<Sensor> {
        let mut entities = self.parameter_error_count.clone();
        entities.push(self.status_sensor.clone());
        entities.push(self.room_temperature_sensor.clone());
        entities.push(self.flame_temperature_sensor.clone());
        entities.push(self.bake_temperature_sensor.clone());
        entities.push(self.wifi_strength_sensor.clone());
        entities.push(self.pellet_consumption_sensor.clone());
        entities.push(self.runtime_sensor.clone());
        entities.push(self.ignition_sensor.clone());
        entities.push(self.onoff_cycles_sensor.clone());
        return entities;
    }
}

impl From<&StoveStatus> for RikaEntities {
    fn from(stove_status: &StoveStatus) -> RikaEntities {
        let manufacturer = &stove_status.oem;
        let model = &stove_status.stove_type;
        let name = &stove_status.name;
        let id = &stove_status.stove_id;
        let unique_id = &format!("{manufacturer}_{model}-{name}-{id}").slug();

        let state_topic = format!("rika-firenet/{unique_id}/state");

        let device = Device {
            name: Some(format!("Stove {name}")),
            identifiers: vec![unique_id.clone()],
            connections: Vec::new(),
            configuration_url: Some(format!("https://www.rika-firenet.com/web/stove/{id}",)),
            manufacturer: Some(manufacturer.clone()),
            model: Some(model.clone()),
            suggested_area: None,
            sw_version: Some(
                stove_status
                    .sensors
                    .parameter_version_main_board
                    .to_string(),
            ),
            hw_version: None,
            via_device: None,
        };

        let availability = Availability::single(AvailabilityCheck {
            payload_available: Some("0".to_string()),
            payload_not_available: None,
            topic: "~/state".to_string(),
            value_template: some_string("{{ value_json.lastSeenMinutes }}"),
        });

        let common = MqttCommon {
            topic_prefix: Some(format!("rika-firenet/{unique_id}")),
            origin: app_infos::origin(),
            device,
            entity_category: None,
            icon: None,
            json_attributes_topic: None,
            json_attributes_template: None,
            object_id: None,
            unique_id: None,
            availability,
            enabled_by_default: Some(true),
        };

        RikaEntities {
            display_name: format!("{name} (id={id})"),
            state_topic: state_topic.clone(),
            status_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-status")),
                    unique_id: Some(format!("{unique_id}-status")),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            room_temperature_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-temp")),
                    unique_id: Some(format!("{unique_id}-temp")),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            flame_temperature_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-flame-temp")),
                    unique_id: Some(format!("{unique_id}-flame-temp")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            bake_temperature_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-bake-temp")),
                    unique_id: Some(format!("{unique_id}-bake-temp")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    enabled_by_default: Some(false),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            wifi_strength_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-wifi-strength")),
                    unique_id: Some(format!("{unique_id}-wifi-strength")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
                    value_template: some_string("{{ value_json.sensors.statusWifiStrength }}"),
                },
                device_class: Some(SensorDeviceClass::SignalStrength),
                expire_after: Some(60),
                force_update: None,
                last_reset_value_template: None,
                name: some_string("Wifi strength"),
                suggested_display_precision: None,
                state_class: Some(SensorStateClass::Measurement),
                unit_of_measurement: Some(Unit::SignalStrength(
                    SignalStrengthUnit::DecibelsMilliwatt,
                )),
            },
            pellet_consumption_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-feed-rate-total")),
                    unique_id: Some(format!("{unique_id}-feed-rate-total")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            runtime_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-runtime")),
                    unique_id: Some(format!("{unique_id}-runtime")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            ignition_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-ignition-count")),
                    unique_id: Some(format!("{unique_id}-ignition-count")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            },
            onoff_cycles_sensor: Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-onoff-count")),
                    unique_id: Some(format!("{unique_id}-onoff-count")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
                    value_template: some_string(
                        "{{ value_json.sensors.parameterOnOffCycleCount }}",
                    ),
                },
                device_class: None,
                expire_after: Some(60),
                force_update: None,
                last_reset_value_template: None,
                name: some_string("On/Off cycle count"),
                suggested_display_precision: None,
                state_class: Some(SensorStateClass::Measurement),
                unit_of_measurement: None,
            },
            parameter_error_count: vec![
                0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 18, 19,
            ]
            .into_iter()
            .map(|number| Sensor {
                common: MqttCommon {
                    object_id: Some(format!("{unique_id}-p-err-count-{number}")),
                    unique_id: Some(format!("{unique_id}-p-err-count-{number}")),
                    entity_category: Some(EntityCategory::Diagnostic),
                    ..common.clone()
                },
                ro_entity: ReadOnlyEntity {
                    state_topic: state_topic.clone(),
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
            })
            .collect(),
        }
    }
}
