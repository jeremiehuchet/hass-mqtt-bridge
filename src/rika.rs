use crate::{
    misc::{app_infos, Sluggable},
    mqtt::{
        EntityConfiguration, HaMqttEntity, MqttActor, MqttMessage, PublishEntityData, Subscribe,
    },
};
use actix::prelude::*;
use actix_web::rt::time;
use anyhow::{bail, Result};
use async_stream::stream;
use backoff::{future::retry_notify, ExponentialBackoff, ExponentialBackoffBuilder};
use chrono::Duration;
use derive_new::new;
use ha_mqtt_discovery::{
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
use std::{fmt::Display, ops::Deref, vec};

lazy_static! {
    static ref RIKA_DISCOVERY_INTERVAL: Duration = Duration::days(7);
    static ref RIKA_STATUS_INTERVAL: Duration = Duration::seconds(10);
    static ref RIKA_SENSOR_EXPIRATION_TIME: Duration = Duration::minutes(2);
    static ref DEDUPLICATE_COMMANDS_GRACE_TIME: Duration = Duration::seconds(2);
    static ref BACKOFF_POLICY: ExponentialBackoff = ExponentialBackoffBuilder::new()
        .with_max_elapsed_time(Some(
            Duration::hours(24)
                .to_std()
                .expect("A valid max elapsed time as std::Duration")
        ))
        .build();
}

const COMMON_BASE_TOPIC: &str = "rika-firenet";

pub struct StoveDiscoveryActor {
    mqtt_addr: Addr<MqttActor>,
    rika_client: RikaFirenetClient,
    stoves: Vec<RunningStoveActor>,
}

#[derive(new)]
struct RunningStoveActor {
    topic_prefix: String,
    addr: Addr<StoveActor>,
}

impl StoveDiscoveryActor {
    pub fn new(mqtt_addr: Addr<MqttActor>, rika_client: RikaFirenetClient) -> Self {
        StoveDiscoveryActor {
            mqtt_addr,
            rika_client,
            stoves: Vec::new(),
        }
    }

    fn handle_topics_subscription_result(
        act: &mut StoveDiscoveryActor,
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
}

impl Actor for StoveDiscoveryActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        // subscribe to all changes related to topics managed by this actor
        let topics_subscription_result = self.mqtt_addr.send(Subscribe::new(
            format!("{COMMON_BASE_TOPIC}/+/+/set"),
            ctx.address().recipient(),
        ));
        ctx.run_later(
            std::time::Duration::ZERO,
            |act: &mut StoveDiscoveryActor, ctx: &mut Context<Self>| {
                Self::handle_topics_subscription_result(act, ctx, topics_subscription_result)
            },
        );

        let discovery_interval = RIKA_DISCOVERY_INTERVAL.deref();
        info!("Scheduling stoves discovery every {discovery_interval}");
        let discovery_interval = discovery_interval.to_std().expect("A valid std::Duration");
        let client = self.rika_client.clone();
        ctx.add_stream(stream! {
            let list_stoves = || async {
                 Ok(client.list_stoves().await?)
            };
            let on_error = |e, next|{
                warn!("Will retry discovering stoves in {next:?} because it failed: {e}'");
            };
            loop {
                match retry_notify(BACKOFF_POLICY.clone(), list_stoves, on_error).await {
                    Ok(stove_ids) => {
                        for stove_id in stove_ids {
                            yield StoveDiscovered::new(stove_id);
                        }
                        time::sleep(discovery_interval).await;
                    }
                    Err(error) => error!("Unable to list stoves: {error}"),
                }
            }
        });
    }
}

#[derive(new)]
struct StoveDiscovered {
    id: String,
}

impl StreamHandler<StoveDiscovered> for StoveDiscoveryActor {
    fn handle(&mut self, stove: StoveDiscovered, ctx: &mut Self::Context) {
        let stove_id = stove.id.clone();
        info!("Found stove id {stove_id}");
        let mqtt_addr = self.mqtt_addr.clone();
        let client = self.rika_client.clone();
        async move { StoveActor::new(mqtt_addr, client, stove_id).await }
            .into_actor(self)
            .map(move |stove_actor, act, _ctx| {
                match stove_actor {
                    Ok(stove_actor) => {
                        let topic_prefix = stove_actor.topic_prefix.clone();
                        let addr = stove_actor.start();
                        act.stoves.push(RunningStoveActor::new(topic_prefix, addr));
                    }
                    Err(error) => {
                        error!("Can't initialize actor for stove id={}: {error}", stove.id)
                    }
                };
            })
            .spawn(ctx);
    }
}

impl Handler<MqttMessage> for StoveDiscoveryActor {
    type Result = ();

    fn handle(&mut self, msg: MqttMessage, _ctx: &mut Self::Context) -> Self::Result {
        match RikaFirenetCommand::try_from(msg) {
            Ok(RikaFirenetCommand {
                topic_prefix,
                command,
            }) => {
                let stove_actor = self.stoves.iter().find(|s| s.topic_prefix == topic_prefix);
                match stove_actor {
                    Some(stove_actor) => stove_actor.addr.do_send(command),
                    None => warn!(
                        "No actor found for a stove identified by topic prefix {topic_prefix}"
                    ),
                }
            }
            Err(error) => debug!("Unsupported command: {error}"),
        }
    }
}

struct StoveActor {
    mqtt_addr: Addr<MqttActor>,
    rika_firenet_client: RikaFirenetClient,
    topic_prefix: String,
    last_status: StoveStatus,
    pending_commands: Vec<StoveCommand>,
}

impl StoveActor {
    async fn new(
        mqtt_addr: Addr<MqttActor>,
        rika_firenet_client: RikaFirenetClient,
        stove_id: String,
    ) -> Result<Self> {
        let last_status = rika_firenet_client.status(stove_id).await?;
        let StoveMetadata { topic_prefix, .. } = (&last_status).into();
        Ok(StoveActor {
            mqtt_addr,
            rika_firenet_client,
            topic_prefix,
            last_status,
            pending_commands: Vec::new(),
        })
    }
}

impl Actor for StoveActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let stove_id = self.last_status.stove_id.clone();
        let client = self.rika_firenet_client.clone();

        let status_interval = RIKA_STATUS_INTERVAL.deref();
        info!("Scheduling stove id {stove_id} data update every {status_interval}");
        let status_interval = status_interval.to_std().expect("A valid std::Duration");

        for entity in RikaEntities::from(&self.last_status).list_entities() {
            self.mqtt_addr.do_send(EntityConfiguration(entity));
        }

        ctx.add_stream(stream! {
            let fetch_stove_status = || async {
                 Ok(client.status(&stove_id).await?)
            };
            let on_error = |e, next|{
                warn!("Will retry stove status id={stove_id} in {next:?} because it failed: {e}'");
            };
            loop {
                match retry_notify(BACKOFF_POLICY.clone(), fetch_stove_status, on_error).await {
                    Ok(status) => {
                        yield status;
                        time::sleep(status_interval).await;
                    }
                    Err(error) => error!("Unable to fetch status for stove id={stove_id}: {error}"),
                }
            }
        });
    }
}

impl StreamHandler<StoveStatus> for StoveActor {
    fn handle(&mut self, stove_status: StoveStatus, _ctx: &mut Self::Context) {
        let stove_id = stove_status.stove_id.clone();
        let old_entities = RikaEntities::from(&self.last_status);
        let new_entities = RikaEntities::from(&stove_status);

        trace!("Publishing status data for stove id={stove_id}: {stove_status:?}");
        for data_payload in new_entities.build_payloads(stove_status) {
            self.mqtt_addr.do_send(data_payload);
        }

        if new_entities != old_entities {
            trace!("Publishing configurations for stove id={stove_id}:\n{new_entities}");
            for entity in new_entities.list_entities() {
                self.mqtt_addr.do_send(EntityConfiguration(entity));
            }
        }
    }
}

impl Handler<StoveCommand> for StoveActor {
    type Result = ();

    fn handle(&mut self, cmd: StoveCommand, ctx: &mut Self::Context) -> Self::Result {
        self.pending_commands.push(cmd);
        let grace_period = DEDUPLICATE_COMMANDS_GRACE_TIME
            .to_std()
            .expect("A valid grace period as std::Duration");
        let pending_commands_before_grace_period = self.pending_commands.clone();
        ctx.run_later(grace_period, move |act, ctx| {
            let client = act.rika_firenet_client.clone();
            if pending_commands_before_grace_period == act.pending_commands {
                let stove_id = act.last_status.stove_id.clone();
                info!(
                    "Executing commands for stove id={stove_id}:\n{}",
                    pending_commands_before_grace_period
                        .iter()
                        .map(|c| format!("- {:?}", c))
                        .collect::<Vec<String>>()
                        .join("\n")
                );
                async move {
                    let mut controls = *client.status(&stove_id).await?.controls;
                    for command in pending_commands_before_grace_period {
                        command.apply_to(&mut controls);
                    }
                    client.restore_controls(&stove_id, controls).await?;
                    client.status(&stove_id).await
                }
                .into_actor(act)
                .map(move |res, _act, ctx| {
                    match res {
                        Ok(status) => {
                            ctx.add_stream(stream! {
                                yield status;
                            });
                        }
                        Err(err) => {
                            error!("Stove controls update failed: {err}");
                        }
                    };
                })
                .spawn(ctx);
            }
        });
    }
}

struct StoveMetadata {
    manufacturer: String,
    model: String,
    name: String,
    id: String,
    unique_id: String,
    object_id: String,
    version: String,
    topic_prefix: String,
}

impl From<&StoveStatus> for StoveMetadata {
    fn from(value: &StoveStatus) -> Self {
        let manufacturer = value.oem.clone();
        let model = value.stove_type.clone();
        let name = value.name.clone();
        let id = value.stove_id.clone();
        let unique_id = format!("{manufacturer}_{model}_{name}-{id}").slug();
        let object_id = format!("{manufacturer}_{model}_{name}").slug();

        let version = value.sensors.parameter_version_main_board.to_string();
        let (version_major, version_minor) = version.split_at(1);

        let topic_prefix = format!("{COMMON_BASE_TOPIC}/{unique_id}");
        StoveMetadata {
            manufacturer,
            model,
            name,
            id,
            unique_id,
            object_id,
            version: format!("{version_major}.{version_minor}"),
            topic_prefix,
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
        let StoveMetadata {
            manufacturer,
            model,
            name,
            id,
            unique_id,
            object_id,
            version,
            topic_prefix,
        } = &stove_status.into();

        let origin = app_infos::origin();

        let device = Device::default()
            .name(format!("Stove {name}"))
            .add_identifier(unique_id)
            .configuration_url(format!("https://www.rika-firenet.com/web/stove/{id}"))
            .manufacturer(manufacturer)
            .model(model)
            .sw_version(version);

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
            topic_prefix: topic_prefix.clone(),
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

#[derive(Message, Debug, Clone, PartialEq)]
#[rtype(result = "()")]
enum StoveCommand {
    OnOff(bool),
    OperatingMode(i32),
    TargetTemperature(Decimal),
    IdleTemperature(Decimal),
    PowerHeating(i32),
    DailySchedulesEnabled(bool),
    FrostProtectionEnabled(bool),
    FrostProtectionTemperature(Decimal),
}

impl StoveCommand {
    fn apply_to(self, controls: &mut StoveControls) {
        match self {
            StoveCommand::OnOff(enabled) => controls.on_off = Some(enabled),
            StoveCommand::OperatingMode(mode) => controls.operating_mode = Some(mode),
            StoveCommand::TargetTemperature(temp) => {
                controls.target_temperature = Some(temp.to_string())
            }
            StoveCommand::IdleTemperature(temp) => {
                controls.set_back_temperature = Some(temp.to_string())
            }
            StoveCommand::PowerHeating(percent) => controls.heating_power = Some(percent),
            StoveCommand::DailySchedulesEnabled(enabled) => {
                controls.heating_times_active_for_comfort = Some(enabled)
            }
            StoveCommand::FrostProtectionEnabled(enabled) => {
                controls.frost_protection_active = Some(enabled)
            }
            StoveCommand::FrostProtectionTemperature(temp) => {
                controls.frost_protection_temperature = Some(temp.to_string())
            }
        };
    }
}

#[derive(Debug, new, Clone, PartialEq)]
struct RikaFirenetCommand {
    topic_prefix: String,
    command: StoveCommand,
}

impl TryFrom<MqttMessage> for RikaFirenetCommand {
    type Error = anyhow::Error;

    fn try_from(msg: MqttMessage) -> Result<Self, Self::Error> {
        let command_topic_re = Regex::new(&format!("^({COMMON_BASE_TOPIC}/[^/]+)/([^/]+)/set$"))
            .expect("A valid regular expression for rika stove command topic");
        match command_topic_re.captures(&msg.topic).map(|c| c.extract()) {
            Some((_, [topic_prefix, attribute])) => {
                let command = match attribute {
                    "power-on" => StoveCommand::OnOff(msg.payload.parse()?),
                    "operating-mode" => StoveCommand::OperatingMode(msg.payload.parse()?),
                    "target-temp" => StoveCommand::TargetTemperature(msg.payload.parse()?),
                    "idle-temp" => StoveCommand::IdleTemperature(msg.payload.parse()?),
                    "power-heating" => StoveCommand::PowerHeating(msg.payload.parse()?),
                    "daily-schedules-enable" => {
                        StoveCommand::DailySchedulesEnabled(msg.payload.parse()?)
                    }
                    "frost-protection-enable" => {
                        StoveCommand::FrostProtectionEnabled(msg.payload.parse()?)
                    }
                    "frost-protection-temp" => {
                        StoveCommand::FrostProtectionTemperature(msg.payload.parse()?)
                    }
                    unsupported_attr => bail!("Unsupported attribute: {unsupported_attr}"),
                };
                Ok(RikaFirenetCommand::new(topic_prefix.to_string(), command))
            }
            None => bail!("Unable to parse command from message: {msg:?}"),
        }
    }
}
