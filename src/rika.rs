use crate::{
    misc::{app_infos, Sluggable},
    mqtt::{
        EntityConfiguration, HaMqttEntity, MqttActor, MqttMessage, PublishEntityData, Subscribe,
    },
};
use actix::prelude::*;
use anyhow::bail;
use async_stream::stream;
use backoff::{future::retry_notify, ExponentialBackoff};
use chrono::Duration;
use derive_new::new;
use hass_mqtt_autodiscovery::{
    mqtt::{
        climate::Climate,
        common::{
            Availability, AvailabilityCheck, Device, EntityCategory, SensorStateClass,
            TemperatureUnit,
        },
        device_classes::{SensorDeviceClass, SwitchDeviceClass},
        number::Number,
        select::Select,
        sensor::Sensor,
        switch::Switch,
        units::{MassUnit, PercentageUnit, SignalStrengthUnit, TempUnit, TimeUnit, Unit},
    },
    Entity,
};
use indoc::indoc;
use lazy_static::lazy_static;
use log::{debug, error, info, trace, warn};
use regex::Regex;
use rika_firenet_client::{HasDetailledStatus, StoveControls};
use rika_firenet_client::{RikaFirenetClient, StoveStatus};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use std::{collections::HashMap, fmt::Display, ops::Deref, vec};

lazy_static! {
    static ref RIKA_DISCOVERY_INTERVAL: Duration = Duration::days(7);
    static ref RIKA_STATUS_INTERVAL: Duration = Duration::seconds(10);
    static ref RIKA_SENSOR_EXPIRATION_TIME: Duration = Duration::minutes(2);
    static ref DEDUPLICATE_COMMANDS_GRACE_TIME: Duration = Duration::seconds(5);
}

const COMMON_BASE_TOPIC: &str = "rika-firenet";

pub struct RikaActor {
    mqtt_addr: Addr<MqttActor>,
    rika_client: RikaFirenetClient,
    stoves: HashMap<String, RikaEntities>,
    pending_commands: Vec<RikaFirenetCommand>,
}

impl RikaActor {
    pub fn new(mqtt_addr: Addr<MqttActor>, rika_client: RikaFirenetClient) -> Self {
        RikaActor {
            mqtt_addr,
            rika_client,
            stoves: HashMap::new(),
            pending_commands: Vec::new(),
        }
    }

    fn execute_stove_discovery(act: &mut RikaActor, ctx: &mut Context<Self>) {
        let known_stove_ids: Vec<String> = act.stoves.keys().map(String::to_string).collect();
        let client = act.rika_client.clone();
        ctx.add_stream(stream! {
            let backoff = ExponentialBackoff::default();
            let list_stoves =  || async {
                Ok(client.list_stoves().await?)
            };
            let on_error = |e, next|{
                warn!("Will retry stove discovery in {next:?} because it failed: {e}'");
            };
            match retry_notify(backoff, list_stoves, on_error).await {
                Ok(stove_ids) => {
                    let new_stoves = stove_ids.into_iter().filter(|id| !known_stove_ids.contains(id));
                    for stove_id in new_stoves {
                        info!("Discovered stove id={stove_id}");
                        yield StoveDiscovered::new(stove_id)
                    }
                },
                Err(error) => {
                    error!("Giving up on stove discovery: {error}");
                },
            }
        });
    }

    fn execute_stove_scraper(act: &mut RikaActor, ctx: &mut Context<Self>) {
        let known_stove_ids: Vec<String> = act.stoves.keys().map(String::clone).collect();
        let client = act.rika_client.clone();
        ctx.add_stream(stream! {
            for stove_id in known_stove_ids {
                match client.status(stove_id.as_str()).await {
                    Ok(status) => {
                        yield status;
                    },
                    Err(error) => error!("Error fetching stove id={stove_id} status: {error}"),
                }
            }
        });
    }

    fn handle_topics_subscription_result(
        act: &mut RikaActor,
        ctx: &mut Context<Self>,
        topics_subscription_result: Request<MqttActor, Subscribe>,
    ) {
        async {
            match topics_subscription_result.await {
                Ok(Ok(success)) => info!("Listening for commands on {}", success.topic),
                Ok(Err(err)) => error!(
                    "Can't listen for commands on {}, device is read-only: {}",
                    err.topic, err.error
                ),
                Err(err) => error!("Can't subscribe topic: {err}"),
            };
        }
        .into_actor(act)
        .spawn(ctx);
    }

    fn execute_pending_commands(
        &mut self,
        ctx: &mut Context<Self>,
        known_commands: Vec<RikaFirenetCommand>,
        topic_prefix: String,
    ) {
        let known_commands_for_given_stove: Vec<RikaFirenetCommand> = known_commands
            .iter()
            .filter(|c| c.topic_prefix == topic_prefix)
            .map(|c| c.clone())
            .collect();

        let latest_pending_commands_for_given_stove: Vec<RikaFirenetCommand> = self
            .pending_commands
            .iter()
            .filter(|c| c.topic_prefix == topic_prefix)
            .map(|c| c.clone())
            .collect();

        if known_commands_for_given_stove == latest_pending_commands_for_given_stove {
            self.pending_commands
                .retain(|c| c.topic_prefix != topic_prefix);
            let stove_id = self
                .stoves
                .iter()
                .find(|(_id, status)| status.topic_prefix == topic_prefix)
                .map(|(id, _status)| id.clone());
            if let Some(stove_id) = stove_id {
                info!(
                    "Executing commands for stove id={stove_id}:\n{}",
                    known_commands_for_given_stove
                        .iter()
                        .map(|c| format!("- {:?}", c.attribute))
                        .collect::<Vec<String>>()
                        .join("\n")
                );
                let client = self.rika_client.clone();
                async move {
                    let mut controls = *client.status(&stove_id).await?.controls;
                    for command in known_commands_for_given_stove {
                        command.apply_to(&mut controls);
                    }
                    client.restore_controls(&stove_id, controls).await?;
                    client.status(&stove_id).await
                }
                .into_actor(self)
                .map(move |res, _act, ctx| {
                    match res {
                        Ok(status) => {
                            ctx.add_stream(stream! {
                                yield status
                            });
                        }
                        Err(err) => {
                            error!("Stove controls update failed: {err}");
                        }
                    };
                })
                .spawn(ctx);
            }
        }
    }
}

impl Actor for RikaActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let discovery_interval = RIKA_DISCOVERY_INTERVAL.deref();

        // subscribe to all changes related to topics managed by this actor
        let topics_subscription_result = self.mqtt_addr.send(Subscribe::new(
            format!("{COMMON_BASE_TOPIC}/+/+/set"),
            ctx.address().recipient(),
        ));
        ctx.run_later(
            std::time::Duration::ZERO,
            |act: &mut RikaActor, ctx: &mut Context<Self>| {
                Self::handle_topics_subscription_result(act, ctx, topics_subscription_result)
            },
        );

        info!("Scheduling stoves discovery every {discovery_interval}");
        let discovery_interval = discovery_interval.to_std().expect("A valid std::Duration");
        ctx.run_later(std::time::Duration::ZERO, Self::execute_stove_discovery);
        ctx.run_interval(discovery_interval, Self::execute_stove_discovery);

        let status_interval = RIKA_STATUS_INTERVAL.deref();
        info!("Scheduling stove data update every {status_interval}");
        let status_interval = status_interval.to_std().expect("A valid std::Duration");
        ctx.run_interval(status_interval, Self::execute_stove_scraper);
    }
}

#[derive(new)]
struct StoveDiscovered {
    id: String,
}

impl StreamHandler<StoveDiscovered> for RikaActor {
    fn handle(&mut self, stove: StoveDiscovered, ctx: &mut Self::Context) {
        let stove_id = stove.id;
        info!("Found stove id {stove_id}");
        let client = self.rika_client.clone();
        ctx.add_stream(stream! {
            match client.status(&stove_id).await {
                Ok(stove_status) => yield stove_status,
                Err(error) => error!("Error fetching stove id={stove_id} status: {error}"),
            };
        });
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        // override default behavior to keep the actor running
    }
}

impl StreamHandler<StoveStatus> for RikaActor {
    fn handle(&mut self, stove_status: StoveStatus, ctx: &mut Self::Context) {
        let stove_id = stove_status.stove_id.clone();
        let new_entities = RikaEntities::from(&stove_status);
        let old_entities = self.stoves.insert(stove_id, new_entities.clone());

        trace!("Publishing status data {stove_status:?}");
        for data_payload in new_entities.build_payloads(stove_status) {
            self.mqtt_addr.do_send(data_payload);
        }

        if Some(&new_entities) != old_entities.as_ref() {
            trace!("Publishing configurations for {new_entities}");
            for entity in new_entities.list_entities() {
                self.mqtt_addr.do_send(EntityConfiguration(entity));
            }
        }
    }

    fn finished(&mut self, _ctx: &mut Self::Context) {
        // override default behavior to keep the actor running
    }
}

impl Handler<MqttMessage> for RikaActor {
    type Result = ();

    fn handle(&mut self, msg: MqttMessage, ctx: &mut Self::Context) -> Self::Result {
        match RikaFirenetCommand::try_from(msg) {
            Ok(command) => {
                self.pending_commands.push(command.clone());
                let deduplicate_grace_time = DEDUPLICATE_COMMANDS_GRACE_TIME
                    .to_std()
                    .expect("A valid std::Duration");
                let all_commands = self.pending_commands.clone();
                ctx.run_later(deduplicate_grace_time, move |act, ctx| {
                    act.execute_pending_commands(ctx, all_commands, command.topic_prefix)
                });
            }
            Err(err) => debug!("Unsupported command: {err}"),
        }
    }
}

#[derive(PartialEq, Clone)]
struct RikaEntities {
    display_name: String,
    topic_prefix: String,

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
    target_temperature_number: Number,
    idle_temperature_number: Number,
    power_heating_number: Number,

    daily_schedules_switch: Switch,

    frost_protection_swith: Switch,
    frost_protection_temperature: Number,
}

impl Display for RikaEntities {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "RikaEntities {}", self.display_name)
    }
}

impl HaMqttEntity<StoveStatus> for RikaEntities {
    fn list_entities(self) -> Vec<Entity> {
        let mut entities = vec![
            self.status_sensor.into(),
            self.room_temperature_sensor.into(),
            self.flame_temperature_sensor.into(),
            self.bake_temperature_sensor.into(),
            self.wifi_strength_sensor.into(),
            self.pellet_consumption_sensor.into(),
            self.runtime_sensor.into(),
            self.ignition_sensor.into(),
            self.onoff_cycles_sensor.into(),
            self.climate.into(),
            self.onoff_button.into(),
            self.mode_select.into(),
            self.target_temperature_number.into(),
            self.idle_temperature_number.into(),
            self.power_heating_number.into(),
            self.daily_schedules_switch.into(),
            self.frost_protection_swith.into(),
            self.frost_protection_temperature.into(),
        ];
        for error_count in self.parameter_error_count {
            entities.push(error_count.into());
        }
        return entities;
    }

    fn build_payloads(&self, data: StoveStatus) -> Vec<PublishEntityData> {
        let topic_prefix = &self.topic_prefix;
        vec![
            PublishEntityData::new(
                format!("{topic_prefix}/status-detail"),
                data.get_status_details(),
            ),
            PublishEntityData::new(format!("{topic_prefix}/state"), data),
        ]
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

        let topic_prefix = &format!("{COMMON_BASE_TOPIC}/{unique_id}");

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
            .state_topic("~/state")
            .origin(origin.clone())
            .device(device.clone())
            .availability(availability.clone());

        RikaEntities {
            display_name: format!("{name} (id={id})"),
            topic_prefix: topic_prefix.to_string(),
            status_sensor: sensor_defaults
                .clone()
                .name("Status")
                .unique_id(format!("{unique_id}-st"))
                .object_id(format!("{object_id}_status"))
                .state_topic("~/status-detail")
                .value_template("{{ value_json }}")
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
                .icon("mdi:weight-kilogram")
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
                .value_template("{{ value_json.sensors.parameterRuntimePellets / 24 }}")
                .device_class(SensorDeviceClass::Duration)
                .state_class(SensorStateClass::TotalIncreasing)
                .unit_of_measurement(Unit::Time(TimeUnit::Days)),
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
                .name("Controller")
                .topic_prefix(topic_prefix)
                .origin(origin.clone())
                .device(device.clone())
                .availability(availability.clone())
                .optimistic(false)
                .action_topic("~/status-detail")
                .action_template(indoc! {"
                    {%- if value_json in ['Ignition', 'Startup', 'Control', 'Cleaning', 'Burnout'] -%}
                        heating
                    {%- elif value_json == 'Standby' -%}
                        idle
                    {%- else -%}
                        off
                    {%- endif -%}
                "})
                .icon("mdi:fire")
                .max_temp(dec!(28.0))
                .min_temp(dec!(14.0))
                .object_id(format!("{object_id}"))
                .unique_id(format!("{unique_id}"))
                .modes(vec!["off", "heat"])
                .mode_state_topic("~/state")
                .mode_state_template(indoc! {"
                    {%- if value_json.controls.onOff == True -%}
                        heat
                    {%- elif value_json.controls.onOff == False -%}
                        off
                    {%- endif -%}
                "})
                .mode_command_topic("~/power-on/set")
                .mode_command_template(indoc! {"
                    {%- if value == 'heat' -%}
                        true
                    {%- elif value == 'off' -%}
                        false
                    {%- endif -%}
                "})
                .preset_modes(vec!["Manual", "Auto", "comfort"])
                .preset_mode_state_topic("~/state")
                .preset_mode_value_template(indoc! {"
                    {%- if value_json.controls.operatingMode == 0 -%}
                        Manual
                    {%- elif value_json.controls.operatingMode == 1 -%}
                        Auto
                    {%- elif value_json.controls.operatingMode == 2 -%}
                        comfort
                    {%- endif -%}
                "})
                .preset_mode_command_topic("~/operating-mode/set")
                .preset_mode_command_template(indoc! {"
                    {%- if value == 'Manual' -%}
                        0
                    {%- elif value == 'Auto' -%}
                        1
                    {%- elif value == 'comfort' -%}
                        2
                    {%- else -%}
                        2
                    {%- endif -%}
                "})
                .power_command_topic("~/power-on/set")
                .power_command_template(indoc! {"
                    {%- if value == 'heat' -%}
                        true
                    {%- elif value == 'off' -%}
                        false
                    {%- endif -%}
                "})
                .precision(dec!(0.1))
                .temperature_state_topic("~/state")
                .temperature_state_template(indoc! {"
                    {%- if value_json.controls.operatingMode == 2 -%}
                        {{ value_json.controls.targetTemperature }}
                    {%- else -%}
                        None
                    {%- endif -%}
                "})
                .temperature_command_topic("~/target-temp/set")
                .current_temperature_topic("~/state")
                .current_temperature_template("{{ value_json.sensors.inputRoomTemperature }}")
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
                .command_topic("~/power-on/set")
                .payload_on("true")
                .payload_off("false")
                .device_class(SwitchDeviceClass::Switch)
                .state_topic("~/state")
                .state_on("on")
                .state_off("off")
                .value_template(indoc! {"
                    {%- if value_json.controls.onOff == True -%}
                        on
                    {%- elif value_json.controls.onOff == False -%}
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
                    {%- if value_json.controls.operatingMode == 0 -%}
                        Manual
                    {%- elif value_json.controls.operatingMode == 1 -%}
                        Auto
                    {%- elif value_json.controls.operatingMode == 2 -%}
                        Comfort
                    {%- endif -%}
                "})
                .options(vec!["Manual", "Auto", "Comfort"])
                .command_topic("~/operating-mode/set")
                .command_template(indoc! {"
                    {%- if value == 'Manual' -%}
                        0
                    {%- elif value == 'Auto' -%}
                        1
                    {%- elif value == 'Comfort' -%}
                        2
                    {%- endif -%}
                "}),
                target_temperature_number: Number::default()
                    .name("Target temperature")
                    .object_id(format!("{object_id}_target_temperature"))
                    .unique_id(format!("{unique_id}_target_temperature"))
                    .icon("mdi:thermometer-auto")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .state_topic("~/state")
                    .value_template("{{ value_json.controls.targetTemperature }}")
                    .command_topic("~/target-temp/set")
                    .min(dec!(14))
                    .max(dec!(28))
                    .mode("slider")
                    .step(dec!(1))
                    .unit_of_measurement(Unit::Temperature(TempUnit::Celsius)),
                idle_temperature_number: Number::default()
                    .name("Idle temperature")
                    .object_id(format!("{object_id}_idle_temperature"))
                    .unique_id(format!("{unique_id}_idle_temperature"))
                    .icon("mdi:thermometer-low")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .state_topic("~/state")
                    .value_template("{{ value_json.controls.setBackTemperature }}")
                    .command_topic("~/idle-temp/set")
                    .min(dec!(12))
                    .max(dec!(20))
                    .mode("slider")
                    .step(dec!(1))
                    .unit_of_measurement(Unit::Temperature(TempUnit::Celsius)),
                power_heating_number: Number::default()
                    .name("Power heating")
                    .object_id(format!("{object_id}_power_heating"))
                    .unique_id(format!("{unique_id}_power_heating"))
                    .icon("mdi:percent")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .state_topic("~/state")
                    .value_template("{{ value_json.controls.heatingPower }}")
                    .command_topic("~/power-heating/set")
                    .min(dec!(0))
                    .max(dec!(100))
                    .mode("slider")
                    .step(dec!(1))
                    .unit_of_measurement(Unit::Percentage(PercentageUnit::Percentage)),
                daily_schedules_switch: Switch::default()
                    .name("Daily schedules?")
                    .object_id(format!("{object_id}_daily_schedules"))
                    .unique_id(format!("{unique_id}_daily_schedules"))
                    .icon("mdi:home-clock")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .command_topic("~/daily-schedules-enable/set")
                    .payload_on("true")
                    .payload_off("false")
                    .device_class(SwitchDeviceClass::Switch)
                    .state_topic("~/state")
                    .state_on("on")
                    .state_off("off")
                    .value_template(indoc! {"
                        {%- if value_json.controls.heatingTimesActiveForComfort == True -%}
                            on
                        {%- elif value_json.controls.heatingTimesActiveForComfort == False -%}
                            off
                        {%- endif -%}
                    "}),
                frost_protection_swith: Switch::default()
                    .name("Frost protection?")
                    .object_id(format!("{object_id}_frost_protection"))
                    .unique_id(format!("{unique_id}_frost_protection"))
                    .icon("mdi:snowflake-check")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .command_topic("~/frost-protection-enable/set")
                    .payload_on("true")
                    .payload_off("false")
                    .device_class(SwitchDeviceClass::Switch)
                    .state_topic("~/state")
                    .state_on("on")
                    .state_off("off")
                    .value_template(indoc! {"
                        {%- if value_json.controls.frostProtectionActive == True -%}
                            on
                        {%- elif value_json.controls.frostProtectionActive == False -%}
                            off
                        {%- endif -%}
                    "}),
                frost_protection_temperature: Number::default()
                    .name("Frost protection temperature")
                    .object_id(format!("{object_id}_frost_protection_temperature"))
                    .unique_id(format!("{unique_id}_frost_protection_temperature"))
                    .icon("mdi:snowflake-thermometer")
                    .topic_prefix(topic_prefix)
                    .origin(origin.clone())
                    .device(device.clone())
                    .availability(availability.clone())
                    .state_topic("~/state")
                    .value_template("{{ value_json.controls.frostProtectionTemperature }}")
                    .command_topic("~/frost-protection-temp/set")
                    .min(dec!(4))
                    .max(dec!(10))
                    .mode("slider")
                    .step(dec!(1))
                    .unit_of_measurement(Unit::Temperature(TempUnit::Celsius)),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum ControlAttribute {
    OnOff(bool),
    OperatingMode(i32),
    TargetTemperature(Decimal),
    IdleTemperature(Decimal),
    PowerHeating(i32),
    DailySchedulesEnabled(bool),
    FrostProtectionEnabled(bool),
    FrostProtectionTemperature(Decimal),
}

#[derive(Debug, new, Clone, PartialEq)]
struct RikaFirenetCommand {
    topic_prefix: String,
    attribute: ControlAttribute,
}

impl RikaFirenetCommand {
    fn apply_to(&self, controls: &mut StoveControls) {
        match self.attribute {
            ControlAttribute::OnOff(enabled) => controls.on_off = Some(enabled),
            ControlAttribute::OperatingMode(mode) => controls.operating_mode = Some(mode),
            ControlAttribute::TargetTemperature(temp) => {
                controls.target_temperature = Some(temp.to_string())
            }
            ControlAttribute::IdleTemperature(temp) => {
                controls.set_back_temperature = Some(temp.to_string())
            }
            ControlAttribute::PowerHeating(percent) => controls.heating_power = Some(percent),
            ControlAttribute::DailySchedulesEnabled(enabled) => {
                controls.heating_times_active_for_comfort = Some(enabled)
            }
            ControlAttribute::FrostProtectionEnabled(enabled) => {
                controls.frost_protection_active = Some(enabled)
            }
            ControlAttribute::FrostProtectionTemperature(temp) => {
                controls.frost_protection_temperature = Some(temp.to_string())
            }
        };
    }
}

impl TryFrom<MqttMessage> for RikaFirenetCommand {
    type Error = anyhow::Error;

    fn try_from(msg: MqttMessage) -> Result<Self, Self::Error> {
        let command_topic_re = Regex::new(&format!("^({COMMON_BASE_TOPIC}/[^/]+)/([^/]+)/set$"))
            .expect("A valid regular expression for rika stove command topic");
        match command_topic_re.captures(&msg.topic).map(|c| c.extract()) {
            Some((_, [topic_prefix, attribute])) => {
                let command = match attribute {
                    "power-on" => ControlAttribute::OnOff(msg.payload.parse()?),
                    "operating-mode" => ControlAttribute::OperatingMode(msg.payload.parse()?),
                    "target-temp" => ControlAttribute::TargetTemperature(msg.payload.parse()?),
                    "idle-temp" => ControlAttribute::IdleTemperature(msg.payload.parse()?),
                    "power-heating" => ControlAttribute::PowerHeating(msg.payload.parse()?),
                    "daily-schedules-enable" => {
                        ControlAttribute::DailySchedulesEnabled(msg.payload.parse()?)
                    }
                    "frost-protection-enable" => {
                        ControlAttribute::FrostProtectionEnabled(msg.payload.parse()?)
                    }
                    "frost-protection-temp" => {
                        ControlAttribute::FrostProtectionTemperature(msg.payload.parse()?)
                    }
                    unsupported_attr => bail!("Unsupported attribute: {unsupported_attr}"),
                };
                Ok(RikaFirenetCommand::new(topic_prefix.to_string(), command))
            }
            None => bail!("Unable to parse command from message: {msg:?}"),
        }
    }
}
