use actix::prelude::*;
use async_stream::stream;
use hass_mqtt_autodiscovery::{
    mqtt::{binary_sensor::BinarySensor, number::Number, sensor::Sensor},
    HomeAssistantMqtt,
};
use log::{debug, error, info, trace};
use rumqttc::v5::{
    mqttbytes::{
        v5::{ConnAck, Packet},
        QoS,
    },
    AsyncClient, ConnectionError, Event, MqttOptions,
};
use serde_json::Value;
use url::Url;

use crate::misc::{app_infos, hostname};

const BIRTH_LAST_WILL_TOPIC: &str = "homeassistant/status";
const BIRTH_PAYLOAD: &str = "online";
const LAST_WILL_PAYLOAD: &str = "offline";

pub struct MqttActor {
    mqtt_options: MqttOptions,
    mqtt_client: Option<AsyncClient>,
    ha_mqtt: Option<HomeAssistantMqtt>,
}

impl MqttActor {
    pub fn new(broker_url: Url, username: String, password: String) -> Self {
        let mqtt_options = MqttOptions::new(
            format!("{}@{}", app_infos::name(), hostname()),
            broker_url
                .host()
                .expect("A broker URL with a host")
                .to_string(),
            broker_url.port().expect("A broker URL with a port"),
        )
        .set_credentials(username, password)
        .clone();
        MqttActor {
            mqtt_options,
            mqtt_client: None,
            ha_mqtt: None,
        }
    }

    fn subscribe_ha_events(&self, ctx: &mut Context<Self>, ack: ConnAck) {
        if let Some(client) = self.mqtt_client.clone() {
            async move {
                client
                    .subscribe(BIRTH_LAST_WILL_TOPIC, QoS::AtLeastOnce)
                    .await;
            }
            .into_actor(self)
            .spawn(ctx);
        }
    }

    fn handle_error(&self, error: ConnectionError) {
        error!("error: {error}");
    }

    fn handle_event(&self, event: Event) {
        trace!("event: {event:?}");
    }
}

impl Actor for MqttActor {
    type Context = Context<Self>;

    fn started(&mut self, ctx: &mut Self::Context) {
        let (async_client, mut event_loop) = AsyncClient::new(self.mqtt_options.clone(), 10);
        self.mqtt_client = Some(async_client.clone());
        self.ha_mqtt = Some(HomeAssistantMqtt::new(async_client, "homeassistant/"));
        ctx.add_stream(stream! {
            loop {
                yield event_loop.poll().await;
            }
        });
    }
}

impl StreamHandler<Result<Event, ConnectionError>> for MqttActor {
    fn handle(&mut self, msg: Result<Event, ConnectionError>, ctx: &mut Self::Context) {
        match msg {
            Ok(Event::Incoming(Packet::ConnAck(ack))) => {
                self.subscribe_ha_events(ctx, ack);
            }
            Ok(event) => self.handle_event(event),
            Err(error) => self.handle_error(error),
        }
    }

    fn finished(&mut self, ctx: &mut Self::Context) {
        info!("Stopped listening MQTT events");
    }
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub enum EntityConfiguration {
    BinarySensor(BinarySensor),
    Number(Number),
    Sensor(Sensor),
}

impl Handler<EntityConfiguration> for MqttActor {
    type Result = ();

    fn handle(&mut self, msg: EntityConfiguration, ctx: &mut Self::Context) -> Self::Result {
        if let Some(ha_mqtt) = self.ha_mqtt.clone() {
            async move {
                let result = match msg {
                    EntityConfiguration::BinarySensor(binary_sensor) => {
                        ha_mqtt.publish_binary_sensor(binary_sensor).await
                    }
                    EntityConfiguration::Sensor(sensor) => ha_mqtt.publish_sensor(sensor).await,
                    EntityConfiguration::Number(number) => ha_mqtt.publish_number(number).await,
                };
                if let Err(error) = result {
                    error!("Unable to publish entity: {error}")
                }
            }
            .into_actor(self)
            .spawn(ctx);
        } else {
            error!("MQTT client not available")
        }
    }
}

#[derive(Message, Clone)]
#[rtype(result = "()")]
pub struct PublishEntityData {
    topic: String,
    payload: Value,
}

impl PublishEntityData {
    pub fn new(topic: String, payload: Value) -> Self {
        PublishEntityData { topic, payload }
    }
}

impl Handler<PublishEntityData> for MqttActor {
    type Result = ();

    fn handle(&mut self, msg: PublishEntityData, ctx: &mut Self::Context) -> Self::Result {
        match self.ha_mqtt.clone() {
            Some(ha_mqtt) => {
                let msg = msg.clone();
                async move {
                    let result = ha_mqtt.publish_data(&msg.topic, &msg.payload, None).await;
                    if let Err(error) = result {
                        error!("Unable to publish data: {error}")
                    }
                }
                .into_actor(self)
                .spawn(ctx);
            }
            None => error!("MQTT client not available"),
        }
    }
}
