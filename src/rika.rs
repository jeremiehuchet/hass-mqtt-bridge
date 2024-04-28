use crate::{
    misc::{app_infos, Sluggable},
    mqtt::{EntityConfiguration, MqttActor, PublishEntityData},
};
use actix::prelude::*;
use async_stream::stream;
use chrono::Duration;
use hass_mqtt_autodiscovery::{
    mqtt::{
        climate::Climate,
        common::{
            Availability, AvailabilityCheck, Device, EntityCategory, SensorStateClass,
            TemperatureUnit,
        },
        device_classes::{SensorDeviceClass, SwitchDeviceClass},
        select::Select,
        sensor::Sensor,
        switch::Switch,
        units::{MassUnit, SignalStrengthUnit, TempUnit, TimeUnit, Unit},
    },
    Entity,
};
use indoc::indoc;
use lazy_static::lazy_static;
use log::{debug, error, info};
use rika_firenet_client::model::StatusDetail;
use rika_firenet_client::HasDetailledStatus;
use rika_firenet_client::{RikaFirenetClient, StoveStatus};
use rust_decimal_macros::dec;
use std::{collections::HashMap, fmt::Display, ops::Deref};

lazy_static! {
    static ref RIKA_DISCOVERY_INTERVAL: Duration = Duration::days(7);
    static ref RIKA_STATUS_INTERVAL: Duration = Duration::seconds(10);
    static ref RIKA_SENSOR_EXPIRATION_TIME: Duration = Duration::minutes(2);
}

pub struct RikaActor {
    mqtt_addr: Addr<MqttActor>,
    rika_client: RikaFirenetClient,
    stoves: HashMap<String, RikaEntities>,
}

impl RikaActor {
    pub fn new(mqtt_addr: Addr<MqttActor>, rika_client: RikaFirenetClient) -> Self {
        RikaActor {
            mqtt_addr,
            rika_client,
            stoves: HashMap::new(),
        }
    }

    fn execute_stove_discovery(act: &mut RikaActor, ctx: &mut Context<Self>) {
        let client = act.rika_client.clone();
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
        let client = act.rika_client.clone();
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
    fn handle(&mut self, stove_status: StoveStatus, _ctx: &mut Self::Context) {
        let stove_id = stove_status.stove_id.clone();
        let new_entities = RikaEntities::from(&stove_status);
        let old_entities = self.stoves.insert(stove_id, new_entities.clone());

        if Some(&new_entities) != old_entities.as_ref() {
            debug!("Publishing configuration for {new_entities}");
            for entity in new_entities.sensors() {
                self.mqtt_addr
                    .do_send(EntityConfiguration(Entity::Sensor(entity)));
            }
            self.mqtt_addr
                .do_send(EntityConfiguration(Entity::Climate(new_entities.climate)));
        }

        debug!("Publishing data");

        let original_payload = serde_json::to_value(&stove_status).unwrap_or_default();
        let mut enriched_payload = original_payload.as_object().unwrap().clone();
        let status_details = &stove_status.get_status_details();
        enriched_payload.insert(
            "status".to_string(),
            status_detail_to_str(status_details).into(),
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
    climate: Climate,

    onoff_button: Switch,
    mode_select: Select,
}

impl Display for RikaEntities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RikaEntities {}", self.display_name)
    }
}

impl RikaEntities {
    fn sensors(&self) -> Vec<Sensor> {
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
        let unique_id = &format!("{manufacturer}_{model}_{name}-{id}").slug();
        let object_id = &format!("{manufacturer}_{model}_{name}").slug();

        let version = stove_status
            .sensors
            .parameter_version_main_board
            .to_string();
        let (version_major, version_minor) = version.split_at(1);

        let topic_prefix = &format!("rika-firenet/{unique_id}");
        let state_topic = "~/state";

        let origin = app_infos::origin();

        let device = Device::default()
            .name(format!("Stove {name}"))
            .add_identifier(unique_id)
            .configuration_url(format!("https://www.rika-firenet.com/web/stove/{id}"))
            .manufacturer(manufacturer)
            .model(model)
            .sw_version(format!("{version_major}.{version_minor}"));

        let availability = Availability::single(
            AvailabilityCheck::topic("~/state")
                .payload_available("0")
                .value_template("{{ value_json.lastSeenMinutes }}"),
        )
        .expire_after(RIKA_SENSOR_EXPIRATION_TIME.num_seconds().unsigned_abs());

        let sensor_defaults = Sensor::default()
            .topic_prefix(topic_prefix)
            .state_topic(state_topic)
            .origin(origin.clone())
            .device(device.clone())
            .availability(availability.clone());

        RikaEntities {
            display_name: format!("{name} (id={id})"),
            state_topic: format!("{topic_prefix}/state"),
            status_sensor: sensor_defaults
                .clone()
                .name("Status")
                .unique_id(format!("{unique_id}-st"))
                .object_id(format!("{object_id}_status"))
                .value_template("{{ value_json.status }}")
                .device_class(SensorDeviceClass::Enum),
            room_temperature_sensor: sensor_defaults
                .clone()
                .name("Room temperature")
                .unique_id(format!("{unique_id}-temp"))
                .object_id(format!("{object_id}_temperature"))
                .value_template("{{ value_json.sensors.inputRoomTemperature }}")
                .device_class(SensorDeviceClass::Temperature)
                .state_class(SensorStateClass::Measurement)
                .unit_of_measurement(Unit::Temperature(TempUnit::Celsius))
                .force_update(true),
            flame_temperature_sensor: sensor_defaults
                .clone()
                .name("Flame temperature")
                .unique_id(format!("{unique_id}-flame-temp"))
                .object_id(format!("{object_id}_flame_temperature"))
                .value_template("{{ value_json.sensors.inputFlameTemperature }}")
                .entity_category(EntityCategory::Diagnostic)
                .device_class(SensorDeviceClass::Temperature)
                .state_class(SensorStateClass::Measurement)
                .unit_of_measurement(Unit::Temperature(TempUnit::Celsius))
                .force_update(true),
            bake_temperature_sensor: sensor_defaults
                .clone()
                .name("Bake temperature")
                .unique_id(format!("{unique_id}-bake-temp"))
                .object_id(format!("{object_id}_bake_temperature"))
                .value_template("{{ value_json.sensors.inputBakeTemperature }}")
                .entity_category(EntityCategory::Diagnostic)
                .device_class(SensorDeviceClass::Temperature)
                .state_class(SensorStateClass::Measurement)
                .unit_of_measurement(Unit::Temperature(TempUnit::Celsius))
                .force_update(true)
                .enabled_by_default(false),
            wifi_strength_sensor: sensor_defaults
                .clone()
                .name("Wifi strength")
                .unique_id(format!("{unique_id}-wifi-strength"))
                .object_id(format!("{object_id}_wifi_strength"))
                .value_template("{{ value_json.sensors.statusWifiStrength }}")
                .entity_category(EntityCategory::Diagnostic)
                .device_class(SensorDeviceClass::SignalStrength)
                .state_class(SensorStateClass::Measurement)
                .unit_of_measurement(Unit::SignalStrength(SignalStrengthUnit::DecibelsMilliwatt))
                .force_update(true),
            pellet_consumption_sensor: sensor_defaults
                .clone()
                .name("Total consumption")
                .unique_id(format!("{unique_id}-feed-rate-total"))
                .object_id(format!("{object_id}_feed_rate_total"))
                .value_template("{{ value_json.sensors.parameterFeedRateTotal }}")
                .device_class(SensorDeviceClass::Weight)
                .state_class(SensorStateClass::TotalIncreasing)
                .unit_of_measurement(Unit::Mass(MassUnit::Kilograms))
                .force_update(true),
            runtime_sensor: sensor_defaults
                .clone()
                .name("Total runtime")
                .unique_id(format!("{unique_id}-rt"))
                .object_id(format!("{object_id}_runtime"))
                .value_template("{{ value_json.sensors.parameterRuntimePellets }}")
                .device_class(SensorDeviceClass::Duration)
                .state_class(SensorStateClass::TotalIncreasing)
                .unit_of_measurement(Unit::Time(TimeUnit::Hours)),
            ignition_sensor: sensor_defaults
                .clone()
                .name("Ignition count")
                .unique_id(format!("{unique_id}-ignition-count"))
                .object_id(format!("{object_id}_ignition_count"))
                .value_template("{{ value_json.sensors.parameterIgnitionCount }}")
                .entity_category(EntityCategory::Diagnostic)
                .state_class(SensorStateClass::TotalIncreasing),
            onoff_cycles_sensor: sensor_defaults
                .clone()
                .name("On/Off cycle count")
                .unique_id(format!("{unique_id}-onoff-count"))
                .object_id(format!("{object_id}_onoff_count"))
                .value_template("{{ value_json.sensors.parameterOnOffCycleCount }}")
                .entity_category(EntityCategory::Diagnostic)
                .state_class(SensorStateClass::TotalIncreasing),
            parameter_error_count: (0..=19)
                .map(|number| {
                    sensor_defaults
                        .clone()
                        .name(format!("Parameter error count {number}"))
                        .unique_id(format!("{unique_id}-p-err-count-{number}"))
                        .object_id(format!("{object_id}_error_count_{number}"))
                        .value_template(format!(
                            "{{{{ value_json.sensors.parameterErrorCount{number} }}}}"
                        ))
                        .entity_category(EntityCategory::Diagnostic)
                        .state_class(SensorStateClass::TotalIncreasing)
                })
                .collect(),
            climate: Climate::default()
                .topic_prefix(topic_prefix)
                .origin(origin.clone())
                .device(device.clone())
                .availability(availability.clone())
                .action_template(indoc! {"
                    {%- if json_value.status in ['Ignition', 'Startup', 'Control', 'Cleaning', 'Burnout'] -%}
                        heating
                    {%- elif json_value.status == 'Standby' -%}
                        idle
                    {%- else -%}
                        off
                    {%- endif -%}
                "})
                .temperature_command_topic("~/temp/set")
                .current_temperature_template(indoc! {"
                    {%- if json_value.controls.operatingMode == 2 -%}
                        {{ value_json.controls.targetTemperature }}
                    {%- endif -%}
                "})
                .icon("mdi:fire")
                .max_temp(dec!(28.0))
                .min_temp(dec!(14.0))
                .preset_mode_command_topic("~/command/set")
                .preset_mode_value_template("{% if json_value.controls.operatingMode == 2 %}comfort{% endif %}")
                .preset_modes(vec!["Manual", "Auto", "Comfort"])
                .mode_command_topic("~/mode/set")
                .mode_state_template(indoc! {"
                    {%- if json_value.controls.onOff == True -%}
                        heat
                    {%- else -%}
                        off
                    {%- endif -%}
                "})
                .modes(vec!["off", "heat"])
                .object_id(format!("{object_id}"))
                .unique_id(format!("{unique_id}"))
                .power_command_topic("~/power/set".to_string())
                .precision(dec!(0.1))
                .temperature_state_template("{{ value_json.sensors.inputRoomTemperature }}")
                .temperature_unit(TemperatureUnit::Celcius)
                .temp_step(dec!(1)),
            onoff_button: Switch::default()
                .name("Power")
                .object_id(format!("{object_id}_power"))
                .unique_id(format!("{unique_id}_power"))
                .icon("mdi:power")
                .topic_prefix(topic_prefix)
                .origin(origin.clone())
                .device(device.clone())
                .availability(availability.clone())
                .command_topic("~/set")
                .payload_on("on")
                .payload_off("off")
                .device_class(SwitchDeviceClass::Switch)
                .enabled_by_default(false)
                .state_topic("~/state")
                .value_template(indoc! {"
                    {%- if json_value.controls.onOff == True -%}
                        on
                    {%- else -%}
                        off
                    {%- endif -%}
                "}),
            mode_select: Select::default()
                .name("Mode")
                .object_id(format!("{object_id}_mode"))
                .unique_id(format!("{unique_id}_mode"))
                .icon("mdi:format-list-bulleted")
                .topic_prefix(topic_prefix)
                .origin(origin.clone())
                .device(device.clone())
                .availability(availability.clone())
                .state_topic("~/state")
                .value_template(indoc! {"
                    {%- if json_value.controls.operatingMode == 0 -%}
                        Manual
                    {%- elif json_value.controls.operatingMode == 1 -%}
                        Auto
                    {%- else -%}
                        Comfort
                    {%- endif -%}
                "})
                .options(vec!["Manual", "Auto", "Comfort"])
                .command_topic("~/set"),
        }
    }
}
